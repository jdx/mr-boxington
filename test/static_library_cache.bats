#!/usr/bin/env bats

# A `-sys` crate: its build script compiles C into a static archive and tells
# cargo to pass `-L native` and `-l static` to the library compile. rustc
# bundles the archive into the rlib, so the archive's content is part of the
# library's action key.

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI MBX_CACHE_LINKS
  unset CC CXX HOST_CC HOST_CXX TARGET_CC TARGET_CXX
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"
  # An edited crate would otherwise get private incremental state, which is
  # never published; these tests are about the shared key.
  export MBX_LEARNED_INCREMENTAL=0

  printf 'int probe(void) { return 0; }\n' >"$BATS_TEST_TMPDIR/probe.c"
  if ! cc -c -o "$BATS_TEST_TMPDIR/probe.o" "$BATS_TEST_TMPDIR/probe.c" 2>/dev/null; then
    skip "no C compiler is available"
  fi
  if ! command -v ar >/dev/null 2>&1; then
    skip "no archiver is available"
  fi

  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/hello-sys/src" "$PROJECT/app/src"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[workspace]
members = ["hello-sys", "app"]
resolver = "2"
EOF
  cat >"$PROJECT/hello-sys/Cargo.toml" <<'EOF'
[package]
name = "hello-sys"
version = "0.1.0"
edition = "2021"
links = "hello"
EOF
  write_hello_c 7
  # Hand-rolled rather than using the cc crate, so the suite resolves
  # offline. The directives are the ones cc emits for a static library.
  cat >"$PROJECT/hello-sys/build.rs" <<'EOF'
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=src/hello.c");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    // Mirrors the cc crate's precedence: HOST_CC before CC.
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
    let object = out.join("hello.o");
    let status = Command::new(&compiler)
        .args(["-O2", "-c", "-o"])
        .arg(&object)
        .arg("src/hello.c")
        .status()
        .expect("the C compiler should run");
    assert!(status.success());
    let archive = out.join("libhello.a");
    let _ = std::fs::remove_file(&archive);
    let status = Command::new("ar")
        .arg("crs")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("ar should run");
    assert!(status.success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=hello");
}
EOF
  cat >"$PROJECT/hello-sys/src/lib.rs" <<'EOF'
extern "C" {
    fn hello_value() -> u32;
}

pub fn value() -> u32 {
    unsafe { hello_value() }
}
EOF
  cat >"$PROJECT/app/Cargo.toml" <<'EOF'
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
hello-sys = { path = "../hello-sys" }
EOF
  echo 'fn main() { println!("{}", hello_sys::value()); }' >"$PROJECT/app/src/main.rs"

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

write_hello_c() {
  printf 'int hello_value(void) { return %s; }\n' "$1" >"$PROJECT/hello-sys/src/hello.c"
}

# Build into the named target directory and write the session's stats there.
# A second target directory shares the store but none of the outputs, so
# anything present in it afterwards was restored rather than rebuilt. (The
# default target directory is a view into the store; removing it removes a
# symlink, not the outputs.)
build_into() {
  run env CARGO_TARGET_DIR="$1" MBX_STATS_REPORT="$1.json" \
    "$MBX_BIN" build --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

@test "a library bundling its build script's static archive restores into a distinct target directory" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"

  build_into "$first_target"
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$first_target.json"
  assert_success
  # The library compile carries `-l static=hello`. It used to bypass for that;
  # now the archive is an input and the compile is published like any other.
  run grep -E '"(native-library|missing-native-library)"' "$first_target.json"
  assert_failure
  run "$first_target/debug/app"
  assert_output 7

  build_into "$second_target"
  # Nothing compiled: the build script, its run, the library that bundles the
  # archive, and the program all restored.
  run grep -E '"misses"[[:space:]]*:[[:space:]]*0' "$second_target.json"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*[3-9]' "$second_target.json"
  assert_success
  run grep -E '"(native-library|missing-native-library)"' "$second_target.json"
  assert_failure
  run "$second_target/debug/app"
  assert_output 7
}

@test "a rebuilt static archive gives the library a new key instead of a stale restore" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"

  build_into "$first_target"
  run "$first_target/debug/app"
  assert_output 7

  # Only the C source changes: the Rust source, the flags, and the archive's
  # name are what they were. The archive's content is the only difference,
  # and it must be enough to compile the library again.
  write_hello_c 8
  build_into "$first_target"
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$first_target.json"
  assert_success
  run "$first_target/debug/app"
  assert_output 8

  # Both archives' results are in the store now. A fresh target directory
  # must restore the one for the archive that exists, not the first one
  # published under the same name.
  build_into "$second_target"
  run grep -E '"misses"[[:space:]]*:[[:space:]]*0' "$second_target.json"
  assert_success
  run "$second_target/debug/app"
  assert_output 8
}
