# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
