# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.9.2...mbx-cache-rustc-v0.9.3) - 2026-08-29

### Other

- lead the landing page with three feature cards ([#198](https://github.com/jdx/mr-boxington/pull/198))
- cache linked proc macros ([#197](https://github.com/jdx/mr-boxington/pull/197))
- corrections, editorial rebalance, and a CI anchor check ([#195](https://github.com/jdx/mr-boxington/pull/195))
- *(benchmarks)* demonstrate parallel lint scheduling ([#191](https://github.com/jdx/mr-boxington/pull/191))

## [0.9.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.9.1...mbx-cache-rustc-v0.9.2) - 2026-08-29

### Fixed

- allow caching release-marked builds ([#194](https://github.com/jdx/mr-boxington/pull/194))

## [0.9.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.9.0...mbx-cache-rustc-v0.9.1) - 2026-08-29

### Added

- *(release)* add GNU Linux artifacts ([#181](https://github.com/jdx/mr-boxington/pull/181))
- *(rustc)* cache the compilations that never link ([#177](https://github.com/jdx/mr-boxington/pull/177))

## [0.9.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.8.0...mbx-cache-rustc-v0.9.0) - 2026-08-29

### Added

- *(cache-rustc)* cache macOS debug links behind an oso_prefix the shim appends ([#166](https://github.com/jdx/mr-boxington/pull/166))

### Fixed

- *(cache-rustc)* predict a native search directory by name, not by its contents ([#162](https://github.com/jdx/mr-boxington/pull/162))
- *(cache-rustc)* model -C link-arg where nothing links ([#161](https://github.com/jdx/mr-boxington/pull/161))

### Other

- [**breaking**] stop rehashing inputs the session already read in full ([#164](https://github.com/jdx/mr-boxington/pull/164))

## [0.8.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.7.1...mbx-cache-rustc-v0.8.0) - 2026-08-28

### Fixed

- *(cache-rustc)* key inert native search directories by path ([#153](https://github.com/jdx/mr-boxington/pull/153))

## [0.7.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.7.0...mbx-cache-rustc-v0.7.1) - 2026-08-28

### Added

- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

## [0.7.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.5.1...mbx-cache-rustc-v0.7.0) - 2026-08-27

### Added

- cache natively linked test binaries ([#129](https://github.com/jdx/mr-boxington/pull/129))
- *(cache)* expose shared Cargo cache integration ([#130](https://github.com/jdx/mr-boxington/pull/130))
- compile churning crates incrementally ([#127](https://github.com/jdx/mr-boxington/pull/127))
- mbx tui, a live view of every build's cache activity ([#128](https://github.com/jdx/mr-boxington/pull/128))

### Fixed

- *(rustc)* restore results into the checkout that asked for them ([#141](https://github.com/jdx/mr-boxington/pull/141))

## [0.5.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.5.0...mbx-cache-rustc-v0.5.1) - 2026-08-26

### Fixed

- cache libraries with native search paths ([#120](https://github.com/jdx/mr-boxington/pull/120))

## [0.5.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.4.0...mbx-cache-rustc-v0.5.0) - 2026-08-26

### Added

- [**breaking**] open the extensible public types to extension ([#103](https://github.com/jdx/mr-boxington/pull/103))

### Other

- simplify the landing page for 1.0 ([#96](https://github.com/jdx/mr-boxington/pull/96))
- give every published crate its crates.io metadata ([#97](https://github.com/jdx/mr-boxington/pull/97))
- version each crate by what it promises ([#100](https://github.com/jdx/mr-boxington/pull/100))

## [0.4.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.3.0...mbx-cache-rustc-v0.4.0) - 2026-08-25

### Added

- report compiler time saved and spent ([#59](https://github.com/jdx/mr-boxington/pull/59))
- support rustc response files ([#65](https://github.com/jdx/mr-boxington/pull/65))
- cache compiler-bundled WebAssembly links ([#66](https://github.com/jdx/mr-boxington/pull/66))
- *(protocol)* share remote cache contract ([#68](https://github.com/jdx/mr-boxington/pull/68))
- cache compiler-linked wasm outputs ([#45](https://github.com/jdx/mr-boxington/pull/45))
- cache plain cargo commands after setup ([#40](https://github.com/jdx/mr-boxington/pull/40))

### Other

- give every crate one synchronized version ([#86](https://github.com/jdx/mr-boxington/pull/86))
- establish compatibility and security policy ([#37](https://github.com/jdx/mr-boxington/pull/37))
- move inline tests into focused modules ([#42](https://github.com/jdx/mr-boxington/pull/42))
- define published Rust API surface ([#41](https://github.com/jdx/mr-boxington/pull/41))

## [0.2.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.1.0...mbx-cache-rustc-v0.2.0) - 2026-08-22

### Added

- *(session)* share compilations that read OUT_DIR across checkouts ([#21](https://github.com/jdx/mr-boxington/pull/21))

## [0.1.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-rustc-v0.0.0...mbx-cache-rustc-v0.1.0) - 2026-08-21

### Added

- *(release)* publish prebuilt binaries on a tag ([#14](https://github.com/jdx/mr-boxington/pull/14))
- *(session)* log why each compilation was not cached ([#11](https://github.com/jdx/mr-boxington/pull/11))
- *(session)* count compilations the cache declined ([#10](https://github.com/jdx/mr-boxington/pull/10))
- *(cache-rustc)* add rustc action analysis ([#5](https://github.com/jdx/mr-boxington/pull/5))
