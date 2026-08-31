#!/usr/bin/env bash

_common_setup() {
  load "test_helper/bats-support/load"
  load "test_helper/bats-assert/load"
  load "test_helper/bats-file/load"

  # Setup tests begin with transparent Cargo inactive. Developers may already
  # have mbx enabled globally, so keep that host configuration out of the
  # isolated test PATH.
  local inherited_cargo
  inherited_cargo="$(command -v cargo)"
  if [[ "$inherited_cargo" == */mbx/bin/cargo ]]; then
    local inherited_shim_dir="${inherited_cargo%/cargo}"
    PATH=":$PATH:"
    PATH="${PATH//:$inherited_shim_dir:/:}"
    PATH="${PATH#:}"
    PATH="${PATH%:}"
    export PATH
  fi

  export PROJECT_ROOT
  PROJECT_ROOT="$(cd "$BATS_TEST_DIRNAME/.." && pwd)"
  local target_dir="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
  if [[ "$target_dir" != /* ]]; then
    target_dir="$PROJECT_ROOT/$target_dir"
  fi
  MBX_BIN="${MBX_BIN:-$target_dir/debug/mbx}"
  if [[ "$MBX_BIN" != /* ]]; then
    MBX_BIN="$PROJECT_ROOT/$MBX_BIN"
  fi
  export MBX_BIN

  if [[ ! -x "$MBX_BIN" ]]; then
    echo "mbx binary not found; run 'mise run build' first: $MBX_BIN" >&2
    return 1
  fi

  # Keep the installed toolchain reachable after HOME is isolated below.
  export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
  export HOME="$BATS_TEST_TMPDIR/home"
  export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
  export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data"
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/cache/mbx"
  mkdir -p "$HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
  cd "$BATS_TEST_TMPDIR" || return 1
}
