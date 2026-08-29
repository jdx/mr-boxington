# Acknowledgements

mbx relies on Cargo and follows earlier compiler-cache work in sccache and
kache.

## Cargo

[Cargo](https://github.com/rust-lang/cargo) resolves dependencies, plans
builds, and orchestrates rustc. Its `RUSTC_WRAPPER` integration is what allows
mbx and other Rust compiler caches to wrap compiler invocations.

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

The projects do not share code, and they make different tradeoffs. The
[comparison with kache](/compared#kache) explains where mbx took a different
direction and where kache may be the better fit.
