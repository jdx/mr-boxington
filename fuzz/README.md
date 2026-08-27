# Parser fuzzing

These targets exercise repository-owned parsers with arbitrary local input:

- `blob_pack`: blob-pack framing, length handling, staging, and digest checks;
- `cc_depfile`: the GNU dependency lists C and C++ compilers write;
- `cc_invocation`: C and C++ driver argument parsing and admission;
- `dep_info`: rustc's Makefile-style dependency metadata;
- `rustc_paths`: compiler argument parsing and mapped-path normalization.

Run all targets briefly with:

```console
cargo +nightly fuzz run blob_pack -- -max_total_time=30
cargo +nightly fuzz run cc_depfile -- -max_total_time=30
cargo +nightly fuzz run cc_invocation -- -max_total_time=30
cargo +nightly fuzz run dep_info -- -max_total_time=30
cargo +nightly fuzz run rustc_paths -- -max_total_time=30
```

The inputs are capped at 1 MiB so CI smoke runs and local campaigns stay
bounded. Crashes are written under `fuzz/artifacts/`; commit minimized
regressions to the matching corpus directory after reviewing them.
