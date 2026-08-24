#!/usr/bin/env bats

setup() {
  local rustup_home="${RUSTUP_HOME:-}"
  if command -v rustup >/dev/null 2>&1; then
    rustup_home="$(rustup show home)"
  fi

  load "test_helper/common_setup"
  _common_setup

  if [[ -n "$rustup_home" ]]; then
    export RUSTUP_HOME="$rustup_home"
  fi
  export CARGO_HOME="$BATS_TEST_TMPDIR/cargo-home"
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/mbx-cache"
  mkdir -p "$CARGO_HOME"
}

write_project() {
  local project="$1"
  mkdir -p "$project/src"
  cat >"$project/Cargo.toml" <<'EOF'
[package]
name = "fixture"
version = "0.1.0"
edition = "2021"
EOF
  cat >"$project/src/lib.rs" <<'EOF'
pub fn double(value: u32) -> u32 {
    value * 2
}
EOF
  cargo generate-lockfile --offline --manifest-path "$project/Cargo.toml"
}

@test "setup installs and configures the persistent wrapper" {
  local wrapper
  mkdir -p "$CARGO_HOME"
  cat >"$CARGO_HOME/config.toml" <<'EOF'
# keep me
[net]
offline = true
EOF

  run "$MBX_BIN" setup

  assert_success
  assert_output --partial "and configured $CARGO_HOME/config.toml"
  assert_file_contains "$CARGO_HOME/config.toml" "# keep me"
  assert_file_contains "$CARGO_HOME/config.toml" "offline = true"
  assert_file_contains "$CARGO_HOME/config.toml" "rustc-wrapper"
  wrapper="$(sed -n 's/.*rustc-wrapper = "\([^"]*\)".*/\1/p' "$CARGO_HOME/config.toml")"
  [[ -n "$wrapper" ]]
  assert_file_executable "$wrapper"
}

@test "setup leaves an existing rustc wrapper unchanged" {
  cat >"$CARGO_HOME/config.toml" <<'EOF'
[build]
rustc-wrapper = "sccache"
EOF

  run "$MBX_BIN" setup

  assert_success
  assert_output --partial "left $CARGO_HOME/config.toml unchanged"
  assert_file_contains "$CARGO_HOME/config.toml" 'rustc-wrapper = "sccache"'
}

@test "setup status update and uninstall cover the full lifecycle" {
  local wrapper
  cat >"$CARGO_HOME/config.toml" <<'EOF'
# keep me
[net]
offline = true
EOF

  run "$MBX_BIN" setup --status
  assert_failure
  assert_output --partial "not installed"

  run "$MBX_BIN" setup
  assert_success
  wrapper="$(sed -n 's/.*rustc-wrapper = "\([^"]*\)".*/\1/p' "$CARGO_HOME/config.toml")"
  [[ -n "$wrapper" ]]

  run "$MBX_BIN" setup --status
  assert_success
  assert_output --partial "installed and current"

  rm "$wrapper"
  printf 'stale wrapper\n' >"$wrapper"
  chmod +x "$wrapper"
  run "$MBX_BIN" setup --status
  assert_failure
  assert_output --partial "outdated"

  run "$MBX_BIN" setup --update
  assert_success
  assert_output --partial "updated"

  run "$MBX_BIN" setup --uninstall
  assert_success
  assert_file_not_exist "$wrapper"
  assert_file_contains "$CARGO_HOME/config.toml" "# keep me"
  assert_file_contains "$CARGO_HOME/config.toml" "offline = true"
  refute_file_contains "$CARGO_HOME/config.toml" "rustc-wrapper"

  run "$MBX_BIN" setup --uninstall
  assert_success
  assert_output --partial "was not installed"
}

@test "the persistent wrapper restores a second checkout without an mbx session" {
  local first="$BATS_TEST_TMPDIR/first-checkout"
  local second="$BATS_TEST_TMPDIR/second-checkout"
  local compiler="$BATS_TEST_TMPDIR/counting-rustc"
  local rustc_log="$BATS_TEST_TMPDIR/rustc.log"
  local real_rustc
  real_rustc="$(command -v rustc)"

  write_project "$first"
  write_project "$second"
  cat >"$compiler" <<'EOF'
#!/bin/sh
crate_name=0
for arg in "$@"; do
  if [ "$crate_name" = 1 ]; then
    if [ "$arg" = fixture ]; then
      printf 'compile\n' >>"$MBX_TEST_RUSTC_LOG"
    fi
    break
  fi
  if [ "$arg" = "--crate-name" ]; then
    crate_name=1
  fi
done
exec "$MBX_TEST_REAL_RUSTC" "$@"
EOF
  chmod +x "$compiler"

  run "$MBX_BIN" setup
  assert_success

  run env \
    CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/first-target" \
    MBX_TEST_REAL_RUSTC="$real_rustc" \
    MBX_TEST_RUSTC_LOG="$rustc_log" \
    RUSTC="$compiler" \
    cargo build --offline --manifest-path "$first/Cargo.toml"
  assert_success

  run env \
    CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/second-target" \
    MBX_TEST_REAL_RUSTC="$real_rustc" \
    MBX_TEST_RUSTC_LOG="$rustc_log" \
    RUSTC="$compiler" \
    cargo build --offline --manifest-path "$second/Cargo.toml"
  assert_success

  run grep -c '^compile$' "$rustc_log"
  assert_success
  assert_output "1"
}
