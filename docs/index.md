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

## With thanks to kache

mbx is deeply inspired by [kache](https://github.com/kunobi-ninja/kache).
kache demonstrated how a content-addressed Rust compiler cache could share
work across checkouts and machines, and its ideas helped shape mbx from the
beginning. It has also been around longer and is the more proven choice today.
We are grateful to its maintainers for the path they opened.

[Read about kache's influence on mbx →](/acknowledgements)
