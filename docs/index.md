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

::: warning GitHub Actions performance
mbx performs well for local development. Remote caching works and is actively
improving, but does not yet consistently outperform
[Swatinem/rust-cache](https://github.com/Swatinem/rust-cache) on GitHub-hosted
runners. Benchmark your complete workflow before switching.
:::
