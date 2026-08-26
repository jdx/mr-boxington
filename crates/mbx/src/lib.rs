//! mbx: a build cache for Rust projects.
//!
//! Compilations are cached as individual rustc actions in a content-addressed
//! store shared by every project and worktree on a machine, and optionally
//! shared further through a remote cache.
//!
//! This library target exists so the executable and integration tests can
//! share application code; it is not an embedding API. The supported
//! interfaces are the `mbx` command line -- its subcommands and its versioned
//! JSON output -- and, for anything speaking to a remote cache,
//! `mbx-cache-protocol`. `mbx-cache-core` and `mbx-cache-rustc` are internals
//! too, and say so in their own descriptions.
//!
//! Nothing here carries a compatibility guarantee, and CI's public-API check
//! skips this package for that reason -- see `UNCHECKED_PACKAGES` in
//! `.github/workflows/ci.yml`. Types the CLI alone reads may gain fields in a
//! patch release. Do not restore the check to "protect" these items: it would
//! only force a major bump every time the CLI gains a setting.
//!
//! The modules below are `#[doc(hidden)]` for the same reason. They have to
//! stay `pub` for the binary and the integration tests, but a published crate
//! whose documentation advertises `store` and `session` invites exactly the
//! dependency the paragraph above rules out.

#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod doctor;
#[doc(hidden)]
pub mod explain;
#[doc(hidden)]
pub mod policy;
pub(crate) mod savings;
#[doc(hidden)]
pub mod session;
#[doc(hidden)]
pub mod store;
#[doc(hidden)]
pub mod target;
#[doc(hidden)]
pub mod util;

mod rustc;
