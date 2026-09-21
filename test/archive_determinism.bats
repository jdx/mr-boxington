#!/usr/bin/env bats

# Apple's `ar` and `ranlib` stamp the current time into an archive. A build
# script that produces one hands Cargo an input whose bytes move between
# otherwise identical builds, so every action downstream of it misses. mbx sets
# `ZERO_AR_DATE` for build scripts to stop that, except under `release`, where
# the artifact's exact bytes are left as the host toolchain made them.

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset MBX_INCREMENTAL CARGO_INCREMENTAL CI
  unset ZERO_AR_DATE MBX_AR_DETERMINISM
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"
  # Pinned rather than managed, so the archive assertion can find the output
  # without following whatever placement mbx chose for this checkout.
  export CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/target"
  export MBX_LEARNED_INCREMENTAL=0

  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/src"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "archive-probe"
version = "0.1.0"
edition = "2021"
EOF
  echo 'fn main() {}' >"$PROJECT/src/main.rs"
  # Reports what mbx placed in the environment rather than inspecting an
  # archive, so the assertion is about mbx's policy and holds on every host
  # regardless of whether the local archiver stamps anything.
  cat >"$PROJECT/build.rs" <<'EOF'
fn main() {
    let value = std::env::var("ZERO_AR_DATE").unwrap_or_else(|_| "<unset>".into());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "<none>".into());
    println!("cargo:warning=probe PROFILE={profile} ZERO_AR_DATE={value}");
}
EOF
}

# Each case needs the build script to actually run. Its cache key does not
# include `ZERO_AR_DATE`, so a warm store would replay an earlier run's output
# and the assertion would describe the wrong build.
build_fresh() {
  rm -rf "$BATS_TEST_TMPDIR/store" "$CARGO_TARGET_DIR"
  run "$MBX_BIN" build "$@"
  [ "$status" -eq 0 ]
}

@test "a debug build script gets ZERO_AR_DATE" {
  cd "$PROJECT"
  build_fresh
  [[ "$output" == *"probe PROFILE=debug ZERO_AR_DATE=1"* ]]
}

@test "a release build script is left as the toolchain makes it" {
  cd "$PROJECT"
  build_fresh --release
  [[ "$output" == *"probe PROFILE=release ZERO_AR_DATE=<unset>"* ]]
}

@test "always covers release too" {
  cd "$PROJECT"
  export MBX_AR_DETERMINISM=always
  build_fresh --release
  [[ "$output" == *"probe PROFILE=release ZERO_AR_DATE=1"* ]]
}

@test "off leaves every profile alone" {
  cd "$PROJECT"
  export MBX_AR_DETERMINISM=off
  build_fresh
  [[ "$output" == *"probe PROFILE=debug ZERO_AR_DATE=<unset>"* ]]
}

@test "a ZERO_AR_DATE the developer set is never overridden" {
  cd "$PROJECT"
  export ZERO_AR_DATE=0
  build_fresh
  [[ "$output" == *"probe PROFILE=debug ZERO_AR_DATE=0"* ]]
}

@test "an archive built twice from unchanged objects keeps its digest" {
  if ! command -v ar >/dev/null 2>&1 || ! command -v ranlib >/dev/null 2>&1; then
    skip "no archiver is available"
  fi
  printf 'int probe(void) { return 0; }\n' >"$BATS_TEST_TMPDIR/probe.c"
  if ! cc -c -o "$BATS_TEST_TMPDIR/probe.o" "$BATS_TEST_TMPDIR/probe.c" 2>/dev/null; then
    skip "no C compiler is available"
  fi
  # Only meaningful where the archiver stamps a timestamp at all; a toolchain
  # that is already deterministic proves nothing either way.
  ( cd "$BATS_TEST_TMPDIR" && ar rc stamped.a probe.o && ranlib stamped.a )
  ( cd "$BATS_TEST_TMPDIR" && ar rc zeroed.a probe.o && ZERO_AR_DATE=1 ranlib zeroed.a )
  if cmp -s "$BATS_TEST_TMPDIR/stamped.a" "$BATS_TEST_TMPDIR/zeroed.a"; then
    skip "this archiver does not stamp a timestamp"
  fi

  # A build script that archives the same object on every run. Without the
  # policy its output differs run to run; with it the bytes are stable.
  cp "$BATS_TEST_TMPDIR/probe.c" "$PROJECT/probe.c"
  cat >"$PROJECT/build.rs" <<'EOF'
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let object = format!("{out}/probe.o");
    let archive = format!("{out}/libprobe.a");
    assert!(std::process::Command::new("cc")
        .args(["-c", "-o", &object, "probe.c"])
        .status().unwrap().success());
    let _ = std::fs::remove_file(&archive);
    assert!(std::process::Command::new("ar")
        .args(["rc", &archive, &object])
        .status().unwrap().success());
    assert!(std::process::Command::new("ranlib")
        .arg(&archive)
        .status().unwrap().success());
    println!("cargo:warning=archive={archive}");
    println!("cargo:rerun-if-changed=probe.c");
}
EOF

  cd "$PROJECT"
  build_fresh
  local first
  first="$(find "$CARGO_TARGET_DIR" -name libprobe.a -print -quit)"
  [ -n "$first" ]
  local first_digest
  first_digest="$(cksum <"$first")"

  # A second archiver run one clock tick later is what moves a stamped digest.
  sleep 1.1
  build_fresh
  local second
  second="$(find "$CARGO_TARGET_DIR" -name libprobe.a -print -quit)"
  [ -n "$second" ]
  [ "$first_digest" = "$(cksum <"$second")" ]
}
