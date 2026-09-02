#!/usr/bin/env python3

import tempfile
import unittest
from pathlib import Path

import edit_loop


class EditLoopTests(unittest.TestCase):
    def test_environment_pins_benchmark_toolchain(self) -> None:
        env = edit_loop.environment("cargo", Path("work"), Path("report"))
        self.assertEqual(env["RUSTUP_TOOLCHAIN"], edit_loop.RUST_TOOLCHAIN)

    def test_percentile_uses_nearest_rank(self) -> None:
        self.assertEqual(edit_loop.percentile(list(range(1, 21)), 0.95), 19)
        self.assertEqual(edit_loop.percentile([3], 0.95), 3)

    def test_set_edit_uses_unique_values(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            checkout = Path(temporary)
            fixture = checkout / edit_loop.FIXTURE
            fixture.parent.mkdir(parents=True)
            fixture.write_text(edit_loop.BEFORE + "\n", encoding="utf-8")
            edit_loop.set_edit(checkout, 1)
            self.assertEqual(
                fixture.read_text(encoding="utf-8"), edit_loop.EDIT_PREFIX + "1);\n"
            )
            edit_loop.set_edit(checkout, 2)
            self.assertEqual(
                fixture.read_text(encoding="utf-8"), edit_loop.EDIT_PREFIX + "2);\n"
            )

    def test_compiler_duration_sums_outcomes(self) -> None:
        stats = {
            "compiler": {
                "incremental": {"invocations": 1, "duration_ns": 10},
                "bypass": {"invocations": 2, "duration_ns": 5},
            }
        }
        self.assertEqual(edit_loop.compiler_duration(stats), 15)


if __name__ == "__main__":
    unittest.main()
