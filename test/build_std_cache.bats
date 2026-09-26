#!/usr/bin/env bats

# Cargo's `-Zbuild-std` compiles every standard library unit with
# `-Zforce-unstable-if-unmarked` and `RUSTC_BOOTSTRAP=1`. Building `core` needs
# nightly and `rust-src`, so this suite puts the same flag and variable on an
# ordinary crate instead: `RUSTC_BOOTSTRAP` lets any toolchain accept `-Z`, and
# the crate then goes through the same discovery, lookup, and publication a
# standard library unit would.

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"
  export RUSTFLAGS="-Zforce-unstable-if-unmarked"

  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/src"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "build-std-fixture"
version = "0.1.0"
edition = "2021"
EOF
  cat >"$PROJECT/src/lib.rs" <<'EOF'
pub fn double(value: u32) -> u32 {
    value * 2
}
EOF

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  # `explain --last` finds the build recorded for this directory.
  cd "$PROJECT"
}

build() {
  local target="$1" bootstrap="$2" report="$3"
  run env \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/$target" \
    RUSTC_BOOTSTRAP="$bootstrap" \
    MBX_STATS_REPORT="$BATS_TEST_TMPDIR/$report" \
    MBX_BYPASS_LOG="$BATS_TEST_TMPDIR/bypasses.tsv" \
    "$MBX_BIN" build --offline
  assert_success
}

@test "a standard-library-like unit is cached and keyed by RUSTC_BOOTSTRAP" {
  build first-target 1 cold.json
  build second-target 1 warm.json
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$BATS_TEST_TMPDIR/warm.json"
  assert_success

  if [ -f "$BATS_TEST_TMPDIR/bypasses.tsv" ]; then
    run grep -E '^unknown-flag' "$BATS_TEST_TMPDIR/bypasses.tsv"
    assert_failure
  fi

  # Still permitted, but by a different value, so the compilation must not be
  # served from the key published above. `___` is the crate name of Cargo's
  # target-information probe, which also passes `-Z`.
  build third-target "___,build_std_fixture" changed.json
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$BATS_TEST_TMPDIR/changed.json"
  assert_success
  run "$MBX_BIN" explain --last
  assert_success
  assert_output --partial "environment RUSTC_BOOTSTRAP"
}
