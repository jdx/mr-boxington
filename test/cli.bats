#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  _common_setup
}

@test "version identifies mbx" {
  run "$MBX_BIN" --version

  assert_success
  assert_output --regexp '^mbx [0-9]+\.[0-9]+\.[0-9]+'
}

@test "help describes the cache commands" {
  run "$MBX_BIN" --help

  assert_success
  assert_output --partial "cache"
  assert_output --partial "gc"
  assert_output --partial "doctor"
}

@test "doctor validates an isolated local installation" {
  run "$MBX_BIN" doctor

  assert_success
  assert_output --partial "cargo"
  assert_output --partial "cache"
  assert_output --partial "remote"
  assert_output --partial "0 failures"
}

@test "doctor reports the Cargo selected by the active project wrapper" {
  local wrapper_dir="$BATS_TEST_TMPDIR/command-wrappers/bin"
  local fallback_dir="$BATS_TEST_TMPDIR/fallback-bin"
  mkdir -p "$wrapper_dir" "$fallback_dir"
  printf '#!/bin/sh\nprintf "cargo 1.98.0 (active)\\n"\n' >"$wrapper_dir/cargo"
  printf '#!/bin/sh\nprintf "cargo 1.97.1 (fallback)\\n"\n' >"$fallback_dir/cargo"
  printf '#!/bin/sh\nprintf "rustc 1.98.0 (active)\\n"\n' >"$wrapper_dir/rustc"
  chmod +x "$wrapper_dir/cargo" "$fallback_dir/cargo" "$wrapper_dir/rustc"

  run env -u CARGO PATH="$wrapper_dir:$fallback_dir:/usr/bin:/bin" "$MBX_BIN" doctor

  assert_success
  assert_output --partial "cargo 1.98.0 (active)"
  refute_output --partial "cargo 1.97.1 (fallback)"
}

@test "a toolchain in front of a Cargo command still selects one" {
  cargo init --lib --vcs none toolchain-project
  cd toolchain-project

  run "$MBX_BIN" +not-a-real-toolchain check

  # Whoever answers — rustup, or a Cargo that is not its shim — names the
  # toolchain that was asked for, which is the proof it was handed over.
  assert_failure
  assert_output --partial "not-a-real-toolchain"
}

@test "a toolchain is refused in front of a command that compiles nothing" {
  run "$MBX_BIN" +1.91 gc

  assert_failure
  assert_output --partial "compiles nothing"
}

@test "an isolated store starts empty" {
  run "$MBX_BIN" cache stats

  assert_success
  assert_output --partial "0 B"
}

@test "explain reports why compilations bypass the cache" {
  cargo init --lib --vcs none explained-project
  cd explained-project

  run "$MBX_BIN" explain check

  assert_success
  assert_output --partial "cache explanation:"
  assert_output --partial "compiler-query"
}

@test "explain --last diagnoses a miss from recorded inputs" {
  # The edited crate would otherwise take private incremental state on its
  # first edit and skip the lookup this test wants explained. CI sets CI=1,
  # which disables that already; locally it has to be said.
  export MBX_LEARNED_INCREMENTAL=0
  cargo init --lib --vcs none missed-project
  cd missed-project

  run "$MBX_BIN" check
  assert_success

  touch src/lib.rs
  run "$MBX_BIN" check
  assert_success

  printf 'pub fn changed() {}\n' >src/lib.rs
  run "$MBX_BIN" check
  assert_success

  run "$MBX_BIN" explain --last
  assert_success
  assert_output --partial "last recorded build"
  assert_output --partial "missed crates"
  assert_output --partial "inputs changed since the last hit"
  assert_output --partial "src/lib.rs"
}

@test "inspection commands offer versioned JSON" {
  run "$MBX_BIN" cache dir --json
  assert_success
  assert_output --partial '"version": 1'
  assert_output --partial '"store"'

  run "$MBX_BIN" cache stats --json
  assert_success
  assert_output --partial '"objects": 0'
  assert_output --partial '"target_directories": 0'

  run "$MBX_BIN" gc --json
  assert_success
  assert_output --partial '"action_store"'
  assert_output --partial '"targets"'

  run "$MBX_BIN" doctor --json
  assert_success
  assert_output --partial '"checks"'
  assert_output --partial '"failures": 0'
}

