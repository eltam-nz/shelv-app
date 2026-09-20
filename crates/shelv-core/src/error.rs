//! The single error type crossing out of the core.
//!
//! Variants are deliberately coarse: the UI needs to tell the user what to do,
//! not to match on every failure mode. Detail belongs in the message and in the
//! per-file `run_event` rows.

use serde::Serialize;

/// Errors surfaced by the core to its callers.
#[derive(Debug, thiserror::Error, Serialize, ts_rs::TS)]
// `export_to` is relative to ts-rs's default `<crate>/bindings` directory,
// not the crate root: three levels up lands at the repo root.
#[ts(export, export_to = "../../../src/types/")]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum CoreError {
    /// The requested rule, tag, destination or run does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The caller supplied something the core will not accept, such as a
    /// destination inside its own source.
    #[error("invalid request: {0}")]
    Invalid(String),

    /// A guard in `safety` refused the operation. Distinct from `Invalid`
    /// because these are refusals to protect data, and the UI should say so.
    #[error("refused: {0}")]
    Refused(String),

    /// The volume a rule depends on is not currently attached, or its identity
    /// does not match the one recorded on the rule.
    #[error("volume unavailable: {0}")]
    VolumeUnavailable(String),

    /// Filesystem or OS failure. Carries the path where one is known.
    #[error("io error: {0}")]
    Io(String),

    /// Persistence failure.
    #[error("store error: {0}")]
    Store(String),
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Io(e.to_string())
    }
}

/// Result alias used throughout the core.
pub type Result<T> = std::result::Result<T, CoreError>;
