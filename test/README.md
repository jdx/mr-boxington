# Behavioral tests

The command-line and end-to-end suite uses
[Bats](https://github.com/bats-core/bats-core), with isolated homes and cache directories for each test. Bats and its assertion helpers are pinned as Git submodules so a
checkout has the exact test runner used by CI.

Initialize the runner once after cloning:

```bash
git submodule update --init --recursive
```

Install the pinned toolchain with `mise install`, then add the target used by
the WebAssembly test:

```sh
mise exec -- rustup target add wasm32-unknown-unknown
```

Run every Rust and behavioral test:

```bash
mise run test
```

Run only Bats tests, one file, or one named case:

```bash
mise run test:bats
MBX_BIN="$PWD/target/mbx-bootstrap/mbx" test/bats/bin/bats test/cli.bats
MBX_BIN="$PWD/target/mbx-bootstrap/mbx" test/bats/bin/bats --filter "isolated store" test/cli.bats
```

The individual commands use the bootstrap binary produced by `mise run build`.
`mise run test:e2e` selects Bats on Unix and the PowerShell suite in `e2e-win/`
on Windows.

Every test loads `test/test_helper/common_setup.bash`, which selects the debug
binary and gives the test an isolated home, config, data, and cache directory.
Behavioral regressions belong here; Rust unit tests should continue to exercise
internal contracts. Periodic fuzz and performance campaigns remain CI concerns
rather than Bats cases.
