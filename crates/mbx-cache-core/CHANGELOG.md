# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.7.1...mbx-cache-core-v0.8.0) - 2026-08-28

### Fixed

- *(cc)* [**breaking**] keep shim diagnostics off the intercepted compiler's stderr ([#154](https://github.com/jdx/mr-boxington/pull/154))
- *(cache-rustc)* key inert native search directories by path ([#153](https://github.com/jdx/mr-boxington/pull/153))

### Other

- stop rereading cached artifacts on warm hits ([#152](https://github.com/jdx/mr-boxington/pull/152))

## [0.7.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.7.0...mbx-cache-core-v0.7.1) - 2026-08-28

### Added

- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

## [0.7.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.5.1...mbx-cache-core-v0.7.0) - 2026-08-27

### Added

- cache C and C++ compiles from build scripts ([#132](https://github.com/jdx/mr-boxington/pull/132))
- cache natively linked test binaries ([#129](https://github.com/jdx/mr-boxington/pull/129))
- cache to an S3-compatible object store ([#140](https://github.com/jdx/mr-boxington/pull/140))
- *(cache)* expose shared Cargo cache integration ([#130](https://github.com/jdx/mr-boxington/pull/130))
- batch remote action lookups and blob uploads ([#131](https://github.com/jdx/mr-boxington/pull/131))
- publish remote objects after the build asks for them ([#126](https://github.com/jdx/mr-boxington/pull/126))
- compile churning crates incrementally ([#127](https://github.com/jdx/mr-boxington/pull/127))
- mbx tui, a live view of every build's cache activity ([#128](https://github.com/jdx/mr-boxington/pull/128))

### Fixed

- bound blob pack members to digest sizes ([#136](https://github.com/jdx/mr-boxington/pull/136))

### Other

- define download_timeout as a whole-download deadline ([#142](https://github.com/jdx/mr-boxington/pull/142))

## [0.5.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.4.0...mbx-cache-core-v0.5.0) - 2026-08-26

### Added

- [**breaking**] open the extensible public types to extension ([#103](https://github.com/jdx/mr-boxington/pull/103))
- count remote cache failures in the summary ([#112](https://github.com/jdx/mr-boxington/pull/112))

### Fixed

- carry manifest entity tags opaquely ([#110](https://github.com/jdx/mr-boxington/pull/110))
- stop timing the runner in the prefetch independence test ([#105](https://github.com/jdx/mr-boxington/pull/105))

### Other

- simplify the landing page for 1.0 ([#96](https://github.com/jdx/mr-boxington/pull/96))
- let the agent's statistics grow without breaking ([#114](https://github.com/jdx/mr-boxington/pull/114))
- give every published crate its crates.io metadata ([#97](https://github.com/jdx/mr-boxington/pull/97))
- version each crate by what it promises ([#100](https://github.com/jdx/mr-boxington/pull/100))

### Security

- bound remote downloads and protect release assets ([#109](https://github.com/jdx/mr-boxington/pull/109))

## [0.4.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.3.0...mbx-cache-core-v0.4.0) - 2026-08-25

### Added

- add installation doctor ([#57](https://github.com/jdx/mr-boxington/pull/57))
- report compiler time saved and spent ([#59](https://github.com/jdx/mr-boxington/pull/59))
- add explicit remote prefetch ([#64](https://github.com/jdx/mr-boxington/pull/64))
- *(protocol)* share remote cache contract ([#68](https://github.com/jdx/mr-boxington/pull/68))
- cache plain cargo commands after setup ([#40](https://github.com/jdx/mr-boxington/pull/40))

### Other

- give every crate one synchronized version ([#86](https://github.com/jdx/mr-boxington/pull/86))
- fuzz untrusted parser inputs ([#38](https://github.com/jdx/mr-boxington/pull/38))
- enforce protocol and API compatibility ([#43](https://github.com/jdx/mr-boxington/pull/43))
- establish compatibility and security policy ([#37](https://github.com/jdx/mr-boxington/pull/37))
- move inline tests into focused modules ([#42](https://github.com/jdx/mr-boxington/pull/42))
- define published Rust API surface ([#41](https://github.com/jdx/mr-boxington/pull/41))
- *(cache)* avoid rereading reflinked outputs ([#44](https://github.com/jdx/mr-boxington/pull/44))

### Changed

- *(protocol)* consume remote wire types and constants from `mbx-cache-protocol`

## [0.3.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.2.0...mbx-cache-core-v0.3.0) - 2026-08-23

### Added

- *(store)* collect automatically, and release deleted checkouts first ([#23](https://github.com/jdx/mr-boxington/pull/23))

## [0.2.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.1.0...mbx-cache-core-v0.2.0) - 2026-08-22

### Added

- *(cache-core)* compress remote transfers with zstd ([#22](https://github.com/jdx/mr-boxington/pull/22))
- *(session)* count the compilations the cache was never asked about ([#18](https://github.com/jdx/mr-boxington/pull/18))

## [0.1.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.0.0...mbx-cache-core-v0.1.0) - 2026-08-21

### Added

- *(release)* publish prebuilt binaries on a tag ([#14](https://github.com/jdx/mr-boxington/pull/14))
- *(session)* count compilations the cache declined ([#10](https://github.com/jdx/mr-boxington/pull/10))
- *(cache-core)* add action cache protocol and transport ([#4](https://github.com/jdx/mr-boxington/pull/4))

### Other

- *(cache-core)* stop fsyncing every stored object ([#13](https://github.com/jdx/mr-boxington/pull/13))
