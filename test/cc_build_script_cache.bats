#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  # CC and CXX are unset for the same reason MBX_CC is: an inherited compiler
  # choice would make mbx stand aside and the fixture prove nothing.
  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI MBX_CC MBX_CACHE_LINKS
  unset CC CXX HOST_CC HOST_CXX TARGET_CC TARGET_CXX MBX_REAL_CC MBX_REAL_CXX
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"
  # Every test in this file measures the compiler shim a build script calls.
  # Keep execution caching from restoring the script before it reaches that
  # shim; build-script execution has its own Rust integration coverage.
  export MBX_BUILD_SCRIPT_EXECUTION=0

  # A real compilation, not `cc -v`: that is what the adapter's identity probe
  # runs, and sharing the signal would let a regression there skip these tests
  # rather than fail them.
  printf 'int probe(void) { return 0; }\n' >"$BATS_TEST_TMPDIR/probe.c"
  if ! cc -c -o "$BATS_TEST_TMPDIR/probe.o" "$BATS_TEST_TMPDIR/probe.c" 2>/dev/null; then
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
    // Mirrors the cc crate's precedence: HOST_CC before CC.
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
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
  # Three: the crate, the C object, and the build script's own binary, which
  # is a native link and cached like any other.
  run grep -E '"hits"[[:space:]]*:[[:space:]]*3' "$warm_report"
  assert_success

  local restored
  restored="$(object_in "$second_target")"
  [ -n "$restored" ]
  run cmp "$compiled" "$restored"
  assert_success
}

@test "a bypassed compile leaves the stderr a build script reads empty" {
  # cc-rs decides whether a flag is supported by running the compiler and
  # checking that it wrote nothing to stderr. The shim stands in for that
  # compiler, so anything the shim prints there is read as the compiler's
  # answer: the flag is dropped, every later compilation carries different
  # arguments than the build that populated the cache, and none of them match
  # a stored key again. `-Wp,` is the trigger with the fewest moving parts --
  # the real compiler takes it silently, and mbx bypasses rather than model
  # what it forwards.
  local probe_project="$BATS_TEST_TMPDIR/probe"
  mkdir -p "$probe_project/src"
  cat >"$probe_project/Cargo.toml" <<'EOF'
[package]
name = "probe-fixture"
version = "0.1.0"
edition = "2021"
EOF
  cat >"$probe_project/build.rs" <<'EOF'
use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
    let source = out.join("flag_check.c");
    std::fs::write(&source, "int main(void) { return 0; }").expect("probe source");
    let output = Command::new(&compiler)
        .arg("-Wp,-DMBX_PROBE=1")
        .arg("-c")
        .arg("-o")
        .arg(out.join("flag_check.o"))
        .arg(&source)
        .output()
        .expect("the C compiler should run");
    assert!(output.status.success(), "the probe compile failed");
    assert!(
        output.stderr.is_empty(),
        "the compiler wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
EOF
  echo 'pub fn value() -> u32 { 7 }' >"$probe_project/src/lib.rs"
  run cargo generate-lockfile --offline --manifest-path "$probe_project/Cargo.toml"
  assert_success

  local report="$BATS_TEST_TMPDIR/probe.json"
  run env \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/probe-target" \
    MBX_STATS_REPORT="$report" \
    "$MBX_BIN" build --offline --manifest-path "$probe_project/Cargo.toml"
  assert_success

  # Guard the guard: a fixture that stopped bypassing would pass this test
  # without ever exercising the path that used to print.
  run grep -F 'cc-tool-passthrough' "$report"
  assert_success
}

@test "MBX_CC=0 leaves the build script's compiler alone" {
  local first_target="$BATS_TEST_TMPDIR/off-first"
  local second_target="$BATS_TEST_TMPDIR/off-second"
  local warm_report="$BATS_TEST_TMPDIR/off-warm.json"

  run env CARGO_TARGET_DIR="$first_target" MBX_CC=0 \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success

  # The C compilation is left alone, so the warm build hits the crate and the
  # build script's binary rather than all three.
  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_STATS_REPORT="$warm_report" \
    MBX_CC=0 \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*2' "$warm_report"
  assert_success
}
