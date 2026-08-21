//! mbx: a build cache for Rust projects.
//!
//! Compilations are cached as individual rustc actions in a content-addressed
//! store shared by every project and worktree on a machine, and optionally
//! shared further through a remote cache.

pub mod cli;
pub mod config;
pub mod policy;
pub mod session;
pub mod store;
pub mod target;
pub mod util;

mod rustc;
