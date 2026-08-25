---
layout: home

hero:
  name: "mr boxington"
  text: "target/, fixed: shared, self-pruning, drop-in."
  tagline: Put mbx in front of any Cargo command. Compiled work is shared across worktrees and CI, build storage keeps itself inside a budget, and mbx tells you what it saved.
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
      text: GitHub Actions
      link: /github-actions

features:
  - icon: "⚙️"
    title: Drop it in front of cargo
    details: Put mbx before build, test, or clippy. Nothing to configure and nothing to install into Cargo; the first build explains what it set up.
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
    details: Budgets scale with the disk. Managed target directories go when their checkout disappears, when they sit unused for 30 days, or when they outgrow their share.
    link: /managed-targets
  - icon: "🧾"
    title: Keeps receipts
    details: One line after a build says what the cache was worth — compilations skipped, hours refunded, gigabytes binned. Deadpan included; savings = "plain" if your logs must keep a straight face.
    link: /getting-started#read-the-result
  - icon: "🔍"
    title: Never lies about a hit
    details: mbx reports hits, misses, what it could not look up, and what it deliberately bypassed. A high hit rate cannot hide work that never entered the cache.
    link: /cache-results
---

::: warning Experimental
mr boxington is pre-1.0. The cache format and behavior may change without notice,
and releases are not a stability promise.
:::
