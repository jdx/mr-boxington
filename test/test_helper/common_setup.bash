#!/usr/bin/env bash

_common_setup() {
  load "test_helper/bats-support/load"
  load "test_helper/bats-assert/load"
  load "test_helper/bats-file/load"

  export PROJECT_ROOT
  PROJECT_ROOT="$(cd "$BATS_TEST_DIRNAME/.." && pwd)"
  local target_dir="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
  if [[ "$target_dir" != /* ]]; then
    target_dir="$PROJECT_ROOT/$target_dir"
  fi
  export MBX_BIN="${MBX_BIN:-$target_dir/debug/mbx}"

  if [[ ! -x "$MBX_BIN" ]]; then
    echo "mbx binary not found; run 'mise run build' first: $MBX_BIN" >&2
    return 1
  fi

  export HOME="$BATS_TEST_TMPDIR/home"
  export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
  export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data"
  mkdir -p "$HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
  cd "$BATS_TEST_TMPDIR" || return 1
}
