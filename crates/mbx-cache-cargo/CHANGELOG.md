# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.15](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.14...mbx-cache-cargo-v0.1.15) - 2026-09-02

### Other

- updated the following local packages: mbx-cache-core

## [0.1.14](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.13...mbx-cache-cargo-v0.1.14) - 2026-09-02

### Other

- trim the README and correct link caching claims ([#277](https://github.com/jdx/mr-boxington/pull/277))

## [0.1.13](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.12...mbx-cache-cargo-v0.1.13) - 2026-09-02

### Fixed

- fix managed target lifecycle edges ([#269](https://github.com/jdx/mr-boxington/pull/269))
- release cache pinned by phantom checkouts ([#270](https://github.com/jdx/mr-boxington/pull/270))

### Other

- preserve Cargo rustc probe cache ([#268](https://github.com/jdx/mr-boxington/pull/268))

## [0.1.12](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.11...mbx-cache-cargo-v0.1.12) - 2026-09-01

### Fixed

- share predictions across Cargo commands ([#256](https://github.com/jdx/mr-boxington/pull/256))

## [0.1.11](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.10...mbx-cache-cargo-v0.1.11) - 2026-09-01

### Other

- updated the following local packages: mbx-cache-core

## [0.1.10](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.9...mbx-cache-cargo-v0.1.10) - 2026-08-31

### Added

- *(setup)* use mise command wrappers ([#249](https://github.com/jdx/mr-boxington/pull/249))

## [0.1.9](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.8...mbx-cache-cargo-v0.1.9) - 2026-08-31

### Fixed

- document Cargo shim activation for agents ([#233](https://github.com/jdx/mr-boxington/pull/233))

### Other

- bound remote cache prefetch work ([#239](https://github.com/jdx/mr-boxington/pull/239))

## [0.1.8](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.7...mbx-cache-cargo-v0.1.8) - 2026-08-30

### Added

- *(cache)* deduplicate in-flight work across runners ([#223](https://github.com/jdx/mr-boxington/pull/223))
- cache rustdoc actions ([#226](https://github.com/jdx/mr-boxington/pull/226))

### Other

- *(mbx)* split CLI commands into modules ([#212](https://github.com/jdx/mr-boxington/pull/212))

## [0.1.7](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.6...mbx-cache-cargo-v0.1.7) - 2026-08-29

### Other

- recognize Cargo, sccache, and kache ([#205](https://github.com/jdx/mr-boxington/pull/205))

## [0.1.6](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.5...mbx-cache-cargo-v0.1.6) - 2026-08-29

### Other

- lead the landing page with three feature cards ([#198](https://github.com/jdx/mr-boxington/pull/198))
- corrections, editorial rebalance, and a CI anchor check ([#195](https://github.com/jdx/mr-boxington/pull/195))
- *(benchmarks)* demonstrate parallel lint scheduling ([#191](https://github.com/jdx/mr-boxington/pull/191))

## [0.1.5](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.4...mbx-cache-cargo-v0.1.5) - 2026-08-29

### Fixed

- allow caching release-marked builds ([#194](https://github.com/jdx/mr-boxington/pull/194))

## [0.1.4](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.3...mbx-cache-cargo-v0.1.4) - 2026-08-29

### Added

- *(release)* add GNU Linux artifacts ([#181](https://github.com/jdx/mr-boxington/pull/181))

## [0.1.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.2...mbx-cache-cargo-v0.1.3) - 2026-08-29

### Other

- updated the following local packages: mbx-cache-core

## [0.1.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.1...mbx-cache-cargo-v0.1.2) - 2026-08-28

### Other

- updated the following local packages: mbx-cache-core

## [0.1.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-cargo-v0.1.0...mbx-cache-cargo-v0.1.1) - 2026-08-28

### Added

- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

## [0.1.0](https://github.com/jdx/mr-boxington/releases/tag/mbx-cache-cargo-v0.1.0) - 2026-08-27

### Added

- *(cache)* expose shared Cargo cache integration ([#130](https://github.com/jdx/mr-boxington/pull/130))
- mbx tui, a live view of every build's cache activity ([#128](https://github.com/jdx/mr-boxington/pull/128))
- [**breaking**] open the extensible public types to extension ([#103](https://github.com/jdx/mr-boxington/pull/103))
- make landing demo interactive and tag output ([#94](https://github.com/jdx/mr-boxington/pull/94))
- make mbx build the golden path and hide setup ([#75](https://github.com/jdx/mr-boxington/pull/75))
- add cache inspection commands ([#63](https://github.com/jdx/mr-boxington/pull/63))
- add managed target retention policies ([#62](https://github.com/jdx/mr-boxington/pull/62))
- cache compiler-bundled WebAssembly links ([#66](https://github.com/jdx/mr-boxington/pull/66))
- add explicit remote prefetch ([#64](https://github.com/jdx/mr-boxington/pull/64))
- cache compiler-linked wasm outputs ([#45](https://github.com/jdx/mr-boxington/pull/45))
- cache plain cargo commands after setup ([#40](https://github.com/jdx/mr-boxington/pull/40))
- add direct cargo wrapper and docs website ([#28](https://github.com/jdx/mr-boxington/pull/28))
- *(target)* place target directories so a deleted checkout frees them ([#24](https://github.com/jdx/mr-boxington/pull/24))
- *(store)* collect automatically, and release deleted checkouts first ([#23](https://github.com/jdx/mr-boxington/pull/23))
- *(session)* share compilations that read OUT_DIR across checkouts ([#21](https://github.com/jdx/mr-boxington/pull/21))
- *(session)* let a build opt into incremental compilation ([#20](https://github.com/jdx/mr-boxington/pull/20))
- *(session)* count the compilations the cache was never asked about ([#18](https://github.com/jdx/mr-boxington/pull/18))
- *(release)* publish prebuilt binaries on a tag ([#14](https://github.com/jdx/mr-boxington/pull/14))
- *(session)* log why each compilation was not cached ([#11](https://github.com/jdx/mr-boxington/pull/11))
- *(session)* count compilations the cache declined ([#10](https://github.com/jdx/mr-boxington/pull/10))

### Other

- simplify the landing page for 1.0 ([#96](https://github.com/jdx/mr-boxington/pull/96))
- stop building macOS x64 releases ([#92](https://github.com/jdx/mr-boxington/pull/92))
- recommend mr-boxington-action ([#54](https://github.com/jdx/mr-boxington/pull/54))
- enforce protocol and API compatibility ([#43](https://github.com/jdx/mr-boxington/pull/43))
- establish compatibility and security policy ([#37](https://github.com/jdx/mr-boxington/pull/37))
- reword tagline to name target/ ([#36](https://github.com/jdx/mr-boxington/pull/36))
- *(release)* hand versioning and publishing to release-plz ([#16](https://github.com/jdx/mr-boxington/pull/16))
- qualify cross-checkout sharing, and test the boundary ([#12](https://github.com/jdx/mr-boxington/pull/12))
- document usage and the standalone-mode blocker ([#8](https://github.com/jdx/mr-boxington/pull/8))
- initial placeholder readme
