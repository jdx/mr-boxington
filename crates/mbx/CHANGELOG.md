# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.5.0](https://github.com/jdx/mr-boxington/compare/v1.4.1...v1.5.0) - 2026-09-02

### Added

- *(cc)* cache preprocessed assembly ([#291](https://github.com/jdx/mr-boxington/pull/291))
- *(cc)* write the caller's dependency list instead of bypassing it ([#294](https://github.com/jdx/mr-boxington/pull/294))

### Fixed

- scale the stale-manifest note to what was predicted ([#289](https://github.com/jdx/mr-boxington/pull/289))
- *(remote)* bound what a build loses to failed cache reads ([#292](https://github.com/jdx/mr-boxington/pull/292))
- keep learned incremental state for large crates ([#282](https://github.com/jdx/mr-boxington/pull/282))
- *(setup)* preserve native compiler path when disabled ([#281](https://github.com/jdx/mr-boxington/pull/281))

### Other

- carry the file-digest ledger across sessions ([#285](https://github.com/jdx/mr-boxington/pull/285))

## [1.4.1](https://github.com/jdx/mr-boxington/compare/v1.4.0...v1.4.1) - 2026-09-02

### Other

- trim the README and correct link caching claims ([#277](https://github.com/jdx/mr-boxington/pull/277))

## [1.4.0](https://github.com/jdx/mr-boxington/compare/v1.3.2...v1.4.0) - 2026-09-02

### Added

- explain cache misses from session history ([#274](https://github.com/jdx/mr-boxington/pull/274))
- add quiet build summaries ([#264](https://github.com/jdx/mr-boxington/pull/264))
- *(scheduler)* respect build CPU limits ([#267](https://github.com/jdx/mr-boxington/pull/267))

### Fixed

- avoid racing persistent Windows shims ([#276](https://github.com/jdx/mr-boxington/pull/276))
- fix managed target lifecycle edges ([#269](https://github.com/jdx/mr-boxington/pull/269))
- release cache pinned by phantom checkouts ([#270](https://github.com/jdx/mr-boxington/pull/270))
- fix build-script caching and doctor toolchain ([#272](https://github.com/jdx/mr-boxington/pull/272))
- *(setup)* isolate rust-analyzer target directory ([#261](https://github.com/jdx/mr-boxington/pull/261))
- cache clippy workspace compilations ([#273](https://github.com/jdx/mr-boxington/pull/273))
- reuse enclosing sessions for nested cargo ([#265](https://github.com/jdx/mr-boxington/pull/265))
- handle rustc workspace wrappers ([#263](https://github.com/jdx/mr-boxington/pull/263))
- *(cache)* map symlinked Cargo registries ([#259](https://github.com/jdx/mr-boxington/pull/259))

### Other

- preserve Cargo rustc probe cache ([#268](https://github.com/jdx/mr-boxington/pull/268))
- make workspace edit loops incremental ([#271](https://github.com/jdx/mr-boxington/pull/271))

## [1.3.2](https://github.com/jdx/mr-boxington/compare/v1.3.1...v1.3.2) - 2026-09-01

### Fixed

- share predictions across Cargo commands ([#256](https://github.com/jdx/mr-boxington/pull/256))

## [1.3.1](https://github.com/jdx/mr-boxington/compare/v1.3.0...v1.3.1) - 2026-09-01

### Fixed

- avoid reentering mise Cargo wrappers ([#252](https://github.com/jdx/mr-boxington/pull/252))

## [1.3.0](https://github.com/jdx/mr-boxington/compare/v1.2.0...v1.3.0) - 2026-08-31

### Added

- *(setup)* use mise command wrappers ([#249](https://github.com/jdx/mr-boxington/pull/249))

### Other

- cache build scripts with Cargo's default inputs ([#245](https://github.com/jdx/mr-boxington/pull/245))

## [1.2.0](https://github.com/jdx/mr-boxington/compare/v1.1.0...v1.2.0) - 2026-08-31

### Added

- support Mercurial and Sapling checkouts ([#232](https://github.com/jdx/mr-boxington/pull/232))

### Fixed

- *(setup)* prevent Cargo shim recursion after HOME changes ([#234](https://github.com/jdx/mr-boxington/pull/234))
- document Cargo shim activation for agents ([#233](https://github.com/jdx/mr-boxington/pull/233))
- support Delta worktrees ([#231](https://github.com/jdx/mr-boxington/pull/231))

### Other

- bound remote cache prefetch work ([#239](https://github.com/jdx/mr-boxington/pull/239))

## [1.1.0](https://github.com/jdx/mr-boxington/compare/v1.0.1...v1.1.0) - 2026-08-30

### Added

- cache build script execution ([#225](https://github.com/jdx/mr-boxington/pull/225))
- *(cache)* deduplicate in-flight work across runners ([#223](https://github.com/jdx/mr-boxington/pull/223))
- cache Windows links and MSVC compiles ([#224](https://github.com/jdx/mr-boxington/pull/224))
- cache rustdoc actions ([#226](https://github.com/jdx/mr-boxington/pull/226))
- *(cache)* export portable build closures ([#227](https://github.com/jdx/mr-boxington/pull/227))
- *(mbx)* prescribe fixes for cache bypasses ([#222](https://github.com/jdx/mr-boxington/pull/222))

### Fixed

- *(rustc)* compact large action predictions ([#218](https://github.com/jdx/mr-boxington/pull/218))

### Other

- remove pre-v1 format fallbacks ([#219](https://github.com/jdx/mr-boxington/pull/219))
- *(mbx)* split session responsibilities ([#216](https://github.com/jdx/mr-boxington/pull/216))
- share cache path mapping ([#215](https://github.com/jdx/mr-boxington/pull/215))
- *(mbx)* split CLI commands into modules ([#212](https://github.com/jdx/mr-boxington/pull/212))

## [1.0.1](https://github.com/jdx/mr-boxington/compare/v1.0.0...v1.0.1) - 2026-08-29

### Fixed

- support Jujutsu repositories in mbx exec ([#207](https://github.com/jdx/mr-boxington/pull/207))

### Other

- recognize Cargo, sccache, and kache ([#205](https://github.com/jdx/mr-boxington/pull/205))

## [1.0.0](https://github.com/jdx/mr-boxington/compare/v0.7.2...v1.0.0) - 2026-08-29

### Other

- lead the landing page with three feature cards ([#198](https://github.com/jdx/mr-boxington/pull/198))
- cache linked proc macros ([#197](https://github.com/jdx/mr-boxington/pull/197))
- corrections, editorial rebalance, and a CI anchor check ([#195](https://github.com/jdx/mr-boxington/pull/195))
- *(benchmarks)* demonstrate parallel lint scheduling ([#191](https://github.com/jdx/mr-boxington/pull/191))

## [0.7.2](https://github.com/jdx/mr-boxington/compare/v0.7.1...v0.7.2) - 2026-08-29

### Fixed

- allow caching release-marked builds ([#194](https://github.com/jdx/mr-boxington/pull/194))

## [0.7.1](https://github.com/jdx/mr-boxington/compare/v0.7.0...v0.7.1) - 2026-08-29

### Added

- *(release)* add GNU Linux artifacts ([#181](https://github.com/jdx/mr-boxington/pull/181))
- *(rustc)* cache native links by default ([#178](https://github.com/jdx/mr-boxington/pull/178))
- *(rustc)* cache the compilations that never link ([#177](https://github.com/jdx/mr-boxington/pull/177))
- a machine-wide, memory-aware compiler scheduler ([#170](https://github.com/jdx/mr-boxington/pull/170))

### Fixed

- *(cc)* make an object independent of the directory it was built in ([#185](https://github.com/jdx/mr-boxington/pull/185))
- *(cc)* say what diverged, and describe when C objects legitimately do ([#182](https://github.com/jdx/mr-boxington/pull/182))

### Other

- *(scheduler)* drop the release watch, which measured as nothing ([#183](https://github.com/jdx/mr-boxington/pull/183))
- *(scheduler)* weigh an unmeasured link by what this machine's links cost ([#180](https://github.com/jdx/mr-boxington/pull/180))
- share OUT_DIR artifacts across worktrees ([#176](https://github.com/jdx/mr-boxington/pull/176))
- *(scheduler)* retire the link guess, and wake waiters on release ([#174](https://github.com/jdx/mr-boxington/pull/174))
- reduce mbx startup relocations ([#175](https://github.com/jdx/mr-boxington/pull/175))

## [0.7.0](https://github.com/jdx/mr-boxington/compare/v0.6.0...v0.7.0) - 2026-08-29

### Added

- *(cache-rustc)* cache macOS debug links behind an oso_prefix the shim appends ([#166](https://github.com/jdx/mr-boxington/pull/166))
- *(cli)* read the toolchain instead of forwarding it ([#159](https://github.com/jdx/mr-boxington/pull/159))

### Fixed

- *(cache-rustc)* predict a native search directory by name, not by its contents ([#162](https://github.com/jdx/mr-boxington/pull/162))

### Other

- keep outputs that already hold the cached bytes ([#165](https://github.com/jdx/mr-boxington/pull/165))
- [**breaking**] stop rehashing inputs the session already read in full ([#164](https://github.com/jdx/mr-boxington/pull/164))
- *(doctor)* reach the failure line without spawning a process ([#163](https://github.com/jdx/mr-boxington/pull/163))

## [0.6.0](https://github.com/jdx/mr-boxington/compare/v0.5.4...v0.6.0) - 2026-08-28

### Fixed

- *(cc)* [**breaking**] keep shim diagnostics off the intercepted compiler's stderr ([#154](https://github.com/jdx/mr-boxington/pull/154))
- *(mbx)* bump the stats report version for the new field ([#157](https://github.com/jdx/mr-boxington/pull/157))
- *(cache-rustc)* key inert native search directories by path ([#153](https://github.com/jdx/mr-boxington/pull/153))

### Other

- stop rereading cached artifacts on warm hits ([#152](https://github.com/jdx/mr-boxington/pull/152))

## [0.5.4](https://github.com/jdx/mr-boxington/compare/v0.5.3...v0.5.4) - 2026-08-28

### Fixed

- *(cc)* keep compiler shims stable across sessions ([#150](https://github.com/jdx/mr-boxington/pull/150))

### Other

- release ([#149](https://github.com/jdx/mr-boxington/pull/149))

## [0.5.3](https://github.com/jdx/mr-boxington/compare/v0.5.2...v0.5.3) - 2026-08-28

### Added

- *(cc)* cache the C and C++ a cross build compiles ([#143](https://github.com/jdx/mr-boxington/pull/143))
- add Windows ARM64 support ([#147](https://github.com/jdx/mr-boxington/pull/147))

### Fixed

- *(exec)* never let a shim stand in for the compiler it shims ([#144](https://github.com/jdx/mr-boxington/pull/144))

### Other

- hand the shim search a PATH instead of setting one ([#146](https://github.com/jdx/mr-boxington/pull/146))

## [0.5.2](https://github.com/jdx/mr-boxington/compare/v0.5.1...v0.5.2) - 2026-08-27

### Added

- cache standalone C and C++ builds through mbx exec ([#138](https://github.com/jdx/mr-boxington/pull/138))
- cache C and C++ compiles from build scripts ([#132](https://github.com/jdx/mr-boxington/pull/132))
- cache natively linked test binaries ([#129](https://github.com/jdx/mr-boxington/pull/129))
- cache to an S3-compatible object store ([#140](https://github.com/jdx/mr-boxington/pull/140))
- *(cache)* expose shared Cargo cache integration ([#130](https://github.com/jdx/mr-boxington/pull/130))
- batch remote action lookups and blob uploads ([#131](https://github.com/jdx/mr-boxington/pull/131))
- publish remote objects after the build asks for them ([#126](https://github.com/jdx/mr-boxington/pull/126))
- compile churning crates incrementally ([#127](https://github.com/jdx/mr-boxington/pull/127))
- mbx tui, a live view of every build's cache activity ([#128](https://github.com/jdx/mr-boxington/pull/128))

### Fixed

- *(rustc)* restore results into the checkout that asked for them ([#141](https://github.com/jdx/mr-boxington/pull/141))
- symlink the session shim so macOS cannot kill it at exec ([#134](https://github.com/jdx/mr-boxington/pull/134))

### Other

- define download_timeout as a whole-download deadline ([#142](https://github.com/jdx/mr-boxington/pull/142))

## [0.5.1](https://github.com/jdx/mr-boxington/compare/v0.5.0...v0.5.1) - 2026-08-26

### Fixed

- cache libraries with native search paths ([#120](https://github.com/jdx/mr-boxington/pull/120))

## [0.5.0](https://github.com/jdx/mr-boxington/compare/v0.4.0...v0.5.0) - 2026-08-26

### Added

- [**breaking**] open the extensible public types to extension ([#103](https://github.com/jdx/mr-boxington/pull/103))
- count remote cache failures in the summary ([#112](https://github.com/jdx/mr-boxington/pull/112))
- make landing demo interactive and tag output ([#94](https://github.com/jdx/mr-boxington/pull/94))

### Other

- document MBX_LOG and how to report a problem ([#99](https://github.com/jdx/mr-boxington/pull/99))
- simplify the landing page for 1.0 ([#96](https://github.com/jdx/mr-boxington/pull/96))
- let the agent's statistics grow without breaking ([#114](https://github.com/jdx/mr-boxington/pull/114))
- give every published crate its crates.io metadata ([#97](https://github.com/jdx/mr-boxington/pull/97))
- version each crate by what it promises ([#100](https://github.com/jdx/mr-boxington/pull/100))

### Security

- bound remote downloads and protect release assets ([#109](https://github.com/jdx/mr-boxington/pull/109))

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
