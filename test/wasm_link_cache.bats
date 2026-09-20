#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  local developer_home="$HOME"
  export RUSTUP_HOME="${RUSTUP_HOME:-$developer_home/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-$developer_home/.cargo}"
  _common_setup

  unset CARGO_TARGET_DIR MBX_INCREMENTAL CARGO_INCREMENTAL CI
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"

  export PROJECT="$BATS_TEST_TMPDIR/project"
  mkdir -p "$PROJECT/src"
  cat >"$PROJECT/Cargo.toml" <<'EOF'
[package]
name = "wasm-fixture"
version = "0.1.0"
edition = "2021"
EOF
  echo 'fn main() {}' >"$PROJECT/src/main.rs"

  run cargo generate-lockfile --offline --manifest-path "$PROJECT/Cargo.toml"
  assert_success
}

@test "a linked wasm binary restores into a distinct target directory" {
  local first_target="$BATS_TEST_TMPDIR/first-target"
  local second_target="$BATS_TEST_TMPDIR/second-target"
  local cold_report="$BATS_TEST_TMPDIR/cold.json"
  local warm_report="$BATS_TEST_TMPDIR/warm.json"
  local relative_output="wasm32-unknown-unknown/debug/wasm-fixture.wasm"

  run env \
    CARGO_TARGET_DIR="$first_target" \
    MBX_STATS_REPORT="$cold_report" \
    "$MBX_BIN" build --offline --target wasm32-unknown-unknown \
    --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  assert_file_exists "$first_target/$relative_output"
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$cold_report"
  assert_success

  run env \
    CARGO_TARGET_DIR="$second_target" \
    MBX_STATS_REPORT="$warm_report" \
    "$MBX_BIN" build --offline --target wasm32-unknown-unknown \
    --manifest-path "$PROJECT/Cargo.toml"
  assert_success
  assert_file_exists "$second_target/$relative_output"
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$warm_report"
  assert_success

  run cmp "$first_target/$relative_output" "$second_target/$relative_output"
  assert_success
}

@test "cargo tests build a separate wasm guest in its own target directory" {
  # Use independent projects for the plain Cargo control and mbx. Never build
  # the guest directly: doing so would hide the inherited-target regression.
  for disabled in 1 0; do
    local parent="$BATS_TEST_TMPDIR/nested-$disabled"
    mkdir -p "$parent/src" "$parent/guest/src"
    cp "$PROJECT/Cargo.toml" "$parent/Cargo.toml"
    cat >"$parent/guest/Cargo.toml" <<'EOF'
[package]
name = "nested-guest"
version = "0.0.0"
edition = "2021"
[workspace]
EOF
    echo 'fn main() {}' >"$parent/guest/src/main.rs"
    cat >"$parent/src/main.rs" <<'EOF'
fn main() {}
#[test]
fn builds_guest() {
    let guest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("guest");
    let artifact = guest.join("target/wasm32-unknown-unknown/debug/nested-guest.wasm");
    assert!(!artifact.exists(), "a previous guest build would mask the regression");
    let output = std::process::Command::new("cargo")
        .args(["build", "--offline", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(guest.join("Cargo.toml")).current_dir(&guest).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let wasm = std::fs::read(&artifact).unwrap_or_else(|e| panic!("{}: {e}; {output:?}", artifact.display()));
    assert_eq!(&wasm[..4], b"\0asm");
    std::fs::remove_file(artifact).unwrap();
    assert!(std::env::var_os("CARGO_TARGET_DIR").is_none());
}
EOF
    for pass in cold warm; do
      run env MBX_DISABLE="$disabled" MBX_CARGO_SHIM_MODE=1 MBX_LINKER=system \
        "$MBX_BIN" test --offline --manifest-path "$parent/Cargo.toml"
      assert_success
    done
  done
}
