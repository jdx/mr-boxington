#!/usr/bin/env bats

setup() {
  local rustup_home="${RUSTUP_HOME:-}"
  if command -v rustup >/dev/null 2>&1; then
    rustup_home="$(rustup show home)"
  fi
  load "test_helper/common_setup"
  _common_setup
  if [[ -n "$rustup_home" ]]; then export RUSTUP_HOME="$rustup_home"; fi
  export CARGO_HOME="$BATS_TEST_TMPDIR/cargo-home"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data-home"
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/mbx-cache"
  export MBX_SHIM_DIR="$XDG_DATA_HOME/mbx/bin"
  if [[ "$(uname -s)" == Darwin ]]; then
    export MBX_RA_CONFIG="$HOME/Library/Application Support/rust-analyzer/rust-analyzer.toml"
  else
    export MBX_RA_CONFIG="$XDG_CONFIG_HOME/rust-analyzer/rust-analyzer.toml"
  fi
  export MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR"
  unset MISE_CONFIG_FILE
  unset MISE_SHELL
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
pub fn double(value: u32) -> u32 { value * 2 }
EOF
  cargo generate-lockfile --offline --manifest-path "$project/Cargo.toml"
}

@test "setup installs a Cargo shim and prints manual activation without mise scope" {
  run "$MBX_BIN" setup
  assert_success
  assert_file_executable "$MBX_SHIM_DIR/cargo"
  [ ! -L "$MBX_SHIM_DIR/cargo" ]
  assert_file_contains "$MBX_RA_CONFIG" "$MBX_SHIM_DIR/cargo"
  assert_file_contains "$MBX_RA_CONFIG" 'message-format=json'
  assert_output --partial "export PATH=\"$MBX_SHIM_DIR"
  assert_output --partial ':$PATH"'
  local fish_shim_dir="$BATS_TEST_TMPDIR/Application Support/mbx/bin"
  run env SHELL=/usr/bin/fish MBX_TEST_SHIM_DIR="$fish_shim_dir" "$MBX_BIN" setup
  assert_success
  assert_file_executable "$fish_shim_dir/cargo"
  assert_output --partial "set -gx PATH '$fish_shim_dir' \$PATH"
  assert_output --partial "does not edit shell startup files"
}

@test "the Unix Cargo launcher survives removal of the setup-time mbx" {
  if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    skip "Unix launcher coverage"
  fi
  local old_mbx="$BATS_TEST_TMPDIR/versioned/mbx"
  local active_bin
  active_bin="$(dirname "$MBX_BIN")"
  mkdir -p "$(dirname "$old_mbx")"
  cp "$MBX_BIN" "$old_mbx"
  chmod +x "$old_mbx"
  "$old_mbx" setup >/dev/null
  rm "$old_mbx"

  run env PATH="$MBX_SHIM_DIR:$active_bin:$PATH" cargo --version
  assert_success
  assert_output --regexp '^cargo [0-9]'
}

@test "setup status refresh and uninstall cover the shim lifecycle" {
  run "$MBX_BIN" setup --status
  assert_failure
  assert_output --partial "not installed"
  run "$MBX_BIN" setup
  assert_success
  run "$MBX_BIN" setup --status
  assert_success
  assert_output --partial "installed and current"

  run "$MBX_BIN" doctor
  assert_success
  assert_output --partial "Cargo shim is current but not active"
  run env PATH="$MBX_SHIM_DIR:$PATH" "$MBX_BIN" doctor
  assert_success
  assert_output --partial "Cargo shim is active and current"

  rm "$MBX_SHIM_DIR/cargo"
  printf 'stale shim\n' >"$MBX_SHIM_DIR/cargo"
  chmod +x "$MBX_SHIM_DIR/cargo"
  run "$MBX_BIN" setup --status
  assert_failure
  assert_output --partial "outdated"
  run "$MBX_BIN" setup
  assert_output --partial "refreshed the Cargo shim"
  refute_output --partial "prepend the Cargo shim"
  assert_success
  run "$MBX_BIN" setup --uninstall
  assert_success
  assert_file_executable "$MBX_SHIM_DIR/cargo"
  assert_output --partial "left in place for other scopes"
  run grep -F 'overrideCommand' "$MBX_RA_CONFIG"
  assert_failure
}

