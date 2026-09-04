import subprocess
import unittest
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


if __name__ == "__main__":
    unittest.main()
