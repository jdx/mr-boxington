//! mbx: a build cache for Rust projects.
//!
//! Compilations are cached as individual rustc actions in a content-addressed
//! store shared by every project and worktree on a machine, and optionally
//! shared further through a remote cache.
//!
//! This library target exists so the executable and integration tests can
//! share application code; it is not a stable embedding API. Applications
//! should use `mbx-cache-core` or `mbx-cache-rustc` instead.

pub mod cli;
pub mod config;
pub mod explain;
pub mod policy;
pub mod session;
pub mod store;
pub mod target;
pub mod util;

mod rustc;
