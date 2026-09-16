//! Error types for `core`.
//!
//! Each submodule owns a `thiserror` enum describing what can actually go
//! wrong there; [`CoreError`] is the aggregate the crate boundary exposes.
//! Application-level orchestration (CLI/GUI) wraps these in `anyhow`.

use std::path::PathBuf;

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Paths(#[from] crate::paths::PathError),

    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    #[error(transparent)]
    BuildInfo(#[from] crate::buildinfo::BuildInfoError),

    #[error(transparent)]
    Product(#[from] crate::product::ProductError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl CoreError {
    /// Attach the offending path to an [`std::io::Error`].
    ///
    /// Bare io errors say *what* failed but never *which file*, which is the
    /// half that actually helps a user debugging a broken prefix.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
