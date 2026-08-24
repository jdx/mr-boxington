---
layout: home

hero:
  name: "mr boxington"
  text: "Build it once."
  tagline: A Cargo wrapper that shares compiled work across worktrees and CI—and prunes build storage automatically.
  image:
    src: /logo.svg
    alt: Mr Boxington, a friendly cache box
  actions:
    - theme: brand
      text: Get started
      link: /getting-started
    - theme: alt
      text: GitHub Actions
      link: /github-actions

features:
  - icon: "⚙️"
    title: Wrap normal Cargo commands
    details: Put mbx before build, test, or clippy. Cargo still plans the build; mbx restores the rustc work it has seen before.
    link: /getting-started
  - icon: "🌳"
    title: Warm every worktree
    details: Cache keys contain no checkout-specific absolute paths, so building one checkout warms its siblings automatically.
    link: /cache-results
  - icon: "☁️"
    title: Warm CI runners
    details: Use a remote cache or GitHub Actions cache while untrusted pull requests remain read-only.
    link: /github-actions
  - icon: "🧹"
    title: Prune automatically
    details: Keep the cache inside a size budget and reclaim managed target directories after their checkout disappears.
    link: /managed-targets
---

::: warning Experimental
mr boxington is pre-1.0. The cache format and behavior may change without notice,
and releases are not a stability promise.
:::
