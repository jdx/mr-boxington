#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI MBX_CC
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"

  if ! cc -v >/dev/null 2>&1; then
    skip "no C compiler is available"
  fi

  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/src" "$PROJECT/include"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "cc-fixture"
version = "0.1.0"
edition = "2021"
EOF
  echo 'int hello_value(void);' >"$PROJECT/include/hello.h"
  cat >"$PROJECT/src/hello.c" <<'EOF'
#include "hello.h"
int hello_value(void) { return 7; }
EOF
  # Hand-rolled rather than using the cc crate: this suite resolves offline,
  # and what is under test is the shim the build script inherits.
  cat >"$PROJECT/build.rs" <<'EOF'
use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let compiler = env::var("CC").unwrap_or_else(|_| "cc".into());
    let status = Command::new(&compiler)
        .arg("-O2")
        .arg("-Iinclude")
        .arg("-c")
        .arg("-o")
        .arg(out.join("hello.o"))
        .arg("src/hello.c")
        .status()
        .expect("the C compiler should run");
    assert!(status.success());
}
EOF
  echo 'pub fn value() -> u32 { 7 }' >"$PROJECT/src/lib.rs"

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

# Find the fixture's object beneath a target directory.
object_in() {
  find "$1" -name hello.o -type f | head -n 1
}

@test "a build script's C object restores into a distinct target directory" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"
  local cold_report="$BATS_TEST_TMPDIR/cold.json"
  local warm_report="$BATS_TEST_TMPDIR/warm.json"

  run env \
    CARGO_TARGET_DIR="$first_target" \
    MBX_STATS_REPORT="$cold_report" \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$cold_report"
  assert_success

  local compiled
  compiled="$(object_in "$first_target")"
  [ -n "$compiled" ]

  # A second target directory shares the store but none of the outputs, so
  # anything present afterwards was restored rather than rebuilt.
  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_STATS_REPORT="$warm_report" \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*2' "$warm_report"
  assert_success

  local restored
  restored="$(object_in "$second_target")"
  [ -n "$restored" ]
  run cmp "$compiled" "$restored"
  assert_success
}

@test "MBX_CC=0 leaves the build script's compiler alone" {
  local first_target="$BATS_TEST_TMPDIR/off-first"
  local second_target="$BATS_TEST_TMPDIR/off-second"
  local warm_report="$BATS_TEST_TMPDIR/off-warm.json"

  run env CARGO_TARGET_DIR="$first_target" MBX_CC=0 \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success

  # Only the Rust compilation is cached, so the warm build has one hit rather
  # than two.
  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_STATS_REPORT="$warm_report" \
    MBX_CC=0 \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$warm_report"
  assert_success
}
