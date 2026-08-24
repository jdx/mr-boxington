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
