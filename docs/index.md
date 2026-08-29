---
layout: home

hero:
  name: "mr boxington"
  text: "fix <code>target/</code>"
  tagline: Put mbx in front of any cargo command. Every build on the machine shares one self-pruning cache, and you can run multiple Cargo builds in parallel.
  image:
    src: /logo.svg
    alt: Mr Boxington, a friendly cache box
  actions:
    - theme: brand
      text: Get started
      link: /getting-started
    - theme: alt
      text: How it works
      link: /how-it-works
    - theme: alt
      text: GitHub Action
      link: /github-action
    - theme: alt
      text: Benchmarks
      link: /benchmarks
---

## With thanks to Cargo, sccache, and kache

[Cargo](https://github.com/rust-lang/cargo) is the foundation mbx runs on, and
its `RUSTC_WRAPPER` integration makes Rust compiler caches possible.
[sccache](https://github.com/mozilla/sccache) established compiler caching in
the Rust ecosystem. [kache](https://github.com/kunobi-ninja/kache) most
directly inspired mbx's design; it has also been around longer and is the more
proven choice today. We are grateful to all of their maintainers and
contributors.

[Read the acknowledgements →](/acknowledgements)
