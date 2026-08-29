# Cache C and C++ builds outside Cargo

Use `mbx exec` to cache compiler calls made by make, CMake, and other build
tools:

```sh
mbx exec make -j8
```

For CMake, run the configure step through `mbx exec` too. CMake chooses a
compiler while configuring and reuses that choice when it builds.

```sh
mbx exec cmake -S . -B build
mbx exec cmake --build build
```

Only the command after `mbx exec` is affected. There is no daemon to start and
nothing is installed globally.

## What `mbx exec` does

While the command runs, mbx puts wrappers for the common Unix compiler names
at the front of `PATH`:

```text
cc  c++  gcc  g++  clang  clang++
```

When the build tool calls one of those names, mbx checks the cache first. On a
hit, it restores the object file. On a miss, it runs the compiler that would
normally have been found on `PATH` and saves the result.

The wrappers use the same local and remote cache as Cargo builds. The command's
exit status and compiler output are passed through unchanged, and the wrappers
go away from `PATH` when the command finishes.

## CMake and other configured builds

Some build systems save the compiler's absolute path during configuration.
CMake writes it to `CMakeCache.txt`; autoconf may write it into generated
makefiles. If configuration happens outside `mbx exec`, the saved path points
straight to the compiler and later `mbx exec` builds cannot intercept it.

Configure once through `mbx exec` so the build system records mbx's wrapper.
That path remains valid across later commands. You should still use `mbx exec`
for each build you want cached:

```sh
# Configure once.
mbx exec cmake -S . -B build

# Build as often as needed.
mbx exec cmake --build build
```

Running `cmake --build build` without `mbx exec` still works, but it calls the
real compiler without using the cache.

## What gets cached

mbx caches ordinary gcc- and clang-style compile commands that compile one C
or C++ source file into an object with `-c`. It does not cache links,
multi-source compiler calls, or commands whose behavior it cannot model
safely. Those commands still run normally; the session summary reports why
they bypassed the cache.

`mbx exec` only intercepts the six unversioned compiler names listed above. It
leaves commands such as `gcc-13`, absolute compiler paths, and explicitly
selected cross-compilers alone.

See [limits](/limits#c-and-c-caching-covers-the-host-compiles-mbx-drives) for
the complete list of supported and bypassed invocations.

## Sharing results across checkouts

mbx removes the checkout's absolute path from compilation keys, so equivalent
checkouts can share cached objects. It normally treats the enclosing Git or
Jujutsu checkout as the project root. Outside a checkout, it uses the working
directory. Override that choice with `--project-root`:

```sh
mbx exec --project-root /path/to/project make -j8
```

To find the same project in another checkout, mbx uses the `Cargo.lock` digest
when one exists, then the Git or Jujutsu `origin` URL, and finally the directory
name. If only the directory name is available, matching directory names are
required for cross-checkout hits.

## Disabling the cache

Set `MBX_CC=0` to run the command without C or C++ caching:

```sh
MBX_CC=0 mbx exec make
```

Because C and C++ compilation is the only work cached by `mbx exec`, this makes
the command equivalent to running it directly. Production release builds may
use the local cache, but should not use a remote cache; this prevents remote
cache poisoning from affecting published artifacts.
