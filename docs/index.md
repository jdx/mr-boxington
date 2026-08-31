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
mbx is not currently as fast as
[Swatinem/rust-cache](https://github.com/Swatinem/rust-cache) in our
GitHub-hosted runner benchmarks. Do not replace rust-cache solely for CI speed;
benchmark your workflow first. The [GitHub Action guide](/github-action)
describes the current results and limitations.

mbx can still be preferable when one cache needs to work across local
worktrees and CI, several builds run concurrently, fine-grained reuse matters,
or detailed cache diagnostics and self-hosted control are requirements. Those
benefits do not currently translate into a faster conventional hosted-runner
job.
:::