@test "yes setup follows postinstall, global, and local mise scopes" {
  local fake_bin="$BATS_TEST_TMPDIR/fake-mise-bin"
  local mise_log="$BATS_TEST_TMPDIR/mise.log"
  local project_config="$BATS_TEST_TMPDIR/project/mise.toml"
  mkdir -p "$fake_bin" "$(dirname "$project_config")"
  touch "$project_config"
cat >"$fake_bin/mise" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$MBX_TEST_MISE_LOG"
if [ "$1 $2 $3" = "config set --help" ]; then
  printf '%s\n' '--append --remove --global'
fi
if [ "$1 $2 $3" = "config ls --json" ]; then
  printf '%s\n' "${MBX_TEST_MISE_CONFIGS:-[]}"
fi
if [ "$1 $2" = "config get" ]; then
  printf '%s\n' "$MBX_TEST_SHIM_DIR"
fi
EOF
  chmod +x "$fake_bin/mise"

  run env PATH="$fake_bin:$PATH" MBX_TEST_MISE_LOG="$mise_log" \
    MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" MISE_CONFIG_FILE="$project_config" \
    "$MBX_BIN" setup --yes
  assert_success
  assert_file_contains "$mise_log" "config set --append --file $project_config env._.path"
  assert_file_contains "$(dirname "$project_config")/rust-analyzer.toml" "$MBX_SHIM_DIR/cargo"

  run env -u MISE_CONFIG_FILE PATH="$fake_bin:$PATH" MISE_SHELL=zsh \
    MISE_GLOBAL_CONFIG_FILE="$BATS_TEST_TMPDIR/global.toml" \
    MBX_TEST_MISE_CONFIGS="[{\"path\":\"$BATS_TEST_TMPDIR/global.toml\",\"tools\":[\"mr-boxington\"]}]" \
    MBX_TEST_MISE_LOG="$mise_log" MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" \
    "$MBX_BIN" setup --yes
  assert_success
  assert_file_contains "$mise_log" "config set --append --global env._.path"
  assert_file_contains "$MBX_RA_CONFIG" "$MBX_SHIM_DIR/cargo"

  run env -u MISE_CONFIG_FILE PATH="$fake_bin:$PATH" MISE_SHELL=zsh \
    MBX_TEST_MISE_CONFIGS="[{\"path\":\"$project_config\",\"tools\":[\"mr-boxington\"]}]" \
    MBX_TEST_MISE_LOG="$mise_log" MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" \
    "$MBX_BIN" setup --yes
  assert_success
  assert_file_contains "$mise_log" "config set --append --file $project_config env._.path"

  cd "$(dirname "$project_config")"
  run env -u MISE_CONFIG_FILE PATH="$fake_bin:$PATH" MISE_SHELL=zsh \
    MBX_TEST_MISE_CONFIGS='[]' MBX_TEST_MISE_LOG="$mise_log" \
    MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" "$MBX_BIN" setup --yes
  assert_success
  assert_file_contains "$mise_log" "config set --append --file $project_config env._.path"

  : >"$mise_log"
  run env -u MISE_CONFIG_FILE -u MISE_SHELL PATH="$fake_bin:$PATH" \
    MBX_TEST_MISE_LOG="$mise_log" MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" \
    "$MBX_BIN" setup --yes
  assert_success
  run grep -F "config set --append" "$mise_log"
  assert_failure

  run env PATH="$fake_bin:$PATH" MBX_TEST_MISE_LOG="$mise_log" \
    MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" "$MBX_BIN" setup --local
  assert_success
  assert_file_contains "$mise_log" "config set --append env._.path"

  run env PATH="$fake_bin:$PATH" MISE_SHELL=zsh \
    MISE_GLOBAL_CONFIG_FILE="$BATS_TEST_TMPDIR/global.toml" \
    MBX_TEST_MISE_CONFIGS="[{\"path\":\"$BATS_TEST_TMPDIR/global.toml\",\"tools\":[\"mr-boxington\"]}]" \
    MBX_TEST_MISE_LOG="$mise_log" MBX_TEST_SHIM_DIR="$MBX_SHIM_DIR" \
    "$MBX_BIN" setup --uninstall
  assert_success
  assert_file_contains "$mise_log" "config set --remove --global env._.path"
}

