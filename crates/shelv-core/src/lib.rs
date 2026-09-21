//! Shelv core.
//!
//! Everything in this crate is free of GUI and Tauri dependencies so that the
//! engine can be exercised by tests, and later by a CLI or a Linux build,
//! without a window. `src-tauri` is a thin shell over this crate.
//!
//! Platform-specific code is confined to [`platform`] and [`cloud`]; see
//! `docs/PLAN.md` §2.6. CI enforces that boundary.

pub mod cloud;
pub mod engine;
pub mod error;
pub mod model;
pub mod platform;
pub mod safety;
pub mod scheduler;
pub mod store;
pub mod view;
pub mod volumes;
pub mod watch;

pub use error::{CoreError, Result};
