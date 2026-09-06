setup() {
  load 'test_helper/common_setup'
  _common_setup
}

teardown() {
  if [[ -n "${build_pid:-}" ]]; then
    touch "$BATS_TEST_TMPDIR/proceed"
    wait "$build_pid" || true
  fi
}

@test "a different cache root cannot move an active Cargo diagnostics directory" {
  cargo init --lib --name diagnostic_fixture --vcs none project
  cd project
  cat >src/lib.rs <<'RS'
pub fn example() { let unused = 1; }
RS
  cat >build.rs <<'RS'
fn main() {
    std::fs::write("../ready", "").unwrap();
    for _ in 0..600 {
        if std::path::Path::new("../proceed").exists() { return; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("test did not release the build");
}
RS
  "$MBX_BIN" check >"$BATS_TEST_TMPDIR/build.log" 2>&1 &
  build_pid=$!
  for _ in $(seq 1 300); do
    [[ -f ../ready ]] && break
    sleep 0.1
  done
  assert_file_exists ../ready
  local original
  original=$(readlink target)
  # A pre-existing destination makes relocation fall back to retiring the old
  # tree, just as a move across filesystems does on the appliance.
  local destination="$BATS_TEST_TMPDIR/other-cache/targets/v1/${original##*/}"
  mkdir -p "$destination"
  touch "$destination/existing-output"

  run env MBX_CACHE_DIR="$BATS_TEST_TMPDIR/other-cache" "$MBX_BIN" no-such-cargo-command
  assert_failure
  local after_inspection
  after_inspection=$(readlink target)

  touch ../proceed
  local status=0
  wait "$build_pid" || status=$?
  build_pid=
  cat "$BATS_TEST_TMPDIR/build.log"
  assert_equal "$status" 0
  assert_equal "$after_inspection" "$original"
  run cat "$BATS_TEST_TMPDIR/build.log"
  assert_output --partial 'unused variable'
  refute_output --partial 'failed to create file'
}
