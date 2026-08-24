---
layout: home

hero:
  name: "mr boxington"
  text: "Build it once."
  tagline: A content-addressed build cache for Rust projects, worktrees, and CI.
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
  - icon: "⚡"
    title: Cache rustc actions
    details: Restore individual compilations instead of rebuilding an entire dependency graph.
    link: /how-it-works
  - icon: "🌳"
    title: Share across worktrees
    details: Cache keys contain no checkout-specific absolute paths, so one checkout can warm another.
    link: /cache-results
  - icon: "☁️"
    title: Warm CI runners
    details: Use a remote cache or GitHub Actions cache while untrusted pull requests remain read-only.
    link: /github-actions
  - icon: "📦"
    title: Reclaim build storage
    details: Sweep cached objects to a budget and optionally collect target directories after their checkout disappears.
    link: /managed-targets
---

::: warning Experimental
mr boxington is pre-1.0. The cache format and behavior may change without notice,
and releases are not a stability promise.
:::