@test "setup prints config instructions for older mise clients without editing" {
  local fake_bin="$BATS_TEST_TMPDIR/legacy-mise-bin"
  local global_config="$BATS_TEST_TMPDIR/legacy-global.toml"
  local project="$BATS_TEST_TMPDIR/legacy-project"
  mkdir -p "$fake_bin" "$project"
  project="$(cd "$project" && pwd -P)"
  cat >"$fake_bin/mise" <<'EOF'
#!/bin/sh
if [ "$1 $2 $3" = "config set --help" ]; then
  printf '%s\n' 'Usage: mise config set --file FILE KEY VALUE'
fi
EOF
  chmod +x "$fake_bin/mise"
  printf '# global comment\n' >"$global_config"
  printf '# local comment\n[env]\n_.path = "/existing/bin"\n' >"$project/mise.toml"

  run env PATH="$fake_bin:$PATH" MISE_GLOBAL_CONFIG_FILE="$global_config" \
    "$MBX_BIN" setup --global
  assert_success
  assert_output --partial "cannot update env._.path"
  assert_output --partial "add \"$MBX_SHIM_DIR\" to env._.path in $global_config"
  assert_file_contains "$global_config" "# global comment"
  run grep -F "$MBX_SHIM_DIR" "$global_config"
  assert_failure
  run env PATH="$fake_bin:$PATH" MISE_GLOBAL_CONFIG_FILE="$global_config" \
    "$MBX_BIN" setup --global --status
  assert_failure
  assert_output --partial "not active"

  cd "$project"
  run env PATH="$fake_bin:$PATH" "$MBX_BIN" setup --local
  assert_success
  assert_output --partial "add \"$MBX_SHIM_DIR\" to env._.path in $project/mise.toml"
  assert_file_contains "$project/mise.toml" "/existing/bin"
  assert_file_contains "$project/mise.toml" "# local comment"
  run grep -F "$MBX_SHIM_DIR" "$project/mise.toml"
  assert_failure

  printf '# local comment\n[env]\n_.path = ["/existing/bin", "%s"]\n' \
    "$MBX_SHIM_DIR" >"$project/mise.toml"
  run env PATH="$fake_bin:$PATH" "$MBX_BIN" setup --local --status
  assert_success
  assert_output --partial "installed and current"
  run env PATH="$fake_bin:$PATH" "$MBX_BIN" setup --local --uninstall
  assert_success
  assert_output --partial "remove \"$MBX_SHIM_DIR\" from env._.path in $project/mise.toml"
  assert_file_contains "$project/mise.toml" "$MBX_SHIM_DIR"
  assert_file_contains "$project/mise.toml" "/existing/bin"
}