@test "cargo new is forwarded" {
  run "$MBX_BIN" new --vcs none new-project

  assert_success
  assert_file_exist "new-project/Cargo.toml"
}

@test "cargo init is forwarded" {
  mkdir initialized-project
  cd initialized-project

  run "$MBX_BIN" init --vcs none

  assert_success
  assert_file_exist "Cargo.toml"
}

@test "Cargo aliases unknown to mbx are forwarded" {
  mkdir .cargo
  printf '[alias]\nmbx-probe = "new --vcs none"\n' >.cargo/config.toml

  run "$MBX_BIN" mbx-probe alias-project

  assert_success
  assert_file_exist "alias-project/Cargo.toml"
}

@test "Cargo rejects commands unknown to both tools" {
  run "$MBX_BIN" command-added-after-mbx --future-flag value

  assert_failure
  assert_output --regexp 'no such command: .*command-added-after-mbx'
}

@test "the first build explains what mbx set up, once" {
  export CI=false GITHUB_ACTIONS=false
  local project="$BATS_TEST_TMPDIR/first-run"
  mkdir -p "$project/src"
  cat >"$project/Cargo.toml" <<'EOF'
[package]
name = "first-run-fixture"
version = "0.1.0"
edition = "2021"
EOF
  echo 'fn main() {}' >"$project/src/main.rs"
  run cargo generate-lockfile --offline --manifest-path "$project/Cargo.toml"
  assert_success

  # A help run does its own bookkeeping but must not consume the explanation:
  # nothing was built and nothing was said.
  run "$MBX_BIN" build --help --manifest-path "$project/Cargo.toml"
  assert_success
  refute_output --partial "first build on this machine"

  run "$MBX_BIN" build --offline --manifest-path "$project/Cargo.toml"
  assert_success
  assert_output --partial "first build on this machine"
  # The caps are resolved from this machine's disk, so assert the shape of the
  # explanation rather than the numbers in it.
  assert_output --partial "pruned to"
  assert_output --partial "its checkout is gone"

  # A machine that has been told does not need telling again.
  run "$MBX_BIN" build --offline --manifest-path "$project/Cargo.toml"
  assert_success
  refute_output --partial "first build on this machine"
}

@test "CI explains object cache results without consuming local onboarding" {
  cargo init --lib --vcs none ci-output
  cd ci-output

  run env CI=true MBX_STATS_REPORT="$PWD/stats.json" "$MBX_BIN" check --offline
  assert_success
  refute_output --partial "first build on this machine"
  assert_output --partial "object cache:"
  assert_output --partial "no usable prior inputs or matching prediction"
  assert_output --partial "Cargo artifact reuse and CI cache archive transfers"
  assert_file_exist "$PWD/stats.json"

  # No-op builds should not print a report just for Cargo's compiler probes.
  run env CI=true "$MBX_BIN" check --offline
  assert_success
  refute_output --partial "mbx[cache]:"

  for style in short full off; do
    touch src/lib.rs
    run env CI=true MBX_SUMMARY="$style" "$MBX_BIN" check --offline
    assert_success
    refute_output --partial "object cache:"
    if [[ "$style" == off ]]; then
      refute_output --partial "mbx[cache]:"
    else
      assert_output --partial "mbx[cache]:"
    fi
  done

  touch src/lib.rs
  run env CI=false GITHUB_ACTIONS=true "$MBX_BIN" check --offline --quiet
  assert_success
  refute_output --partial "mbx[cache]:"
  refute_output --partial "first build on this machine"

  touch src/lib.rs
  run env CI=false GITHUB_ACTIONS=true "$MBX_BIN" check --offline
  assert_success
  assert_output --partial "object cache:"
  refute_output --partial "first build on this machine"

  run env CI=false GITHUB_ACTIONS=false "$MBX_BIN" check --offline
  assert_success
  assert_output --partial "first build on this machine"
}
