# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.5](https://github.com/jdx/mr-boxington/compare/mbx-cache-protocol-v0.5.4...mbx-cache-protocol-v0.5.5) - 2026-08-29

### Fixed

- allow caching release-marked builds ([#194](https://github.com/jdx/mr-boxington/pull/194))

## [0.5.4](https://github.com/jdx/mr-boxington/compare/mbx-cache-protocol-v0.5.3...mbx-cache-protocol-v0.5.4) - 2026-08-29

### Added

- *(release)* add GNU Linux artifacts ([#181](https://github.com/jdx/mr-boxington/pull/181))

## [0.5.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-protocol-v0.5.2...mbx-cache-protocol-v0.5.3) - 2026-08-29

### Fixed

- *(cache-rustc)* predict a native search directory by name, not by its contents ([#162](https://github.com/jdx/mr-boxington/pull/162))

## [0.5.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-protocol-v0.5.1...mbx-cache-protocol-v0.5.2) - 2026-08-28

### Added

- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

## [0.5.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-protocol-v0.5.0...mbx-cache-protocol-v0.5.1) - 2026-08-27

### Added

- cache C and C++ compiles from build scripts ([#132](https://github.com/jdx/mr-boxington/pull/132))
- batch remote action lookups and blob uploads ([#131](https://github.com/jdx/mr-boxington/pull/131))
- mbx tui, a live view of every build's cache activity ([#128](https://github.com/jdx/mr-boxington/pull/128))

## [0.5.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-protocol-v0.4.0...mbx-cache-protocol-v0.5.0) - 2026-08-26

### Added

- [**breaking**] open the extensible public types to extension ([#103](https://github.com/jdx/mr-boxington/pull/103))

### Other

- simplify the landing page for 1.0 ([#96](https://github.com/jdx/mr-boxington/pull/96))
- give every published crate its crates.io metadata ([#97](https://github.com/jdx/mr-boxington/pull/97))
- version each crate by what it promises ([#100](https://github.com/jdx/mr-boxington/pull/100))

## [0.4.0](https://github.com/jdx/mr-boxington/releases/tag/mbx-cache-protocol-v0.4.0) - 2026-08-25

### Added

- *(protocol)* share remote cache contract ([#68](https://github.com/jdx/mr-boxington/pull/68))

### Added

- Publish the versioned remote-cache wire types, capability schema, media types,
  headers, and blob-pack framing shared by mbx clients and servers.
