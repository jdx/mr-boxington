import contextlib
import io
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import real_world


class CountCompilersTest(unittest.TestCase):
    def test_excludes_wrappers_and_probe_invocations(self) -> None:
        listing = "\n".join(
            (
                "/opt/rust/bin/rustc --crate-name real src/lib.rs --emit=link",
                "/opt/rust/bin/rustc --crate-name=also_real src/main.rs --emit=link",
                "/opt/rust/bin/rustc --crate-name ___ -",
                "/opt/rust/bin/rustc --crate-name ___ --print=file-names",
                "/usr/bin/mbx /opt/rust/bin/rustc --crate-name wrapped src/lib.rs",
            )
        )
        completed = subprocess.CompletedProcess(
            ["ps", "-Ao", "args="], 0, stdout=listing, stderr=""
        )

        with mock.patch.object(real_world.subprocess, "run", return_value=completed):
            self.assertEqual(real_world.count_compilers(), 2)


class MedianCellTest(unittest.TestCase):
    def test_publishes_the_middle_trial_and_keeps_the_rest(self) -> None:
        trials = [
            {"tool": "mbx", "wall_duration_ns": 9, "stats": {"hits": 3}},
            {"tool": "mbx", "wall_duration_ns": 5, "stats": {"hits": 1}},
            {"tool": "mbx", "wall_duration_ns": 7, "stats": {"hits": 2}},
        ]

        cell = real_world.median_cell(trials)

        self.assertEqual(cell["wall_duration_ns"], 7)
        # The statistics come from the trial that produced the timing, not
        # from whichever trial happened to run last.
        self.assertEqual(cell["stats"], {"hits": 2})
        self.assertEqual(cell["trials"], 3)
        self.assertEqual(cell["wall_durations_ns"], [9, 5, 7])

    def test_even_trial_count_takes_the_lower_middle(self) -> None:
        trials = [{"tool": "mbx", "wall_duration_ns": ns} for ns in (4, 8, 6, 2)]

        self.assertEqual(real_world.median_cell(trials)["wall_duration_ns"], 4)


class ValidateTest(unittest.TestCase):
    def warm(self, duration: int, seed: int) -> list[dict[str, object]]:
        return [
            {
                "scenario": "warm",
                "results": [
                    {
                        "tool": "mbx",
                        "wall_duration_ns": duration,
                        "seed_wall_duration_ns": seed,
                        "stats": {"hits": 700, "restored_output_files": 1500},
                    }
                ],
            }
        ]

    def test_warm_must_beat_the_build_that_seeded_it(self) -> None:
        self.assertEqual(real_world.validate(self.warm(5, 20)), [])

        failures = real_world.validate(self.warm(25, 20))

        self.assertEqual(len(failures), 1)
        self.assertIn("no faster than the cold build that seeded it", failures[0])

    def test_warm_restoring_nothing_is_not_a_cache_result(self) -> None:
        scenarios = self.warm(5, 20)
        scenarios[0]["results"][0]["stats"] = {"hits": 0, "restored_output_files": 0}

        failures = real_world.validate(scenarios)

        self.assertEqual(len(failures), 1)
        self.assertIn("restored nothing", failures[0])

    def test_an_edit_that_compiled_nothing_fails_the_run(self) -> None:
        scenarios: list[dict[str, object]] = [
            {
                "scenario": "edit",
                "results": [
                    {"tool": "cargo", "wall_duration_ns": 3, "recompiled": True},
                    {"tool": "mbx", "wall_duration_ns": 2, "recompiled": False},
                ],
            }
        ]

        failures = real_world.validate(scenarios)

        self.assertEqual(len(failures), 1)
        self.assertIn("mbx rebuilt without compiling anything", failures[0])


    def test_a_wrapped_cargo_is_not_an_uncached_baseline(self) -> None:
        scenarios: list[dict[str, object]] = [
            {
                "scenario": "commit",
                "results": [{"tool": "cargo", "wall_duration_ns": 3, "wrapped": True}],
            }
        ]

        failures = real_world.validate(scenarios)

        self.assertEqual(len(failures), 1)
        self.assertIn("not the uncached control", failures[0])


class PublishableTest(unittest.TestCase):
    def test_keeps_every_trial_timing_and_drops_the_rest(self) -> None:
        result = {
            "scenarios": [
                {
                    "scenario": "warm",
                    "results": [
                        {
                            "tool": "mbx",
                            "wall_duration_ns": 7,
                            "trials": 3,
                            "wall_durations_ns": [9, 5, 7],
                            # Gate inputs and log excerpts stay in the run's
                            # own output directory; the published file is read
                            # by a person in a pull request.
                            "seed_wall_duration_ns": 20,
                            "recompiled": True,
                            "summary": ["noise"],
                            "stats": {"hits": 700, "internal": 1},
                        }
                    ],
                }
            ]
        }

        cell = real_world.publishable(result)["scenarios"][0]["results"][0]

        self.assertEqual(cell["trials"], 3)
        self.assertEqual(cell["wall_durations_ns"], [9, 5, 7])
        self.assertEqual(cell["stats"], {"hits": 700})
        for dropped in ("seed_wall_duration_ns", "recompiled", "summary"):
            self.assertNotIn(dropped, cell)


