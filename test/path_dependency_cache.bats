#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI MBX_CC MBX_CACHE_LINKS
  unset CC CXX HOST_CC HOST_CXX TARGET_CC TARGET_CXX MBX_REAL_CC MBX_REAL_CXX
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"
  # Run the build script every time, so the warm build reaches the C shim.
  export MBX_BUILD_SCRIPT_EXECUTION=0

  # The workspace and the crate it depends on by path are siblings, and
  # neither is below HOME: the layout of a CI runner whose work directory is
  # outside its home directory.
  export WORKSPACE="$BATS_TEST_TMPDIR/checkout/app"
  export DEPENDENCY="$BATS_TEST_TMPDIR/checkout/dep"
  mkdir -p "$WORKSPACE/src" "$DEPENDENCY/src"
  cat >"$DEPENDENCY/Cargo.toml" <<'EOF'
[package]
name = "dep"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn one() -> u32 { 1 }' >"$DEPENDENCY/src/lib.rs"
  cat >"$WORKSPACE/Cargo.toml" <<'EOF'
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
dep = { path = "../dep" }
EOF
  echo 'fn main() { println!("{}", dep::one()); }' >"$WORKSPACE/src/main.rs"
}

# Add a build script that compiles C from the dependency's own sources.
native_build_script() {
  printf 'int probe(void) { return 0; }\n' >"$BATS_TEST_TMPDIR/probe.c"
  if ! cc -c -o "$BATS_TEST_TMPDIR/probe.o" "$BATS_TEST_TMPDIR/probe.c" 2>/dev/null; then
    skip "no C compiler is available"
  fi
  echo 'int dep_value(void) { return 1; }' >"$DEPENDENCY/src/dep.c"
  cat >"$DEPENDENCY/build.rs" <<'EOF'
use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
    let status = Command::new(&compiler)
        .args(["-O2", "-c", "-o"])
        .arg(out.join("dep.o"))
        .arg("src/dep.c")
        .status()
        .expect("the C compiler should run");
    assert!(status.success());
}
EOF
}

# Build into `target`, writing the stats report to `report`.
build() {
  run env \
    CARGO_TARGET_DIR="$1" \
    MBX_STATS_REPORT="$2" \
    "$MBX_BIN" build --offline --manifest-path "$WORKSPACE/Cargo.toml"
}

@test "a path dependency outside the workspace and home is stored and restored" {
  build "$BATS_TEST_TMPDIR/first-target" "$BATS_TEST_TMPDIR/cold.json"
  assert_success
  refute_output --partial "no stable cache mapping"

  build "$BATS_TEST_TMPDIR/second-target" "$BATS_TEST_TMPDIR/warm.json"
  assert_success
  refute_output --partial "no stable cache mapping"
  # The dependency and the binary. Before the dependency's package had a
  # root of its own only the binary could be stored, so one hit would pass
  # with the fix reverted.
  run grep -E '"hits"[[:space:]]*:[[:space:]]*2([^0-9]|$)' "$BATS_TEST_TMPDIR/warm.json"
  assert_success
}

@test "C sources a path dependency's build script compiles are restored" {
  native_build_script

  build "$BATS_TEST_TMPDIR/first-target" "$BATS_TEST_TMPDIR/cold.json"
  assert_success
  refute_output --partial "no stable cache mapping"

  build "$BATS_TEST_TMPDIR/second-target" "$BATS_TEST_TMPDIR/warm.json"
  assert_success
  refute_output --partial "no stable cache mapping"
  # The build script, the dependency, its C object, and the binary.
  run grep -E '"hits"[[:space:]]*:[[:space:]]*4([^0-9]|$)' "$BATS_TEST_TMPDIR/warm.json"
  assert_success
}
