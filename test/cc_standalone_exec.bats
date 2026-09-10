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

  # One that separates exec's own options from the command belongs to exec,
  # and running it as the program would fail.
  run "$MBX_BIN" exec -- /bin/echo hi
  assert_success
  assert_line 'hi'

  run "$MBX_BIN" exec --project-root "$project" -- /bin/echo ok
  assert_success
  assert_line 'ok'

  # An option exec knows, named after the command, belongs to the command.
  run "$MBX_BIN" exec /bin/echo --project-root x
  assert_success
  assert_line '--project-root x'
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

@test "gdb and full debug objects restore across checkouts and invalidate on source changes" {
  local flag
  for flag in -ggdb -gfull; do
    # GCC does not support Apple's -gfull; exercise it wherever the driver does.
    echo 'int probe(void) { return 0; }' >"$BATS_TEST_TMPDIR/probe.c"
    if ! cc "$flag" -c "$BATS_TEST_TMPDIR/probe.c" -o "$BATS_TEST_TMPDIR/probe.o" 2>/dev/null; then
      continue
    fi
    local first="$BATS_TEST_TMPDIR/first-$flag"
    local second="$BATS_TEST_TMPDIR/second-$flag"
    local report="$BATS_TEST_TMPDIR/report-$flag.json"
    write_project "$first"
    write_project "$second"
    (cd "$first" && "$MBX_BIN" exec make "CFLAGS=$flag -Iinclude" hello)
    (cd "$second" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec make "CFLAGS=$flag -Iinclude" hello)
    run grep -E '"hits"[[:space:]]*:[[:space:]]*2' "$report"
    assert_success
    run cmp "$first/hello.o" "$second/hello.o"
    assert_success
    "$second/hello"

    # A sampled fresh compiler run must reproduce the cached debug objects.
    rm "$second/hello.o" "$second/main.o"
    (cd "$second" && MBX_VERIFY=0 MBX_VERIFY_SAMPLE_RATE=100 MBX_STATS_REPORT="$report" "$MBX_BIN" exec make "CFLAGS=$flag -Iinclude" hello)
    run grep -E '"verifications"[[:space:]]*:[[:space:]]*2' "$report"
    assert_success
    run grep -E '"divergences"[[:space:]]*:[[:space:]]*0' "$report"
    assert_success
    "$second/hello"

    echo 'int hello_value(void) { return 9; }' >"$second/src/hello.c"
    rm "$second/hello.o" "$second/hello"
    (cd "$second" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec make "CFLAGS=$flag -Iinclude" hello)
    run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$report"
    assert_success
    run "$second/hello"
    assert_failure
  done
}