class RunScenarioTest(unittest.TestCase):
    def test_each_scenario_says_what_its_cargo_row_is(self) -> None:
        # The same tool means two different things: a control with no cache to
        # help it in one scenario, the incremental rebuild to beat in the
        # other. The page labels the row from this rather than assuming.
        self.assertEqual(
            self.run_trials(None, scenario="edit")["baseline"], "incremental rebuild"
        )
        self.assertEqual(
            self.run_trials(None, scenario="commit")["baseline"], "uncached baseline"
        )
        self.assertNotIn("baseline", self.run_trials(None))

    def run_trials(
        self, failing_trial: int | None, scenario: str = "warm"
    ) -> dict[str, object]:
        seen: list[str] = []

        def trial(scenario, tool, cell, subject, runner, work):  # noqa: ANN001
            seen.append(cell)
            if len(seen) == failing_trial:
                raise RuntimeError(f"{cell}/{tool} build failed")
            return {"tool": tool, "wall_duration_ns": 5}

        with tempfile.TemporaryDirectory() as temporary:
            with contextlib.redirect_stderr(io.StringIO()):
                with mock.patch.object(real_world, "one_trial", side_effect=trial):
                    return real_world.run_scenario(
                        scenario,
                        ("kache",),
                        {},
                        None,
                        Path(temporary),
                        real_world.Progress(3),
                        3,
                    )

    def test_a_tool_that_finished_every_trial_is_published(self) -> None:
        entry = self.run_trials(None)

        self.assertEqual(entry["results"][0]["trials"], 3)
        self.assertEqual(entry["skipped"], [])

    def test_a_failed_trial_does_not_keep_its_work_directories(self) -> None:
        # A late failure in a big scenario would otherwise hold its target and
        # store for every trial that follows it.
        seen: list[Path] = []

        def trial(scenario, tool, cell, subject, runner, work):  # noqa: ANN001
            (work / f"target-{cell}").mkdir()
            seen.append(work)
            raise RuntimeError(f"{cell}/{tool} build failed")

        with tempfile.TemporaryDirectory() as temporary:
            with contextlib.redirect_stderr(io.StringIO()):
                with mock.patch.object(real_world, "one_trial", side_effect=trial):
                    real_world.run_scenario(
                        "warm",
                        ("kache",),
                        {},
                        None,
                        Path(temporary),
                        real_world.Progress(3),
                        3,
                    )

            self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_a_tool_that_dropped_out_partway_publishes_nothing(self) -> None:
        # Otherwise the note calling it unmeasured sits beside a cell holding
        # a median of whichever trials happened to finish first.
        entry = self.run_trials(2)

        self.assertEqual(entry["results"], [])
        self.assertEqual(len(entry["skipped"]), 1)
        self.assertIn("build failed", entry["skipped"][0])


class LocalEnvironmentTest(unittest.TestCase):
    def environment(self, local: bool) -> dict[str, str]:
        runner = real_world.Runner(Path("/out"), Path("/cargo-home"), Path("/mbx"))
        with mock.patch.dict(
            real_world.os.environ,
            {"CI": "true", "GITHUB_ACTIONS": "true", "RUSTC_WRAPPER": "/inherited"},
        ):
            return runner.base_environment({"toolchain": "1.97.1"}, Path("/target"), local)

    def test_a_runner_scenario_keeps_the_runner_it_is_on(self) -> None:
        environment = self.environment(local=False)

        self.assertEqual(environment["CARGO_INCREMENTAL"], "0")
        self.assertEqual(environment["CI"], "true")

    def test_the_edit_loop_does_not_look_like_ci(self) -> None:
        # mbx turns learned incremental reuse off when CI is set, so leaving it
        # would time the local loop with the feature that makes it fast
        # disabled, on a machine nobody edits code on.
        environment = self.environment(local=True)

        self.assertEqual(environment["CARGO_INCREMENTAL"], "1")
        self.assertNotIn("CI", environment)
        self.assertNotIn("GITHUB_ACTIONS", environment)
        # The uncached baseline stays uncached either way.
        self.assertNotIn("RUSTC_WRAPPER", environment)


class DiscardTest(unittest.TestCase):
    def test_removes_one_cell_without_touching_its_neighbours(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            for name in (
                "store-warm-mbx-1",
                "target-warm-mbx-1",
                "checkout-warm-mbx-1-seed",
                "store-warm-mbx-10",
                "target-warm-kache-1",
                "mirror.git",
            ):
                (work / name).mkdir()

            real_world.discard(work, "warm-mbx-1")

            self.assertEqual(
                sorted(path.name for path in work.iterdir()),
                ["mirror.git", "store-warm-mbx-10", "target-warm-kache-1"],
            )


class GuardSummaryTest(unittest.TestCase):
    def test_worktree_guard_reports_hits_rather_than_seconds(self) -> None:
        summary = real_world.guard_summary(
            "worktree",
            [{"tool": "mbx", "stats": {"hits": 926, "restored_output_files": 1462}}],
        )

        assert summary is not None
        self.assertIn("1462 output files", summary)
        self.assertIn("926 cache hits", summary)
        self.assertNotIn("faster", summary)

    def test_a_scenario_that_measured_nothing_has_nothing_to_claim(self) -> None:
        self.assertIsNone(real_world.guard_summary("worktree", []))


if __name__ == "__main__":
    unittest.main()
