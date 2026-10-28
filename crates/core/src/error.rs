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

    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error(transparent)]
    Proton(#[from] crate::proton::ProtonError),

    #[error(transparent)]
    Prefix(#[from] crate::prefix::PrefixError),

    #[error(transparent)]
    BattleNet(#[from] crate::battlenet::BattleNetError),

    #[error(transparent)]
    Preflight(#[from] crate::preflight::PreflightError),

    #[error(transparent)]
    Backup(#[from] crate::backup::BackupError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Render an error together with everything that caused it, as one line.
///
/// `thiserror`'s `Display` drops the cause; walking `source` puts it back:
///
/// ```text
/// i/o error at /home/u/games/World of Warcraft: Permission denied (os error 13)
/// ```
///
/// Used where an error becomes a string.
pub fn chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        // `#[error(transparent)]` wraps without adding anything of its own, and
        // a format that already names its cause would say it twice.
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cause_is_appended_once_however_deep_it_sits() {
        let error = CoreError::Io {
            path: "/games/World of Warcraft".into(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        assert_eq!(
            chain(&error),
            "i/o error at /games/World of Warcraft: permission denied"
        );
    }

    #[test]
    fn a_message_that_already_carries_its_cause_does_not_repeat_it() {
        #[derive(Debug, thiserror::Error)]
        #[error("cannot read {path}: {source}")]
        struct Verbose {
            path: &'static str,
            source: std::io::Error,
        }

        let error = Verbose {
            path: "/tmp/x",
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert_eq!(chain(&error), "cannot read /tmp/x: entity not found");
    }
}