@test "named preprocessor output restores exact text and tracks changed headers" {
  local project="$BATS_TEST_TMPDIR/preprocess"
  local report="$BATS_TEST_TMPDIR/preprocess.json"
  mkdir -p "$project/include"
  echo '#define VALUE 7' >"$project/include/value.h"
  cat >"$project/main.c" <<'SOURCE'
#include "value.h"
const char *source = __FILE__;
int value = VALUE;
SOURCE
  # Absolute input/header paths deliberately exercise line markers that must
  # retain their exact spelling in the cached text.
  local source="$project/main.c"
  local include="$project/include"
  (cd "$project" && cc -E "$source" -I"$include" -o reference.i)
  (cd "$project" && "$MBX_BIN" exec cc -E "$source" -I"$include" -o result.i)
  run cmp "$project/reference.i" "$project/result.i"
  assert_success
  rm "$project/result.i"
  (cd "$project" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -E "$source" -I"$include" -o result.i)
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$report"
  assert_success
  run cmp "$project/reference.i" "$project/result.i"
  assert_success

  # Changing only a header must invalidate a warm prediction.
  echo '#define VALUE 9' >"$project/include/value.h"
  (cd "$project" && cc -E "$source" -I"$include" -o reference.i)
  rm "$project/result.i"
  (cd "$project" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -E "$source" -I"$include" -o result.i)
  run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$report"
  assert_success
  run cmp "$project/reference.i" "$project/result.i"
  assert_success

  # A caller's explicit dependency file is regenerated on a hit. Its target
  # must match the real driver (GCC and Clang differ under -E).
  (cd "$project" && cc -E "$source" -I"$include" -o deps.i -MD -MF reference.d)
  local expected_target
  expected_target=$(sed -n '1s/:.*//p' "$project/reference.d")
  rm "$project/deps.i"
  (cd "$project" && "$MBX_BIN" exec cc -E "$source" -I"$include" -o deps.i -MD -MF deps.d)
  rm "$project/deps.i" "$project/deps.d"
  (cd "$project" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -E "$source" -I"$include" -o deps.i -MD -MF deps.d)
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$report"
  assert_success
  run grep 'value.h' "$project/deps.d"
  assert_success
  run grep -F "$expected_target:" "$project/deps.d"
  assert_success
  run cmp "$project/reference.i" "$project/deps.i"
  assert_success

  # Stdout still goes through the real compiler transparently.
  (cd "$project" && cc -E "$source" -I"$include" >stdout.reference)
  (cd "$project" && "$MBX_BIN" exec cc -E "$source" -I"$include" >stdout.actual)
  run cmp "$project/stdout.reference" "$project/stdout.actual"
  assert_success
}

@test "C++ preprocessing without line markers restores from the cache" {
  command -v c++ >/dev/null || skip "no C++ compiler is available"
  local project="$BATS_TEST_TMPDIR/preprocess-cxx"
  local report="$BATS_TEST_TMPDIR/preprocess-cxx.json"
  mkdir -p "$project/src"
  cat >"$project/src/main.cpp" <<'SOURCE'
#ifdef __cplusplus
template <typename T> T twice(T value) { return value + value; }
#else
#error expected C++ preprocessing
#endif
SOURCE
  (cd "$project" && c++ -E -P src/main.cpp -o expected.ii)
  (cd "$project" && "$MBX_BIN" exec c++ -E -P src/main.cpp -o result.ii)
  rm "$project/result.ii"
  (cd "$project" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec c++ -E -P src/main.cpp -o result.ii)
  run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$report"
  assert_success
  run cmp "$project/expected.ii" "$project/result.ii"
  assert_success
}

@test "a divergent C verification preserves its cached result and writes caller dependencies" {
  local project="$BATS_TEST_TMPDIR/verify-stream"
  local compiler="$BATS_TEST_TMPDIR/compiler"
  local real_cc
  real_cc="$(command -v cc)"
  write_project "$project"
  mkdir -p "$compiler"
  cat >"$compiler/cc" <<'SCRIPT'
#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = -c ]; then
    echo "$MBX_TEST_CC_MESSAGE" >&2
    break
  fi
done
exec "$MBX_TEST_REAL_CC" "$@"
SCRIPT
  chmod +x "$compiler/cc"
  export MBX_TEST_REAL_CC="$real_cc"
  export PATH="$compiler:$PATH"
  cd "$project"
  run env MBX_TEST_CC_MESSAGE=cached "$MBX_BIN" exec make 'CFLAGS=-O2 -Iinclude -MMD -MF hello.d' hello.o
  assert_success
  rm hello.o hello.d
  run env MBX_CACHE_EXPORT_GROUP=verify-audit MBX_TEST_CC_MESSAGE=verified MBX_VERIFY=1 "$MBX_BIN" exec make 'CFLAGS=-O2 -Iinclude -MMD -MF hello.d' hello.o
  assert_success
  assert_output --partial 'shadow verification diverged'
  refute_output --partial 'result was not published'
  run "$MBX_BIN" cache export --group verify-audit "$BATS_TEST_TMPDIR/audit.tar"
  assert_success
  assert_output --partial "exported 1 actions"
  assert_file_exists "$BATS_TEST_TMPDIR/audit.tar"
  assert_file_exists hello.d
  rm hello.o hello.d
  run env MBX_TEST_CC_MESSAGE=unused "$MBX_BIN" exec make 'CFLAGS=-O2 -Iinclude -MMD -MF hello.d' hello.o
  assert_success
  assert_line 'cached'
  refute_line 'unused'
  refute_line 'verified'
  assert_file_exists hello.d
}

