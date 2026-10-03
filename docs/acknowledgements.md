---
description: See which projects mbx builds on or draws ideas from, including Cargo, sccache, kache, and cargo-pretty.
---
# Acknowledgements

mbx depends on Cargo and builds on ideas established by earlier compiler
caches. In particular, kache directly inspired the project's design. mbx's
Cargo display includes code from cargo-pretty and norimel.

## Cargo

[Cargo](https://github.com/rust-lang/cargo) resolves dependencies, plans
builds, and orchestrates rustc. Its `RUSTC_WRAPPER` integration lets mbx and
other Rust compiler caches wrap compiler invocations.

## sccache

[sccache](https://github.com/mozilla/sccache) predates mbx and established
compiler caching in the Rust ecosystem. It reuses compiler work locally and
through remote storage, and supports a broader set of compilers and use cases
than mbx.

See the [comparison with sccache](/compared#sccache) for where mbx makes
different tradeoffs.

## kache

[kache](https://github.com/kunobi-ninja/kache) predates mbx and directly
inspired its design. It combines a content-addressed `RUSTC_WRAPPER` cache
with C and C++ compiler shims, remote storage, and executable caching.

mbx and kache do not share code. Both aim to make compiled work reusable across
checkouts; they differ in process lifecycle, storage management, and remote
policy. The [comparison with kache](/compared#kache) explains where mbx took a
different direction and where kache may be the better fit.

## cargo-pretty

[cargo-pretty](https://github.com/romancitodev/cargo-pretty) by
[romancitodev](https://github.com/romancitodev) provides the base for mbx's
Cargo display: live crate rows, completed rows that dim as newer ones finish,
per-crate timers, and browsable warnings. mbx adds cache statistics and a
progress bar colored by hits, misses, and bypasses while retaining Cargo's
native run and test execution.

mbx draws the display with norimel, a styled-text block library from
romancitodev's [nobubbles](https://github.com/romancitodev/nobubbles) project.
mbx vendors norimel as a private module, so it ships inside mbx rather than as
a registry dependency. Both the adapted cargo-pretty code and the vendored
norimel keep their upstream MIT licenses, and the repository's
[`NOTICE`](https://github.com/jdx/mr-boxington/blob/main/NOTICE) file records
each upstream revision.
