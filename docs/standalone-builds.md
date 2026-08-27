# Standalone C and C++ builds

`mbx exec` runs a build command that is not a cargo build — make, CMake, or
any tool that finds its compiler on `PATH` — with mbx's C and C++ cache
around it:

```sh
mbx exec make -j8
mbx exec cmake --build build
```

For the command's duration, a directory holding shims named `cc`, `c++`,
`gcc`, `g++`, `clang`, and `clang++` sits first on `PATH`. Each shim stands in
for the real compiler of the same name, resolved once when the session starts,
so `make`'s default `CC = cc` and an explicit `CC=gcc` both reach a shim that
chains to the compiler the build would have used anyway. The session starts
with the command and exits with it — the same no-daemon lifecycle as a cargo
build, with the same store, remote cache, per-build statistics, and
[CI write policy](/remote-cache#read-and-write-policy).

## Build systems that record their compiler

A configure step resolves the compiler once and writes down where it found
it: CMake stores an absolute `CMAKE_C_COMPILER` in `CMakeCache.txt`, and
autoconf bakes `CC` into the makefiles it generates. What it records is the
shim.

The shim directory therefore lives in the cache directory rather than beside
the session, and stays where it is between commands, so a recorded path keeps
resolving. Configure once under `mbx exec` and every later
`mbx exec cmake --build build` compiles through the cache. A build run
*without* `mbx exec` still works: the shim finds no session to consult, runs
the real compiler, and gets out of the way — that build is simply not cached.

Nothing is added to any `PATH` except the one handed to a single `mbx exec`
command, so a build that never asks for the cache never meets a shim.

## What is cached

The same compilations the
[build-script cache](/configuration#build-script-c-and-c) admits: a plain single-source `-c` compile through a gcc-style or clang-style
driver. Anything else — linking, multi-source invocations, unmodeled flags —
bypasses to the real compiler transparently and is reported with a `cc-`
prefixed reason in the session summary. Outputs land wherever the build's
`-o` pointed; unlike a cargo build there is no managed output directory,
because the build system owns its own.

Only the plain driver names above are shimmed. A build that selects
`gcc-13`, an absolute compiler path, or a cross toolchain chose that compiler
deliberately, and mbx stands aside rather than intercepting it.

## Sharing across checkouts

Compilation keys map the project root to a placeholder, so equivalent
checkouts share objects even when their absolute paths differ. The project
root is the enclosing git checkout (or the working directory outside one);
`--project-root` overrides it.

Warm lookups travel between checkouts through the build's manifest, and the
manifest's identity must therefore name the *project*, not the checkout. It
is derived from, in order: the `Cargo.lock` digest where one exists, the git
`origin` remote URL, and the directory name as a last resort. A project with
none of the first two still caches within each checkout; give worktrees the
same directory name or pass `--project-root` through a stable path to share
across them.

## Turning it off

`MBX_CC=0` disables the C and C++ cache, which is all `mbx exec` caches, so
the command runs plainly. Tag and release builds run plainly always, exactly
as cargo builds do.
