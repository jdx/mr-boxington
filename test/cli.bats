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
  assert_output --partial "inputs changed since the last recording"
  assert_output --partial "src/lib.rs"
}

@test "analyze charges a dependent's rebuild to the crate that changed" {
  export MBX_LEARNED_INCREMENTAL=0
  mkdir analyzed-project
  cd analyzed-project
  printf '[workspace]\nmembers = ["engine", "app"]\nresolver = "2"\n' >Cargo.toml
  cargo new --lib --vcs none engine
  cargo new --lib --vcs none app
  printf 'engine = { path = "../engine" }\n' >>app/Cargo.toml
  printf 'pub fn app() -> u64 { engine::add(1, 2) }\n' >app/src/lib.rs

  run "$MBX_BIN" check
  assert_success

  printf 'pub fn add(a: u64, b: u64) -> u64 { a + b + 1 }\n' >engine/src/lib.rs
  run "$MBX_BIN" check
  assert_success

  run "$MBX_BIN" analyze
  assert_success
  assert_output --partial "uncached compiler time by cause"
  assert_output --partial "inputs of engine changed (2)"
  assert_output --partial "then 1 crate that depends on it rebuilt: app"
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
  assert_output --partial '"incremental_directories": 0'
  assert_output --partial '"combined_total_bytes": 0'
  assert_output --partial '"byte_accounting": "logical"'

  run "$MBX_BIN" gc --json
  assert_success
  assert_output --partial '"action_store"'
  assert_output --partial '"byte_accounting": "logical"'
  assert_output --partial '"targets"'
  assert_output --partial '"incremental"'
  assert_output --partial '"remaining_bytes"'

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

@test "stats reports lifetime savings and a separate automatic pruning period" {
  mkdir -p "$MBX_CACHE_DIR/actions/savings/v1"
  cat > "$MBX_CACHE_DIR/actions/savings/v1/tally.json" <<'JSON'
{"version":1,"since_secs":1700000000,"builds":42,"cached_compilations":321,"avoided_compiler_ns":3059100000000,"freed_target_bytes":1073741824,"freed_store_bytes":1073741824,"auto_pruned_bytes":1073741824,"auto_pruned_since_secs":1750000000,"freed_requested_bytes":1073741824}
JSON

  run "$MBX_BIN" stats
  assert_success
  assert_output --partial "since 2023-11-14"
  assert_output --partial "50m 59s"
  assert_output --partial "automatically pruned"
  assert_output --partial "since 2025-06-15"
  assert_output --partial "duplication avoided"

  run "$MBX_BIN" stats --json
  assert_success
  assert_output --partial '"version": 1'
  assert_output --partial '"pruned_bytes": 2147483648'
  assert_output --partial '"automatically_pruned_bytes": 1073741824'
  assert_output --partial '"requested_removal_bytes": 1073741824'
}

@test "adopt moves an existing target directory under the managed root" {
  cargo init --lib --vcs none adopted-project
  mkdir -p adopted-project/target/debug
  printf 'old output' >adopted-project/target/debug/artifact

  run "$MBX_BIN" adopt adopted-project

  assert_success
  assert_output --partial "adopted $(pwd -P)/adopted-project/target"
  assert_link_exists adopted-project/target
  assert_file_exists adopted-project/target/debug/artifact
  [[ "$(readlink adopted-project/target)" == "$MBX_CACHE_DIR/targets/v1/"* ]]
  run "$MBX_BIN" cache stats
  assert_success
  assert_output --partial "target directories: 1"
}

@test "adopt --recursive finds every checkout and --dry-run moves nothing" {
  cargo init --lib --vcs none projects/one
  cargo init --lib --vcs none projects/two
  mkdir -p projects/one/target/debug projects/two/target/debug
  printf 'one' >projects/one/target/debug/artifact
  printf 'two' >projects/two/target/debug/artifact

  run "$MBX_BIN" adopt --recursive --dry-run projects

  assert_success
  assert_output --partial "would adopt $(pwd -P)/projects/one/target"
  assert_output --partial "would adopt $(pwd -P)/projects/two/target"
  assert_output --partial "would adopt 2 target directories"
  [[ ! -L projects/one/target && -d projects/one/target ]]

  run "$MBX_BIN" adopt -r projects

  assert_success
  assert_output --partial "adopted 2 target directories"
  assert_link_exists projects/one/target
  assert_link_exists projects/two/target
  assert_file_exists projects/one/target/debug/artifact
  assert_file_exists projects/two/target/debug/artifact
}

@test "adopt accepts a checkout named through a link" {
  cargo init --lib --vcs none projects/linked-project
  mkdir -p projects/linked-project/target/debug
  printf 'old output' >projects/linked-project/target/debug/artifact
  ln -s projects linked

  run "$MBX_BIN" adopt linked/linked-project

  assert_success
  assert_output --partial "adopted $(pwd -P)/projects/linked-project/target"
  assert_link_exists projects/linked-project/target
  assert_file_exists projects/linked-project/target/debug/artifact
}

@test "adopt leaves a configured target directory alone" {
  cargo init --lib --vcs none configured-project
  mkdir -p configured-project/.cargo configured-project/target
  printf '[build]\ntarget-dir = "target"\n' >configured-project/.cargo/config.toml

  run "$MBX_BIN" adopt configured-project

  assert_success
  assert_output --partial "left $(pwd -P)/configured-project/target alone"
  [[ ! -L configured-project/target && -d configured-project/target ]]
}

@test "an adopted target directory keeps its build fresh" {
  cargo init --lib --vcs none fresh-project
  cd fresh-project
  # Placement off, so the build fills a real directory for adopt to move.
  mkdir target
  MBX_TARGET_VIEWS=false run "$MBX_BIN" build
  assert_success
  assert_output --partial "Compiling"
  [[ ! -L target ]]

  run "$MBX_BIN" adopt

  assert_success
  assert_link_exists target

  run "$MBX_BIN" build

  assert_success
  refute_output --partial "Compiling"
  assert_output --partial "Finished"
}

@test "a build adopts an existing target directory without prompting" {
  cargo init --lib --vcs none built-project
  cd built-project
  MBX_TARGET_VIEWS=false run "$MBX_BIN" build
  assert_success
  [[ ! -L target && -d target ]]

  # Bats runs without a terminal, which is how an agent or script builds.
  run env -u CI -u GITHUB_ACTIONS "$MBX_BIN" build

  assert_success
  assert_output --partial "moved the existing target/ directory under the managed root"
  refute_output --partial "Compiling"
  assert_link_exists target
  [[ "$(readlink target)" == "$MBX_CACHE_DIR/targets/v1/"* ]]
}
