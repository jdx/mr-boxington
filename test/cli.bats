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