@test "objects retaining absolute source paths cache without leaking another checkout's FILE string" {
  local first="$BATS_TEST_TMPDIR/first"
  local second="$BATS_TEST_TMPDIR/second"
  local project report
  for project in "$first" "$second"; do
    mkdir -p "$project/out"
    echo 'version = 4' >"$project/Cargo.lock"
    cat >"$project/source.c" <<'SOURCE'
extern int puts(const char *);
int main(void) { puts(__FILE__); return 0; }
SOURCE
  done

  for project in "$first" "$second"; do
    report="$project-cold.json"
    (cd "$project" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -g -c "$project/source.c" -o out/source.o)
    # Identical source and normalized arguments must not reuse the other path.
    run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$report"
    assert_success
    rm "$project/out/source.o"
    report="$project-warm.json"
    (cd "$project" && MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -g -c "$project/source.c" -o out/source.o)
    run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$report"
    assert_success
    cc "$project/out/source.o" -o "$project-show-path"
    run "$project-show-path"
    assert_success
    assert_output "$project/source.c"
  done

  # Returning to the first path still finds its action after the shared
  # prediction has been refreshed by the second checkout.
  rm "$first/out/source.o"
  (cd "$first" && MBX_VERIFY=1 MBX_STATS_REPORT="$first-verify.json" "$MBX_BIN" exec cc -g -c "$first/source.c" -o out/source.o)
  run grep -E '"verifications"[[:space:]]*:[[:space:]]*1' "$first-verify.json"
  assert_success
  run grep -E '"divergences"[[:space:]]*:[[:space:]]*0' "$first-verify.json"
  assert_success
}

@test "literal paths under mapped roots outside the working directory stay isolated" {
  local project="$BATS_TEST_TMPDIR/project"
  local cargo_home report
  mkdir -p "$project/out"
  echo 'version = 4' >"$project/Cargo.lock"
  # CARGO_HOME roots are modeled independently of the working directory and
  # are not necessarily included in the injected debug-prefix maps.
  for cargo_home in "$BATS_TEST_TMPDIR/home-one" "$BATS_TEST_TMPDIR/home-two"; do
    mkdir -p "$cargo_home/registry"
    cat >"$cargo_home/registry/generated.c" <<'SOURCE'
extern int puts(const char *);
int main(void) { puts(__FILE__); return 0; }
SOURCE
  done
  for cargo_home in "$BATS_TEST_TMPDIR/home-one" "$BATS_TEST_TMPDIR/home-two"; do
    rm -f "$project/out/source.o"
    report="$cargo_home-cold.json"
    (cd "$project" && CARGO_HOME="$cargo_home" MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -g -c "$cargo_home/registry/generated.c" -o out/source.o)
    run grep -E '"hits"[[:space:]]*:[[:space:]]*0' "$report"
    assert_success
    rm "$project/out/source.o"
    report="$cargo_home-warm.json"
    (cd "$project" && CARGO_HOME="$cargo_home" MBX_STATS_REPORT="$report" "$MBX_BIN" exec cc -g -c "$cargo_home/registry/generated.c" -o out/source.o)
    run grep -E '"hits"[[:space:]]*:[[:space:]]*1' "$report"
    assert_success
    cc "$project/out/source.o" -o "$BATS_TEST_TMPDIR/show-path"
    run "$BATS_TEST_TMPDIR/show-path"
    assert_success
    assert_output "$cargo_home/registry/generated.c"
  done
}
