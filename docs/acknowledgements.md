# Acknowledgements

mbx stands on the work of Cargo, sccache, and kache. Each contributed
something different: Cargo is the foundation, sccache established compiler
caching in the Rust ecosystem, and kache most directly inspired mbx's design.

## Cargo

[Cargo](https://github.com/rust-lang/cargo) resolves dependencies, plans
builds, and orchestrates rustc; mbx only restores supported work from around
that process. Cargo's `RUSTC_WRAPPER` integration is the seam that makes mbx
and other Rust compiler caches possible in the first place.

We are grateful to the Cargo team and the wider Rust project for building and
maintaining the foundation mbx relies on every time it runs.

## sccache

[sccache](https://github.com/mozilla/sccache) established compiler caching as
a practical part of Rust development long before mbx existed. It showed that
rustc work could be reused locally and through remote storage, while serving a
broader set of compilers and use cases than mbx does.

mbx makes different tradeoffs, but it follows a trail sccache helped create.
We are grateful to its maintainers and contributors for proving and advancing
compiler caching in the Rust ecosystem. See the
[comparison with sccache](/compared#sccache) for where the projects differ.

## kache

[kache](https://github.com/kunobi-ninja/kache) is the project that most
directly inspired mbx. It demonstrated that a content-addressed
`RUSTC_WRAPPER` cache could make compilations reusable across worktrees and
machines, and it paired that Rust cache with C and C++ compiler shims, remote
storage, and executable caching. Those ideas helped shape mbx from the
beginning.

mbx would look very different without the path kache opened. We are grateful
to kache's maintainers for building it in public and giving the Rust community
a strong foundation to learn from.

The projects do not share code, and they make different tradeoffs. The
[comparison with kache](/compared#kache) explains where mbx took a different
direction and where kache may be the better fit. Most importantly, kache has
been around longer and has more real-world history behind it. It is the more
proven choice today, and that maturity may matter more than mbx's different
tradeoffs.

Those differences do not diminish the influence: kache is the closest
antecedent to mbx, and that debt deserves to be visible.
