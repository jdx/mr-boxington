# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-store-v0.1.0...mbx-cache-store-v0.1.1) - 2026-08-28

### Added

- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

## [0.1.0](https://github.com/jdx/mr-boxington/releases/tag/mbx-cache-store-v0.1.0) - 2026-08-27

### Added

- cache standalone C and C++ builds through mbx exec ([#138](https://github.com/jdx/mr-boxington/pull/138))
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
