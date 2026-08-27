#!/usr/bin/env bats

setup() {
  load "test_helper/common_setup"
  _common_setup

  # An inherited compiler choice or policy would make the fixture prove the
  # environment rather than the shims.
  unset MBX_CC CC CXX HOST_CC HOST_CXX TARGET_CC TARGET_CXX
  unset MBX_REAL_CC MBX_REAL_CXX MBX_CC_SHIM_COMPILERS CI
  export MBX_CACHE_DIR="$BATS_TEST_TMPDIR/store"

  if ! cc -v >/dev/null 2>&1; then
    skip "no C compiler is available"
  fi
  if ! command -v make >/dev/null 2>&1; then
    skip "make is not available"
  fi
}

# Lay the fixture down at $1: one C file compiled by make's default rules,
# plus a link step the cache must pass through untouched.
write_project() {
  mkdir -p "$1/src" "$1/include"
  echo 'int hello_value(void);' >"$1/include/hello.h"
  cat >"$1/src/hello.c" <<'EOF'
#include "hello.h"
int hello_value(void) { return 7; }
EOF
  cat >"$1/src/main.c" <<'EOF'
#include "hello.h"
int main(void) { return hello_value() == 7 ? 0 : 1; }
EOF
  cat >"$1/Makefile" <<'EOF'
CFLAGS = -O2 -Iinclude

hello: hello.o main.o
	$(CC) -o $@ hello.o main.o

hello.o: src/hello.c include/hello.h
	$(CC) $(CFLAGS) -c -o $@ src/hello.c

main.o: src/main.c include/hello.h
	$(CC) $(CFLAGS) -c -o $@ src/main.c
EOF
  # The identity a second checkout must reproduce: worktrees share a manifest
  # through the lockfile digest when one exists.
  echo 'version = 4' >"$1/Cargo.lock"
}

@test "a make build's C objects restore into a second checkout" {
  local first="$BATS_TEST_TMPDIR/first"
  local second="$BATS_TEST_TMPDIR/second"
  local cold_report="$BATS_TEST_TMPDIR/cold.json"
  local warm_report="$BATS_TEST_TMPDIR/warm.json"
  write_project "$first"
  write_project "$second"

  (cd "$first" && MBX_STATS_REPORT="$cold_report" "$MBX_BIN" exec make hello)
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$cold_report"
  assert_success
  assert_file_exists "$first/hello.o"
  "$first/hello"

  # A distinct directory shares nothing but the store, so a hit here proves
  # the key survived the path change.
  (cd "$second" && MBX_STATS_REPORT="$warm_report" "$MBX_BIN" exec make hello)
  run grep -E '"hits"[[:space:]]*:[[:space:]]*2' "$warm_report"
  assert_success
  run cmp "$first/hello.o" "$second/hello.o"
  assert_success
  run cmp "$first/main.o" "$second/main.o"
  assert_success
  # The link is not cached; it must still have produced a working binary.
  "$second/hello"
}

@test "a double dash reaches the command" {
  local project="$BATS_TEST_TMPDIR/dashes"
  write_project "$project"
  cd "$project"

  # `cmake --build build -- -j8` passes -j8 to the underlying tool, so a
  # delimiter the argument parser swallowed would change what ran.
  run "$MBX_BIN" exec /bin/echo a -- b
  assert_success
  assert_line 'a -- b'
}

@test "a configured build directory outlives the session that configured it" {
  if ! command -v cmake >/dev/null 2>&1; then
    skip "cmake is not available"
  fi
  local project="$BATS_TEST_TMPDIR/configured"
  mkdir -p "$project"
  cat >"$project/CMakeLists.txt" <<'EOF'
cmake_minimum_required(VERSION 3.20)
project(probe C)
add_executable(probe main.c)
EOF
  echo 'int main(void) { return 0; }' >"$project/main.c"
  cd "$project"

  run "$MBX_BIN" exec cmake -S . -B build
  assert_success

  # CMake records the compiler it resolved by absolute path. A shim directory
  # belonging to the session would leave this naming one that is already gone.
  local recorded
  recorded="$(grep -E '^CMAKE_C_COMPILER:' build/CMakeCache.txt | cut -d= -f2)"
  assert_file_exists "$recorded"

  # And that recorded path still builds when nothing runs it under mbx.
  touch main.c
  run cmake --build build
  assert_success
  assert_file_exists build/probe
}

@test "a failing command's exit code passes through" {
  local project="$BATS_TEST_TMPDIR/failing"
  write_project "$project"
  echo 'this is not C' >"$project/src/hello.c"

  (cd "$project" && ! "$MBX_BIN" exec make hello)
}

@test "MBX_CC=0 runs the command plainly" {
  local project="$BATS_TEST_TMPDIR/plain"
  write_project "$project"

  (cd "$project" && MBX_CC=0 "$MBX_BIN" exec make hello)
  assert_file_exists "$project/hello.o"
  "$project/hello"
  # Nothing was cached, so no store was ever created.
  assert_not_exists "$MBX_CACHE_DIR"
}
