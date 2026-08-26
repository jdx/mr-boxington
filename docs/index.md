---
layout: home

hero:
  name: "mr boxington"
  text: "fix target/"
  tagline: Put mbx in front of any cargo command. One cache warms every worktree and CI run, and prunes itself to a size budget.
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
      src: /features/ci.svg
      alt: A cloud with upload and download arrows
      width: 64
      height: 64
    title: Warm CI runners
    details: Share the same cache with CI through a remote cache or GitHub Actions.
    link: /github-actions
  - icon:
      src: /features/prune.svg
      alt: A broom sweeping
      width: 64
      height: 64
    title: Prune automatically
    details: Stale or oversized target directories clean themselves up.
    link: /managed-targets
---