@test "mise postinstall activates transparent Cargo in local and global scopes" {
  local project="$BATS_TEST_TMPDIR/mise-project"
  local outside="$BATS_TEST_TMPDIR/mise-outside"
  local first="$project/first-checkout"
  local second="$project/second-checkout"
  local compiler="$BATS_TEST_TMPDIR/mise-counting-rustc"
  local rustc_log="$BATS_TEST_TMPDIR/mise-rustc.log"
  local real_rustc
  real_rustc="$(command -v rustc)"
  export MISE_DATA_DIR="$BATS_TEST_TMPDIR/mise-data"
  export MISE_CACHE_DIR="$BATS_TEST_TMPDIR/mise-cache"
  export MISE_GLOBAL_CONFIG_FILE="$BATS_TEST_TMPDIR/mise-global.toml"
  mkdir -p "$project" "$outside"
  write_project "$first"
  write_project "$second"

  cat >"$compiler" <<'EOF'
#!/bin/sh
crate_name=0
for arg in "$@"; do
  if [ "$crate_name" = 1 ]; then
    if [ "$arg" = fixture ]; then printf 'compile\n' >>"$MBX_TEST_RUSTC_LOG"; fi
    break
  fi
  if [ "$arg" = "--crate-name" ]; then crate_name=1; fi
done
exec "$MBX_TEST_REAL_RUSTC" "$@"
EOF
  chmod +x "$compiler"

  cd "$project"
  run mise use --yes --postinstall "$MBX_BIN setup --yes" mr-boxington
  assert_success
  assert_file_executable "$MBX_SHIM_DIR/cargo"
  assert_file_contains "$project/mise.toml" "$MBX_SHIM_DIR"

  run mise exec -- sh -c 'command -v cargo'
  assert_success
  assert_output "$MBX_SHIM_DIR/cargo"
  run mise exec -- "$MBX_BIN" doctor
  assert_success
  assert_output --partial "Cargo shim is active and current"

  run mise exec -- env CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/mise-first-target" \
    MBX_TEST_REAL_RUSTC="$real_rustc" MBX_TEST_RUSTC_LOG="$rustc_log" RUSTC="$compiler" \
    sh -c 'cd "$1" && cargo build --offline' sh "$first"
  assert_success
  run mise exec -- env CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/mise-second-target" \
    MBX_TEST_REAL_RUSTC="$real_rustc" MBX_TEST_RUSTC_LOG="$rustc_log" RUSTC="$compiler" \
    sh -c 'cd "$1" && cargo build --offline' sh "$second"
  assert_success
  run grep -c '^compile$' "$rustc_log"
  assert_success
  assert_output "1"

  run env MISE_CONFIG_FILE="$project/mise.toml" "$MBX_BIN" setup --uninstall
  assert_success
  run grep -F "$MBX_SHIM_DIR" "$project/mise.toml"
  assert_failure

  cd "$outside"
  run mise use --yes --global --postinstall "$MBX_BIN setup --yes" mr-boxington
  assert_success
  assert_file_contains "$MISE_GLOBAL_CONFIG_FILE" "$MBX_SHIM_DIR"
  run mise exec -- sh -c 'command -v cargo'
  assert_success
  assert_output "$MBX_SHIM_DIR/cargo"
  run mise exec -- "$MBX_BIN" doctor
  assert_success
  assert_output --partial "Cargo shim is active and current"
}

@test "the Cargo shim preserves Cargo's namespace and disable escape hatch" {
  local real_bin="$BATS_TEST_TMPDIR/real-bin"
  mkdir -p "$real_bin"
  cat >"$real_bin/cargo" <<'EOF'
#!/bin/sh
if [ -n "${MBX_TEST_CARGO_LOG:-}" ]; then
  printf '%s\n' "$*" >>"$MBX_TEST_CARGO_LOG"
fi
printf '%s\n' "$*"
EOF
  chmod +x "$real_bin/cargo"
  "$MBX_BIN" setup >/dev/null

  run env PATH="$MBX_SHIM_DIR:$real_bin:/usr/bin:/bin" cargo cache stats
  assert_success
  assert_output "cache stats"
  run env MBX_DISABLE=1 PATH="$MBX_SHIM_DIR:$real_bin:/usr/bin:/bin" cargo build --quiet
  assert_success
  assert_output "build --quiet"
  run env PATH="$MBX_SHIM_DIR:$real_bin:/usr/bin:/bin" cargo +nightly --version
  assert_success
  assert_output "+nightly --version"

  local cargo_log="$BATS_TEST_TMPDIR/cargo.log"
  run env MBX_TEST_CARGO_LOG="$cargo_log" PATH="$MBX_SHIM_DIR:$real_bin:/usr/bin:/bin" cargo clean
  assert_success
  assert_output "clean"
  run grep -c '^clean$' "$cargo_log"
  assert_success
  assert_output "1"
}

