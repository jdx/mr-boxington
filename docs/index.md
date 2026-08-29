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

features:
  - icon:
      src: /features/drop-in.svg
      alt: A golden gear
      width: 64
      height: 64
    title: Drop it in front of cargo
    details: mbx build, mbx test, mbx clippy. Nothing to configure, no daemon to manage, nothing to install into Cargo.
    link: /getting-started
  - icon:
      src: /features/worktrees.svg
      alt: A tree growing small cache boxes
      width: 64
      height: 64
    title: Warm every worktree
    details: Build in one checkout and every other worktree starts warm.
    link: /cache-results
  - icon:
      src: /features/benchmarks.svg
      alt: A stopwatch with a rising performance line
      width: 64
      height: 64
    title: Prove the speedup
    details: Reproducible CI benchmarks compare caching and concurrency strategies, with validity gates around every published result.
    link: /benchmarks
---
