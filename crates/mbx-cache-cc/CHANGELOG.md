# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.11.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.11.0...mbx-cache-cc-v0.11.1) - 2026-09-02

### Other

- trim the README and correct link caching claims ([#277](https://github.com/jdx/mr-boxington/pull/277))

## [0.11.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.10.3...mbx-cache-cc-v0.11.0) - 2026-09-02

### Fixed

- fix managed target lifecycle edges ([#269](https://github.com/jdx/mr-boxington/pull/269))
- release cache pinned by phantom checkouts ([#270](https://github.com/jdx/mr-boxington/pull/270))
- *(cache)* model no-input assembler options ([#266](https://github.com/jdx/mr-boxington/pull/266))

## [0.10.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.10.1...mbx-cache-cc-v0.10.2) - 2026-08-31

### Added

- *(setup)* use mise command wrappers ([#249](https://github.com/jdx/mr-boxington/pull/249))

## [0.10.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.10.0...mbx-cache-cc-v0.10.1) - 2026-08-31

### Fixed

- document Cargo shim activation for agents ([#233](https://github.com/jdx/mr-boxington/pull/233))

## [0.10.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.9.4...mbx-cache-cc-v0.10.0) - 2026-08-30

### Added

- *(cache)* deduplicate in-flight work across runners ([#223](https://github.com/jdx/mr-boxington/pull/223))
- cache Windows links and MSVC compiles ([#224](https://github.com/jdx/mr-boxington/pull/224))
- cache rustdoc actions ([#226](https://github.com/jdx/mr-boxington/pull/226))
- *(mbx)* prescribe fixes for cache bypasses ([#222](https://github.com/jdx/mr-boxington/pull/222))

### Other

- share cache path mapping ([#215](https://github.com/jdx/mr-boxington/pull/215))

## [0.9.4](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.9.3...mbx-cache-cc-v0.9.4) - 2026-08-29

### Fixed

- *(cc)* deduplicate recursive include manifests ([#210](https://github.com/jdx/mr-boxington/pull/210))

### Other

- recognize Cargo, sccache, and kache ([#205](https://github.com/jdx/mr-boxington/pull/205))

## [0.9.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.9.2...mbx-cache-cc-v0.9.3) - 2026-08-29

### Other

- lead the landing page with three feature cards ([#198](https://github.com/jdx/mr-boxington/pull/198))
- corrections, editorial rebalance, and a CI anchor check ([#195](https://github.com/jdx/mr-boxington/pull/195))
- *(benchmarks)* demonstrate parallel lint scheduling ([#191](https://github.com/jdx/mr-boxington/pull/191))

## [0.9.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.9.1...mbx-cache-cc-v0.9.2) - 2026-08-29

### Fixed

- allow caching release-marked builds ([#194](https://github.com/jdx/mr-boxington/pull/194))

## [0.9.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.9.0...mbx-cache-cc-v0.9.1) - 2026-08-29

### Added

- *(release)* add GNU Linux artifacts ([#181](https://github.com/jdx/mr-boxington/pull/181))

### Fixed

- *(cc)* make an object independent of the directory it was built in ([#185](https://github.com/jdx/mr-boxington/pull/185))

## [0.9.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.8.0...mbx-cache-cc-v0.9.0) - 2026-08-29

### Other

- [**breaking**] stop rehashing inputs the session already read in full ([#164](https://github.com/jdx/mr-boxington/pull/164))

## [0.7.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.7.0...mbx-cache-cc-v0.7.1) - 2026-08-28

### Added

- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

## [0.7.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-cc-v0.1.0...mbx-cache-cc-v0.7.0) - 2026-08-27

### Added

- cache C and C++ compiles from build scripts ([#132](https://github.com/jdx/mr-boxington/pull/132))
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
