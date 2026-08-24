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
