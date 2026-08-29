#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"

  # A library, a binary, and a test target: the three shapes `cargo check
  # --all-targets` compiles, two of which used to bypass for naming a crate
  # type no linker was ever going to be asked for.
  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/src"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "check-fixture"
version = "0.1.0"
edition = "2021"
EOF
  cat >"$PROJECT/src/lib.rs" <<'EOF'
pub fn double(value: u32) -> u32 {
    value * 2
}

#[test]
fn doubles() {
    assert_eq!(double(21), 42);
}
EOF
  cat >"$PROJECT/src/main.rs" <<'EOF'
fn main() {
    println!("{}", check_fixture::double(21));
}
EOF

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

@test "a checked binary and test target restore into a distinct target directory" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"
  local cold_report="$BATS_TEST_TMPDIR/cold.json"
  local warm_report="$BATS_TEST_TMPDIR/warm.json"
  local bypasses="$BATS_TEST_TMPDIR/bypasses.tsv"

  run env \
    CARGO_TARGET_DIR="$first_target" \
    MBX_STATS_REPORT="$cold_report" \
    "$MBX_BIN" check --offline --all-targets --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$cold_report"
  assert_success

  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_STATS_REPORT="$warm_report" \
    MBX_BYPASS_LOG="$bypasses" \
    "$MBX_BIN" check --offline --all-targets --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  # Three, not one: the library, the binary, and the test target. Before these
  # compilations were admitted the library alone would have hit, so a laxer
  # assertion would pass with the feature switched off.
  run grep -E '"hits"[[:space:]]*:[[:space:]]*[3-9]' "$warm_report"
  assert_success

  # And none of them bypassed for being named after an artifact this
  # compilation never asked anyone to link.
  if [ -f "$bypasses" ]; then
    run grep -E '^unsupported-crate-type' "$bypasses"
    assert_failure
  fi
}
