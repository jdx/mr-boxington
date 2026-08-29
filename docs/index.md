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

mbx relies on [Cargo](https://github.com/rust-lang/cargo) and follows earlier
compiler-cache work in [sccache](https://github.com/mozilla/sccache) and
[kache](https://github.com/kunobi-ninja/kache), which directly inspired its
design — [read the acknowledgements](/acknowledgements).
