---
description: Choose a Rust build cache based on how you work, and see how mbx compares with kache, sccache, and CI caching.
---
# How mbx compares

Cargo already skips work when a project's build outputs are up to date. But
start a second copy of the project, or build on a fresh CI runner, and you may
compile the same dependencies again. Build caches save that work so another
build can reuse it.

mbx, kache, and sccache all help with this. **mbx also manages the Cargo build
around the cache**, including where build files live and when old ones are
cleaned up.

| What you want | Where to start |
| --- | --- |
| Reuse builds and clean up build files from multiple copies of a Rust project | [mbx](#what-mbx-adds) |
| Add a build cache to your existing Cargo setup | [kache](#kache) |
| Cache work from a wider range of compilers, or compile on other machines | [sccache](#sccache) |
| Save and restore build files between GitHub Actions runs | [CI caches](#tarball-ci-caches) |
| Speed up repeated edits in the same project directory | [Incremental compilation](#cargos-incremental-compilation) |

## What mbx adds

Suppose you keep several copies of a project to work on different branches.
A compiler cache helps them reuse work, but each copy can still leave a large
`target/` directory behind. That's the directory where Cargo puts build files.

mbx handles both parts:

- **Reuse completed work.** A second copy of a project can restore matching
  compilations instead of running them again.
- **Clean up old build files.** mbx manages `target/` directories by default and
  reclaims space when a copy of the project is deleted or storage limits are
  reached. See [Managed targets](/managed-targets).
- **Run builds together.** When several builds run at once, mbx coordinates
  their CPU and memory use. See [Parallel builds](/scheduling).
- **Explain the build.** Progress and [cache results](/cache-results) appear
  together, so you can see what was reused and what still needed compiling.

You can start with `mbx build` in place of `cargo build`. After
[setup](/setup), you can keep typing `cargo build` and have it run through mbx.
No cache server is needed to get started.

## kache

[kache](https://github.com/kunobi-ninja/kache) directly inspired mbx. Both help
reuse builds across copies of a project, save disk space, and share cached
work with other machines. Both also coordinate simultaneous compilations and
provide tools to explain cache hits and misses.

**The main difference is that mbx wraps Cargo, while kache wraps the
compiler.** Cargo organizes a Rust build and calls the compiler to do the work.
kache steps in when Cargo calls the compiler. mbx starts Cargo too, which lets
it set up the build and manage its build directories as well as cache the
compilation work.

With kache, you run `kache init` once and continue using `cargo build`. With
mbx, you use `mbx build` or enable the Cargo setup described above. Choose
kache if you want a cache in your existing build setup; choose mbx if you also
want its Cargo setup and build-directory cleanup.

kache also works with C/C++ and CUDA builds. See its
[current feature list](https://github.com/kunobi-ninja/kache) for supported
workloads and its [architecture guide](https://ninja.kunobi.com/docs/kache/how-it-works/architecture)
for the implementation details.
This comparison was checked against its documentation on September 25, 2026.

## sccache

[sccache](https://github.com/mozilla/sccache) is an established compiler cache
for Rust and several other languages and compilers. Like kache, it works when
the build calls the compiler.

It is also worth considering if you want **other machines to do the
compiling**. This is called distributed compilation. Sharing a remote cache
helps when work has already been built; distributed compilation helps run new
work elsewhere.

If you already use sccache or kache, follow the
[migration guide](/cookbook/migrate#from-sccache) when switching to mbx.
Leaving another Rust compiler cache enabled can cause mbx to defer to it.

## CI caches {#tarball-ci-caches}

[`actions/cache`](https://github.com/actions/cache) saves directories at the
end of a CI job and restores them in a later job.
[`Swatinem/rust-cache`](https://github.com/Swatinem/rust-cache) does this with
Rust-specific defaults. This is useful when the next job can reuse much of
the previous job's build directory.

A compiler cache such as mbx saves individual compilation results. That lets
a build reuse matching pieces even when it needs a different set of outputs.

The [mbx GitHub Action](/github-action) supports both approaches. By default,
it saves and restores an archive of Cargo files, then runs builds through mbx.
It also offers an `objects` mode for sharing individual cached results.
The action guide explains when to choose each; avoid configuring two actions
to save the same data.

<span id="cargo-s-incremental-compilation"></span>

## Cargo's incremental compilation {#cargos-incremental-compilation}

Incremental compilation helps with a different situation: you edit a little
code and rebuild in the same project directory. Rust can reuse parts of the
previous compilation instead of starting that crate over.

mbx combines shared caching with incremental work kept privately for the
crates you edit. You can leave the defaults in place for everyday development;
[Incremental builds](/incremental) explains the available controls.

## Which is fastest?

It depends on your project and how you build it. A second build of unchanged
code, a small source edit, and a fresh CI job benefit from different kinds of
reuse. Our [benchmarks](/benchmarks) show the tested workloads and tool versions.
Use them as a starting point, then compare the tools on your own builds.

To try mbx, follow [Get started](/getting-started).
