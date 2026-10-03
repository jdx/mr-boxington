---
description: Cache C and C++ compiler calls from make, CMake, and other build tools with mbx exec.
---
# Cache C and C++ builds outside Cargo

Use `mbx exec` to cache compiler calls made by make, CMake, and other build
tools:

```sh
mbx exec make -j8
```

For CMake, run the configure step through `mbx exec` too. That is where mbx
sets up CMake's [compiler launchers](#cmake-builds), which later builds reuse.

```sh
mbx exec cmake -S . -B build
mbx exec cmake --build build
```

Only the command after `mbx exec` uses mbx's compiler shims. There is no
daemon or global compiler setup. For C and C++ compiled by Cargo build scripts,
use `mbx build`; that integration is
[enabled by default](/configuration#build-script-c-and-c).

## What `mbx exec` does

While the command runs, mbx puts shims for the common compiler names at the
front of `PATH`:

```text
cc  c++  gcc  g++  clang  clang++
```

On Windows it wraps `cl.exe` instead.

When the build tool calls one of those names, mbx checks the cache first. On a
hit, it restores the object file. On a miss, it runs the compiler that would
normally have been found on `PATH` and saves the result.

The shims use the same local and remote cache as Cargo builds. mbx passes
the command's exit status and compiler output through unchanged, and the
shims stay on `PATH` only while the command runs.

## CMake builds

When `mbx exec` runs a CMake configure, it also sets
[`CMAKE_C_COMPILER_LAUNCHER`](https://cmake.org/cmake/help/latest/prop_tgt/LANG_COMPILER_LAUNCHER.html)
and `CMAKE_CXX_COMPILER_LAUNCHER` in the build directory's CMake cache. CMake
then runs every C and C++ compilation through mbx, whichever compiler it uses:

- **An existing build directory.** A directory configured earlier without mbx
  keeps its compiler and options. Reconfiguring it through `mbx exec` only adds
  the launchers, so CMake does not start over.
- **A compiler the build names itself.** If the build picks its compiler with
  `-DCMAKE_C_COMPILER=/opt/gcc-13/bin/gcc`, `CC=gcc-13`, or a toolchain file,
  the shims on `PATH` never see it, but mbx still caches its compilations.

```sh
# configured once without mbx
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release

# add the launchers, keeping the existing configuration
mbx exec cmake -S . -B build

# build as often as needed
mbx exec cmake --build build
```

mbx leaves a launcher alone if you choose one yourself, either with
`-DCMAKE_C_COMPILER_LAUNCHER=...` or by exporting `CMAKE_C_COMPILER_LAUNCHER`.
mbx replaces a launcher left by an older mbx.

Running `cmake --build build` without `mbx exec` still works, but it calls the
real compiler without using the cache.

The launchers work with the Makefile and Ninja generators. CMake ignores them
with the Visual Studio and Xcode generators. CMake has no launcher for
assembly, so mbx caches `.S` files only when CMake found its compiler through
the shims on `PATH`. That happens on a fresh configure under `mbx exec`.

mbx sets up the launchers only when the command after `mbx exec` is `cmake`
itself. A script that runs CMake for you still gets the `PATH` shims
described in [What `mbx exec` does](#what-mbx-exec-does).

## Other configured builds

Some build systems save the compiler's absolute path during configuration. For
example, Autoconf may write it into generated makefiles. If you configure
outside `mbx exec`, the saved path points straight to the compiler, and later
`mbx exec` builds cannot intercept it. Configure through `mbx exec` so the
build system records mbx's shim instead:

```sh
mbx exec ./configure
mbx exec make -j8
```

The recorded shim stays valid across later commands. Outside `mbx exec` it
runs the real compiler without using the cache.

## What gets cached

mbx caches ordinary gcc-, clang-, and MSVC-style compiler calls that compile
one C or C++ source file into an object. It also caches GCC/Clang preprocessing
to a file (`-E ... -o file`), using checkout-specific keys to preserve literal
paths in the output. It does not cache links, multi-source compiler calls, or
commands whose behavior it cannot model safely. Those commands still run
normally. The build summary counts them as bypasses, and `MBX_SUMMARY=full`
reports the grouped reasons.

Outside CMake, `mbx exec` intercepts only the unversioned compiler names listed
in [What `mbx exec` does](#what-mbx-exec-does). It leaves commands such as
`gcc-13`, absolute compiler paths, and explicitly selected cross-compilers
alone.

For the complete list of supported and bypassed compiler calls, see the
[C and C++ limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives).

## Cross-checkout sharing {#sharing-results-across-checkouts}

mbx removes the checkout's absolute path from compilation keys, so equivalent
checkouts can share cached objects. The project root is normally the enclosing
Git, Jujutsu, Mercurial, or Sapling checkout, or the working directory outside
a checkout. To choose the root yourself, pass `--project-root`:

```sh
mbx exec --project-root /path/to/project make -j8
```

To find the same project in another checkout, mbx uses the `Cargo.lock` digest
when one exists, then the repository's origin (the Git or Jujutsu `origin` URL,
or the Mercurial or Sapling default path), and finally the directory name. If
only the directory name is available, cross-checkout hits require matching
directory names.

## Disable the cache {#disabling-the-cache}

Set `MBX_CC=0` to turn off C and C++ caching. C and C++ compilation is the only
work `mbx exec` caches, so mbx warns and runs the command uncached:

```sh
MBX_CC=0 mbx exec make
```

For published artifacts, follow the
[production release policy](/github-action#production-releases).
