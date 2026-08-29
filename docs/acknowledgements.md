# Acknowledgements

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
