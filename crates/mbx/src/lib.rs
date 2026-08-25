//! mbx: a build cache for Rust projects.
//!
//! Compilations are cached as individual rustc actions in a content-addressed
//! store shared by every project and worktree on a machine, and optionally
//! shared further through a remote cache.
//!
//! This library target exists so the executable and integration tests can
//! share application code; it is not a stable embedding API. Applications
//! should use `mbx-cache-core` or `mbx-cache-rustc` instead.
//!
//! Nothing here carries a compatibility guarantee, and CI's public-API check
//! skips this package for that reason -- see `UNCHECKED_PACKAGES` in
//! `.github/workflows/ci.yml`. Types the CLI alone reads may gain fields in a
//! patch release. Do not restore the check to "protect" these items: it would
//! only force a major bump every time the CLI gains a setting.

pub mod cli;
pub mod config;
pub mod doctor;
pub mod explain;
pub mod policy;
pub(crate) mod savings;
pub mod session;
pub mod store;
pub mod target;
pub mod util;

mod rustc;
