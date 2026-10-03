---
description: Find the right guide for installing mbx, local development, CI caching, configuration, and troubleshooting.
---
# Documentation

mbx adds a shared compiler cache and automatic storage management to Cargo.
Start with a local build, then add the integrations your workflow needs.

## Start here

[Get started](/getting-started) takes you from installation to your first
cached build. For platform-specific downloads, see [Installation](/installation).
For automatic Cargo wrapping and rust-analyzer, see [Cargo and editor setup](/setup).

## Work locally

| Task | Guide |
| --- | --- |
| Use editors, watch loops, or a debugger | [Local development](/cookbook/local-development) |
| Understand what is stored or deleted | [Managed target directories](/managed-targets) |
| Run tests and Clippy at the same time | [Parallel builds](/scheduling) |
| Tune repeated source edits | [Incremental builds](/incremental) |
| Select a linker by profile and target | [Managed linkers](/linkers) |
| Watch compiler and cache activity | [Watching builds](/tui) |
| Cache make or CMake compiler calls | [Standalone C and C++](/standalone-builds) |

## Set up CI and sharing

| Task | Guide |
| --- | --- |
| Add caching to a GitHub Actions job | [GitHub Action](/github-action) |
| Replace an existing caching step | [Migrate from rust-cache or sccache](/cookbook/migrate) |
| Support pull requests from forks | [CI with fork pull requests](/cookbook/fork-prs) |
| Connect a server or S3-compatible bucket | [Remote cache](/remote-cache) |
| Operate the reference server | [Cache server](/cache-server) |

## Understand a result

| Task | Guide |
| --- | --- |
| Diagnose an unexpected build | [Troubleshooting](/troubleshooting) |
| Rank where a rebuild's uncached compiler time went | [Analyzing a build](/analyze) |
| Interpret the cache counters | [Cache results](/cache-results) |
| See lifetime totals and workspace sharing | [Savings and statistics](/stats) |
| Explore live activity and per-build insights | [Watching builds](/tui) |
| Learn the architecture | [How it works](/how-it-works) |
| See which invocations mbx bypasses | [Caching limits](/limits) |
| Read common questions and the story behind the name | [FAQ](/faq) |

## About mbx

[Benchmarks](/benchmarks) includes measured results and the method behind them.
[How mbx compares](/compared) explains the tradeoffs with other caches and
Cargo's incremental compilation.

## Look something up

- [Configuration](/configuration): file locations, precedence, and every setting
- [CLI reference](/cli/): command syntax, flags, and arguments
- [Stability](/stability): upgrade behavior and supported output formats
- [Protocol compatibility](/protocol-compatibility): local, remote, and Rust API contracts

To improve these docs or contribute code, read
[Contributing](https://github.com/jdx/mr-boxington/blob/main/CONTRIBUTING.md).
The [acknowledgements](/acknowledgements) recognize the projects mbx builds on.
