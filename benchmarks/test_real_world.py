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

    def test_a_trial_that_restored_nothing_fails_even_when_another_is_median(
        self,
    ) -> None:
        # The median trial's own statistics look fine. The bad trial is still
        # in the published spread, which is what the page reads a result
        # against, so it has to fail the run.
        good = {"stats": {"hits": 700, "restored_output_files": 1500}}
        scenarios: list[dict[str, object]] = [
            {
                "scenario": "warm",
                "results": [
                    {
                        "tool": "mbx",
                        "wall_duration_ns": 5,
                        "seed_wall_duration_ns": 20,
                        **good,
                        "trial_cells": [
                            {"tool": "mbx", "wall_duration_ns": 3, "stats": {"hits": 0}},
                            {"tool": "mbx", "wall_duration_ns": 5, **good},
                            {"tool": "mbx", "wall_duration_ns": 9, **good},
                        ],
                    }
                ],
            }
        ]

        failures = real_world.validate(scenarios)

        self.assertEqual(len(failures), 1)
        self.assertIn("restored nothing", failures[0])

    def test_a_trial_that_compiled_nothing_fails_even_when_another_is_median(
        self,
    ) -> None:
        scenarios: list[dict[str, object]] = [
            {
                "scenario": "edit",
                "results": [
                    {
                        "tool": "mbx",
                        "wall_duration_ns": 5,
                        "recompiled": True,
                        "trial_cells": [
                            {"tool": "mbx", "wall_duration_ns": 3, "recompiled": False},
                            {"tool": "mbx", "wall_duration_ns": 5, "recompiled": True},
                        ],
                    }
                ],
            }
        ]

        failures = real_world.validate(scenarios)

        self.assertEqual(len(failures), 1)
        self.assertIn("rebuilt without compiling anything", failures[0])

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
                            "trial_cells": [{"tool": "mbx", "wall_duration_ns": 9}],
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
        for dropped in ("seed_wall_duration_ns", "recompiled", "summary", "trial_cells"):
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


class TrialOrderTest(unittest.TestCase):
    def test_trials_alternate_between_tools(self) -> None:
        # A runner that slows partway through must slow every tool's later
        # trials alike, not only whichever tool happened to run last.
        order: list[str] = []

        def trial(scenario, tool, cell, subject, runner, work):  # noqa: ANN001
            order.append(cell)
            return {"tool": tool, "wall_duration_ns": 1}

        with tempfile.TemporaryDirectory() as temporary:
            with contextlib.redirect_stderr(io.StringIO()):
                with mock.patch.object(real_world, "one_trial", side_effect=trial):
                    entry = real_world.run_scenario(
                        "contention",
                        ("mbx", "mbx-previous"),
                        {},
                        None,
                        Path(temporary),
                        real_world.Progress(4),
                        2,
                    )

        self.assertEqual(
            order,
            [
                "contention-mbx-1",
                "contention-mbx-previous-1",
                "contention-mbx-2",
                "contention-mbx-previous-2",
            ],
        )
        self.assertEqual([cell["tool"] for cell in entry["results"]], ["mbx", "mbx-previous"])

    def test_a_skipped_tool_is_not_tried_again(self) -> None:
        calls: list[str] = []

        def trial(scenario, tool, cell, subject, runner, work):  # noqa: ANN001
            calls.append(cell)
            if tool == "mbx-previous":
                raise real_world.Skipped("no mbx binary was given (--previous-mbx)")
            return {"tool": tool, "wall_duration_ns": 1}

        with tempfile.TemporaryDirectory() as temporary:
            with contextlib.redirect_stderr(io.StringIO()):
                with mock.patch.object(real_world, "one_trial", side_effect=trial):
                    entry = real_world.run_scenario(
                        "contention",
                        ("mbx", "mbx-previous"),
                        {},
                        None,
                        Path(temporary),
                        real_world.Progress(4),
                        2,
                    )

        self.assertEqual(calls.count("contention-mbx-previous-1"), 1)
        self.assertNotIn("contention-mbx-previous-2", calls)
        self.assertEqual([cell["tool"] for cell in entry["results"]], ["mbx"])
        self.assertEqual(entry["skipped"], ["mbx-previous: no mbx binary was given (--previous-mbx)"])