@test "explicit mbx Cargo commands do not reenter the installed shim" {
  local project="$BATS_TEST_TMPDIR/explicit-mbx-project"
  write_project "$project"
  "$MBX_BIN" setup >/dev/null

  run env PATH="$MBX_SHIM_DIR:$PATH" \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/explicit-mbx-target" \
    "$MBX_BIN" check --offline --manifest-path "$project/Cargo.toml"
  assert_success
  refute_output --partial "RUSTC_WRAPPER is already set"
}

@test "plain Cargo restores a second checkout through a full mbx session" {
  local first="$BATS_TEST_TMPDIR/first-checkout"
  local second="$BATS_TEST_TMPDIR/second-checkout"
  local compiler="$BATS_TEST_TMPDIR/counting-rustc"
  local rustc_log="$BATS_TEST_TMPDIR/rustc.log"
  local real_rustc
  real_rustc="$(command -v rustc)"
  write_project "$first"
  write_project "$second"
  mkdir -p "$first/.cargo"
  cat >"$first/.cargo/config.toml" <<'EOF'
[alias]
fixture-build = "build --offline"
EOF
  cat >"$compiler" <<'EOF'
#!/bin/sh
crate_name=0
for arg in "$@"; do
  if [ "$crate_name" = 1 ]; then
    if [ "$arg" = fixture ]; then printf 'compile\n' >>"$MBX_TEST_RUSTC_LOG"; fi
    break
  fi
  if [ "$arg" = "--crate-name" ]; then crate_name=1; fi
done
exec "$MBX_TEST_REAL_RUSTC" "$@"
EOF
  chmod +x "$compiler"
  "$MBX_BIN" setup >/dev/null

  run env PATH="$MBX_SHIM_DIR:$PATH" CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/first-target" \
    MBX_TEST_REAL_RUSTC="$real_rustc" MBX_TEST_RUSTC_LOG="$rustc_log" RUSTC="$compiler" \
    sh -c 'cd "$1" && cargo fixture-build' sh "$first"
  assert_success
  run env PATH="$MBX_SHIM_DIR:$PATH" CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/second-target" \
    MBX_TEST_REAL_RUSTC="$real_rustc" MBX_TEST_RUSTC_LOG="$rustc_log" RUSTC="$compiler" \
    cargo build --offline --manifest-path "$second/Cargo.toml"
  assert_success
  run grep -c '^compile$' "$rustc_log"
  assert_success
  assert_output "1"
}

@test "plain Cargo restores rustdoc output through the installed shim" {
  local first="$BATS_TEST_TMPDIR/first-doc-checkout"
  local second="$BATS_TEST_TMPDIR/second-doc-checkout"
  local cold_report="$BATS_TEST_TMPDIR/cold-doc.json"
  local warm_report="$BATS_TEST_TMPDIR/warm-doc.json"
  write_project "$first"
  write_project "$second"
  "$MBX_BIN" setup >/dev/null

  run env PATH="$MBX_SHIM_DIR:$PATH" \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/first-doc-target" \
    MBX_STATS_REPORT="$cold_report" \
    cargo doc --offline --no-deps --manifest-path "$first/Cargo.toml"
  assert_success
  run grep -E '"misses"[[:space:]]*:[[:space:]]*[1-9]' "$cold_report"
  assert_success

  run env PATH="$MBX_SHIM_DIR:$PATH" \
    CARGO_TARGET_DIR="$BATS_TEST_TMPDIR/second-doc-target" \
    MBX_STATS_REPORT="$warm_report" \
    cargo doc --offline --no-deps --manifest-path "$second/Cargo.toml"
  assert_success
  run grep -E '"hits"[[:space:]]*:[[:space:]]*[1-9]' "$warm_report"
  assert_success
  assert_file_exists "$BATS_TEST_TMPDIR/second-doc-target/doc/fixture/index.html"
}
