# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1](https://github.com/jdx/mr-boxington/compare/v0.4.0...v0.4.1) - 2026-08-26

### Added

- make landing demo interactive and tag output ([#94](https://github.com/jdx/mr-boxington/pull/94))

## [0.4.0](https://github.com/jdx/mr-boxington/compare/v0.3.0...v0.4.0) - 2026-08-25

### Added

- make mbx build the golden path and hide setup ([#75](https://github.com/jdx/mr-boxington/pull/75))
- explain the cache and its caps on the first build ([#74](https://github.com/jdx/mr-boxington/pull/74))
- report what mbx has saved on this machine ([#73](https://github.com/jdx/mr-boxington/pull/73))
- scale disk budgets to the disk and prune idle targets by default ([#72](https://github.com/jdx/mr-boxington/pull/72))
- add cache inspection commands ([#63](https://github.com/jdx/mr-boxington/pull/63))
- add managed target retention policies ([#62](https://github.com/jdx/mr-boxington/pull/62))
- add JSON inspection output ([#60](https://github.com/jdx/mr-boxington/pull/60))
- add installation doctor ([#57](https://github.com/jdx/mr-boxington/pull/57))
- report compiler time saved and spent ([#59](https://github.com/jdx/mr-boxington/pull/59))
- complete setup lifecycle ([#61](https://github.com/jdx/mr-boxington/pull/61))
- add explicit remote prefetch ([#64](https://github.com/jdx/mr-boxington/pull/64))
- *(config)* add safe workspace policy ([#67](https://github.com/jdx/mr-boxington/pull/67))
- *(protocol)* share remote cache contract ([#68](https://github.com/jdx/mr-boxington/pull/68))
- explain cache bypasses ([#58](https://github.com/jdx/mr-boxington/pull/58))
- cache compiler-linked wasm outputs ([#45](https://github.com/jdx/mr-boxington/pull/45))
- cache plain cargo commands after setup ([#40](https://github.com/jdx/mr-boxington/pull/40))
- add direct cargo wrapper and docs website ([#28](https://github.com/jdx/mr-boxington/pull/28))

### Fixed

- probe reflinks across the span the restore actually copies ([#84](https://github.com/jdx/mr-boxington/pull/84))
- deflake two tests that race the machine ([#82](https://github.com/jdx/mr-boxington/pull/82))
- restore the build after a merge skewed a verify call ([#71](https://github.com/jdx/mr-boxington/pull/71))

### Other

- give every crate one synchronized version ([#86](https://github.com/jdx/mr-boxington/pull/86))
- stop checking the CLI library's public API ([#77](https://github.com/jdx/mr-boxington/pull/77))
- wrap project cargo commands with mbx ([#34](https://github.com/jdx/mr-boxington/pull/34))
- *(deps)* bump the cargo-dependencies group across 1 directory with 2 updates ([#55](https://github.com/jdx/mr-boxington/pull/55))
- add Bats end-to-end harness ([#46](https://github.com/jdx/mr-boxington/pull/46))
- establish compatibility and security policy ([#37](https://github.com/jdx/mr-boxington/pull/37))
- move inline tests into focused modules ([#42](https://github.com/jdx/mr-boxington/pull/42))
- define published Rust API surface ([#41](https://github.com/jdx/mr-boxington/pull/41))
- *(cache)* avoid rereading reflinked outputs ([#44](https://github.com/jdx/mr-boxington/pull/44))
- *(config)* generate settings docs with usage-rs ([#30](https://github.com/jdx/mr-boxington/pull/30))

### Added

- *(cli)* run a Cargo command with actionable cache-bypass diagnostics using `mbx explain`

### Changed

- *(cache)* restore verified outputs with observable copy-on-write materialization

## [0.3.0](https://github.com/jdx/mr-boxington/compare/v0.2.0...v0.3.0) - 2026-08-23

### Added

- *(target)* place target directories so a deleted checkout frees them ([#24](https://github.com/jdx/mr-boxington/pull/24))
- *(store)* collect automatically, and release deleted checkouts first ([#23](https://github.com/jdx/mr-boxington/pull/23))

## [0.2.0](https://github.com/jdx/mr-boxington/compare/v0.1.0...v0.2.0) - 2026-08-22

### Added

- *(session)* share compilations that read OUT_DIR across checkouts ([#21](https://github.com/jdx/mr-boxington/pull/21))
- *(session)* let a build opt into incremental compilation ([#20](https://github.com/jdx/mr-boxington/pull/20))
- *(session)* count the compilations the cache was never asked about ([#18](https://github.com/jdx/mr-boxington/pull/18))

## [0.1.0](https://github.com/jdx/mr-boxington/compare/v0.0.0...v0.1.0) - 2026-08-21

### Added

- *(release)* publish prebuilt binaries on a tag ([#14](https://github.com/jdx/mr-boxington/pull/14))
- *(session)* log why each compilation was not cached ([#11](https://github.com/jdx/mr-boxington/pull/11))
- *(session)* count compilations the cache declined ([#10](https://github.com/jdx/mr-boxington/pull/10))
- *(cli)* add build, gc, and cache commands ([#7](https://github.com/jdx/mr-boxington/pull/7))
- *(session)* add the cache session and rustc shim ([#6](https://github.com/jdx/mr-boxington/pull/6))

### Other

- *(cli)* stop the target-dir probe test reading the ambient environment ([#15](https://github.com/jdx/mr-boxington/pull/15))
- qualify cross-checkout sharing, and test the boundary ([#12](https://github.com/jdx/mr-boxington/pull/12))
- *(session)* stop fsyncing restored outputs ([#9](https://github.com/jdx/mr-boxington/pull/9))
- add workspace scaffolding and CI ([#2](https://github.com/jdx/mr-boxington/pull/2))