class PreviousReleaseTest(unittest.TestCase):
    def test_the_previous_release_needs_its_own_binary(self) -> None:
        runner = real_world.Runner(Path("/out"), Path("/cargo-home"), Path("/mbx"))
        with self.assertRaisesRegex(real_world.Skipped, "--previous-mbx"):
            runner.invocation(
                tool="mbx-previous",
                cell="contention-mbx-previous-1",
                subject={"args": ["build"]},
                target=Path("/target"),
                store=Path("/store"),
            )

    def test_the_previous_release_runs_its_own_binary_scheduled(self) -> None:
        runner = real_world.Runner(
            Path("/out"), Path("/cargo-home"), Path("/mbx"), Path("/previous/mbx")
        )
        command, environment = runner.invocation(
            tool="mbx-previous",
            cell="contention-mbx-previous-1",
            subject={"args": ["build"]},
            target=Path("/target"),
            store=Path("/store"),
        )

        self.assertEqual(command, ["/previous/mbx", "build"])
        self.assertEqual(environment["MBX_SCHEDULER"], "1")

    def test_the_summary_names_and_compares_the_previous_release(self) -> None:
        cell = lambda tool, times: {  # noqa: E731
            "tool": tool,
            "wall_duration_ns": sorted(times)[len(times) // 2],
            "wall_durations_ns": times,
            "peak_compilers": 32,
            "permits": 32,
            "min_available_bytes": 60e9,
            "stats": {"hits": 1},
        }
        result = {
            "subject": "hk",
            "toolchain": "1.94.0",
            "versions": {"mbx": "1.17.0", "mbx-previous": "1.16.0"},
            "failures": [],
            "scenarios": [
                {
                    "scenario": "contention",
                    "description": "six jobs",
                    "kind": "contention",
                    "skipped": [],
                    "results": [
                        cell("mbx", [32_500_000_000, 32_900_000_000, 35_200_000_000]),
                        cell("mbx-previous", [33_600_000_000, 37_800_000_000, 38_900_000_000]),
                    ],
                }
            ],
        }

        summary = real_world.summarize(result)

        self.assertIn("| mbx 1.16.0 (previous release) | 37.8 s |", summary)
        self.assertIn(
            "On this runner, mbx 1.17.0 finished the scheduled batch 4.9 s sooner than "
            "mbx 1.16.0 (trials 32.5-35.2 s against 33.6-38.9 s).",
            summary,
        )


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


class ToolchainTest(unittest.TestCase):
    def environment(self, subject: dict[str, object]) -> dict[str, str]:
        runner = real_world.Runner(Path("/out"), Path("/cargo-home"), Path("/mbx"))
        with mock.patch.dict(real_world.os.environ, {}, clear=False):
            real_world.os.environ.pop("RUSTUP_TOOLCHAIN", None)
            return runner.base_environment(subject, Path("/target"), local=False)

    def test_a_subject_without_a_toolchain_builds_under_the_runners(self) -> None:
        # The published numbers come from the compiler the runner ships, so the
        # benchmark must not quietly select a different one.
        self.assertNotIn("RUSTUP_TOOLCHAIN", self.environment({}))

    def test_a_subject_that_names_a_toolchain_still_gets_it(self) -> None:
        environment = self.environment({"toolchain": "1.91"})

        self.assertEqual(environment["RUSTUP_TOOLCHAIN"], "1.91")

    def test_the_recorded_release_comes_out_of_the_version_line(self) -> None:
        self.assertEqual(
            real_world.rust_release("rustc 1.97.1 (8bab26f4f 2026-07-14)"), "1.97.1"
        )
        self.assertIsNone(real_world.rust_release(None))
        self.assertIsNone(real_world.rust_release("rustc"))


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


if __name__ == "__main__":
    unittest.main()


class FilesystemTest(unittest.TestCase):
    def test_describes_the_scratch_tree_without_raising(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            described = real_world.filesystem(Path(directory))
        self.assertEqual(described["path"], directory)
        self.assertIn("clones", described)

    def test_clone_support_is_only_claimed_where_it_can_be_proven(self) -> None:
        # macOS `cp -c` copies and exits zero when it cannot clone, so the
        # probe must decline to answer rather than report support that is not
        # there.
        with mock.patch.object(real_world.sys, "platform", "darwin"):
            with tempfile.TemporaryDirectory() as directory:
                self.assertIsNone(real_world.clones_supported(Path(directory)))
