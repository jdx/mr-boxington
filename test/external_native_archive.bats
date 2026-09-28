#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR CI MBX_INCREMENTAL
  export CARGO_INCREMENTAL=0
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"
  export MBX_LEARNED_INCREMENTAL=0
  export PROJECT="$BATS_TEST_TMPDIR/project"
  export EXTERNAL_NATIVE="$BATS_TEST_TMPDIR/external-native"
  mkdir -p "$PROJECT/src" "$EXTERNAL_NATIVE"

  if ! command -v cc >/dev/null 2>&1 || ! command -v ar >/dev/null 2>&1; then
    skip "a C compiler and archiver are required"
  fi

  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "external-native-archive"
version = "0.1.0"
edition = "2021"
build = "build.rs"
EOF
  cat >"$PROJECT/build.rs" <<'EOF'
fn main() {
    println!("cargo:rustc-link-search=native={}", std::env::var("EXTERNAL_NATIVE").unwrap());
}
EOF
  cat >"$PROJECT/src/lib.rs" <<'EOF'
#[link(name = "fixture", kind = "static")]
extern "C" { fn answer() -> i32; }

pub fn value() -> i32 { unsafe { answer() } }
EOF
  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

write_archive() {
  printf 'int answer(void) { return %s; }\n' "$1" >"$EXTERNAL_NATIVE/fixture.c"
  cc -c "$EXTERNAL_NATIVE/fixture.c" -o "$EXTERNAL_NATIVE/fixture.o"
  rm -f "$EXTERNAL_NATIVE/libfixture.a"
  ar crs "$EXTERNAL_NATIVE/libfixture.a" "$EXTERNAL_NATIVE/fixture.o"
}

build_into() {
  run env CARGO_TARGET_DIR="$1" MBX_STATS_REPORT="$1.json" \
    "$MBX_BIN" build --offline --lib --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  assert_output --partial 'result was not stored: rustc search path kind is not cacheable yet: native'
}

@test "a source-level native archive outside mapped roots cannot restore a stale rlib" {
  local first="$BATS_TEST_TMPDIR/first-target"
  local second="$BATS_TEST_TMPDIR/second-target"
  write_archive 7
  build_into "$first"

  write_archive 8
  build_into "$second"

  local first_rlib second_rlib
  first_rlib="$(find "$first/debug" -name 'libexternal_native_archive*.rlib' -print -quit)"
  second_rlib="$(find "$second/debug" -name 'libexternal_native_archive*.rlib' -print -quit)"
  assert_file_exists "$first_rlib"
  assert_file_exists "$second_rlib"
  run cmp "$first_rlib" "$second_rlib"
  assert_failure
}
