# Behavioral tests

The command-line and end-to-end suite uses
[Bats](https://github.com/bats-core/bats-core), following the same layout as
fnox and hk. Bats and its assertion helpers are pinned as Git submodules so a
checkout has the exact test runner used by CI.

Initialize the runner once after cloning:

```bash
git submodule update --init --recursive
```

Run every Rust and behavioral test:

```bash
mise run test
```

Run only Bats tests, one file, or one named case:

```bash
mise run test:bats
bats test/cli.bats
bats test/cli.bats --filter "isolated store"
```

Every test loads `test/test_helper/common_setup.bash`, which selects the debug
binary and gives the test an isolated home, config, data, and cache directory.
Behavioral regressions belong here; Rust unit tests should continue to exercise
internal contracts. Periodic fuzz and performance campaigns remain CI concerns
rather than Bats cases.
