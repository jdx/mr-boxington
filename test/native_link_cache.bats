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
  # Debug info is what makes a macOS link unportable, and rustc records it by
  # default in the test profile. Turning it off is what the predicate asks for,
  # and keeps this test the same on both platforms.
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "native-fixture"
version = "0.1.0"
edition = "2021"

[profile.test]
debug = false
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

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

# The test binary in `deps/`, which has no extension to match on.
test_binary() {
  find "$1/debug/deps" -type f -name 'native_fixture-*' ! -name '*.d' | head -1
}

@test "a linked test binary restores into a distinct target directory" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"
  local cold_report="$BATS_TEST_TMPDIR/cold.json"
  local warm_report="$BATS_TEST_TMPDIR/warm.json"

  run env \
    CARGO_TARGET_DIR="$first_target" \
    MBX_CACHE_LINKS=1 \
    MBX_STATS_REPORT="$cold_report" \
    "$MBX_BIN" test --offline --no-run --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$cold_report"
  assert_success

  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_CACHE_LINKS=1 \
    MBX_STATS_REPORT="$warm_report" \
    "$MBX_BIN" test --offline --no-run --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  # Two hits, not merely one: the library and the linked test binary. A host
  # whose linker mbx cannot identify bypasses the link and still restores the
  # library, so a laxer assertion would pass while the feature did nothing.
  run grep -E '"hits"[[:space:]]*:[[:space:]]*[2-9]' "$warm_report"
  assert_success
  # And it bypassed nothing beyond the probes cargo always makes.
  run grep -E '"(other|unsupported-crate-type|unportable-native-link)"' "$warm_report"
  assert_failure

  local first second
  first="$(test_binary "$first_target")"
  second="$(test_binary "$second_target")"
  assert_file_exists "$first"
  assert_file_exists "$second"

  # Byte-identical, still executable, and it still runs: a restored program
  # that cannot be executed would be a cache hit worth nothing.
  run cmp "$first" "$second"
  assert_success
  assert_file_executable "$second"
  run "$second"
  assert_success
}

@test "native links stay outside the cache until asked for" {
  local target="$BATS_TEST_TMPDIR/unasked-target"
  local bypasses="$BATS_TEST_TMPDIR/bypasses.tsv"

  run env \
    CARGO_TARGET_DIR="$target" \
    MBX_BYPASS_LOG="$bypasses" \
    "$MBX_BIN" test --offline --no-run --manifest-path "$PROJECT/Cargo.toml"
  assert_success

  run grep -E '^unsupported-crate-type' "$bypasses"
  assert_success
}
