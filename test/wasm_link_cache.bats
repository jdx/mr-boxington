#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"

  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/src"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "wasm-fixture"
version = "0.1.0"
edition = "2021"
EOF
  echo 'fn main() {}' >"$PROJECT/src/main.rs"

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

@test "a linked wasm binary restores into a distinct target directory" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"
  local cold_report="$BATS_TEST_TMPDIR/cold.json"
  local warm_report="$BATS_TEST_TMPDIR/warm.json"
  local relative_output="wasm32-unknown-unknown/debug/wasm-fixture.wasm"

  run env \
    CARGO_TARGET_DIR="$first_target" \
    MBX_STATS_REPORT="$cold_report" \
    "$MBX_BIN" build --offline --target wasm32-unknown-unknown \
    --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  assert_file_exists "$first_target/$relative_output"
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$cold_report"
  assert_success

  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_STATS_REPORT="$warm_report" \
    "$MBX_BIN" build --offline --target wasm32-unknown-unknown \
    --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  assert_file_exists "$second_target/$relative_output"
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$warm_report"
  assert_success

  run cmp "$first_target/$relative_output" "$second_target/$relative_output"
  assert_success
}
