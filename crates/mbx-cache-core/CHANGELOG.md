# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.12.0...mbx-cache-core-v0.13.0) - 2026-09-05

### Other

- take the bookkeeping out of the hot edit loop ([#362](https://github.com/jdx/mr-boxington/pull/362))

## [0.12.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.11.5...mbx-cache-core-v0.12.0) - 2026-09-05

### Fixed

- restore NFS digest reuse without read storms ([#341](https://github.com/jdx/mr-boxington/pull/341))

### Other

- *(cache)* avoid redundant import copies and hashes ([#353](https://github.com/jdx/mr-boxington/pull/353))

## [0.11.5](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.11.4...mbx-cache-core-v0.11.5) - 2026-09-04

### Fixed

- verify NFS compiler inputs by content ([#338](https://github.com/jdx/mr-boxington/pull/338))

## [0.11.4](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.11.3...mbx-cache-core-v0.11.4) - 2026-09-03

### Added

- *(cache)* inherit predictions from earlier lockfiles ([#327](https://github.com/jdx/mr-boxington/pull/327))

### Other

- keep hot-edit bookkeeping off the build's critical path ([#331](https://github.com/jdx/mr-boxington/pull/331))
- *(cache)* adopt prefetched blobs into CAS ([#324](https://github.com/jdx/mr-boxington/pull/324))
- *(cache)* download blob packs concurrently ([#323](https://github.com/jdx/mr-boxington/pull/323))
- *(cache)* gate prefetch on matching adapters ([#321](https://github.com/jdx/mr-boxington/pull/321))

## [0.11.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.11.2...mbx-cache-core-v0.11.3) - 2026-09-03

### Added

- manage profile-specific linkers ([#319](https://github.com/jdx/mr-boxington/pull/319))

### Other

- *(cache)* complete large remote transfers reliably ([#320](https://github.com/jdx/mr-boxington/pull/320))

## [0.11.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.11.1...mbx-cache-core-v0.11.2) - 2026-09-02

### Fixed

- *(remote)* bound what a build loses to failed cache reads ([#292](https://github.com/jdx/mr-boxington/pull/292))

### Other

- verify compiler inputs by identity after compiling ([#284](https://github.com/jdx/mr-boxington/pull/284))
- carry the file-digest ledger across sessions ([#285](https://github.com/jdx/mr-boxington/pull/285))

## [0.11.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.11.0...mbx-cache-core-v0.11.1) - 2026-09-02

### Other

- trim the README and correct link caching claims ([#277](https://github.com/jdx/mr-boxington/pull/277))

## [0.11.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.10.3...mbx-cache-core-v0.11.0) - 2026-09-02

### Added

- explain cache misses from session history ([#274](https://github.com/jdx/mr-boxington/pull/274))

### Fixed

- fix managed target lifecycle edges ([#269](https://github.com/jdx/mr-boxington/pull/269))
- release cache pinned by phantom checkouts ([#270](https://github.com/jdx/mr-boxington/pull/270))
- cache clippy workspace compilations ([#273](https://github.com/jdx/mr-boxington/pull/273))
- *(cache)* include remote error response details ([#275](https://github.com/jdx/mr-boxington/pull/275))

### Other

- preserve Cargo rustc probe cache ([#268](https://github.com/jdx/mr-boxington/pull/268))

## [0.10.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.10.2...mbx-cache-core-v0.10.3) - 2026-09-01

### Added

- *(cache)* model -fuse-ld linker selection for native links ([#254](https://github.com/jdx/mr-boxington/pull/254))

## [0.10.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.10.1...mbx-cache-core-v0.10.2) - 2026-08-31

### Added

- *(setup)* use mise command wrappers ([#249](https://github.com/jdx/mr-boxington/pull/249))

### Other

- *(cache)* prefetch outputs in progressive waves ([#244](https://github.com/jdx/mr-boxington/pull/244))
- *(cache)* reduce speculative action prefetch ([#242](https://github.com/jdx/mr-boxington/pull/242))

## [0.10.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.10.0...mbx-cache-core-v0.10.1) - 2026-08-31

### Fixed

- document Cargo shim activation for agents ([#233](https://github.com/jdx/mr-boxington/pull/233))

### Other

- bound remote cache prefetch work ([#239](https://github.com/jdx/mr-boxington/pull/239))

## [0.10.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.9.4...mbx-cache-core-v0.10.0) - 2026-08-30

### Added

- *(cache)* deduplicate in-flight work across runners ([#223](https://github.com/jdx/mr-boxington/pull/223))
- cache Windows links and MSVC compiles ([#224](https://github.com/jdx/mr-boxington/pull/224))
- cache rustdoc actions ([#226](https://github.com/jdx/mr-boxington/pull/226))
- *(cache)* export portable build closures ([#227](https://github.com/jdx/mr-boxington/pull/227))

### Other

- *(cache)* start prediction prefetch earlier ([#220](https://github.com/jdx/mr-boxington/pull/220))
- remove pre-v1 format fallbacks ([#219](https://github.com/jdx/mr-boxington/pull/219))
- share cache path mapping ([#215](https://github.com/jdx/mr-boxington/pull/215))
- *(core)* split cache agent modules ([#217](https://github.com/jdx/mr-boxington/pull/217))

## [0.9.4](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.9.3...mbx-cache-core-v0.9.4) - 2026-08-29

### Other

- recognize Cargo, sccache, and kache ([#205](https://github.com/jdx/mr-boxington/pull/205))

## [0.9.3](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.9.2...mbx-cache-core-v0.9.3) - 2026-08-29

### Other

- lead the landing page with three feature cards ([#198](https://github.com/jdx/mr-boxington/pull/198))
- corrections, editorial rebalance, and a CI anchor check ([#195](https://github.com/jdx/mr-boxington/pull/195))
- *(benchmarks)* demonstrate parallel lint scheduling ([#191](https://github.com/jdx/mr-boxington/pull/191))

## [0.9.2](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.9.1...mbx-cache-core-v0.9.2) - 2026-08-29

### Fixed

- allow caching release-marked builds ([#194](https://github.com/jdx/mr-boxington/pull/194))

## [0.9.1](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.9.0...mbx-cache-core-v0.9.1) - 2026-08-29

### Added

- *(release)* add GNU Linux artifacts ([#181](https://github.com/jdx/mr-boxington/pull/181))

## [0.9.0](https://github.com/jdx/mr-boxington/compare/mbx-cache-core-v0.8.0...mbx-cache-core-v0.9.0) - 2026-08-29

### Fixed

- *(cache-rustc)* predict a native search directory by name, not by its contents ([#162](https://github.com/jdx/mr-boxington/pull/162))

### Other

- keep outputs that already hold the cached bytes ([#165](https://github.com/jdx/mr-boxington/pull/165))
- [**breaking**] stop rehashing inputs the session already read in full ([#164](https://github.com/jdx/mr-boxington/pull/164))

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
