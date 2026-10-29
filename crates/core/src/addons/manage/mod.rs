//! Install, update and remove addons end to end.
//!
//! Resolve, fetch, extract and record, reporting through [`Event`]s. One
//! addon failing (an opt-out, a deleted project) never stops the rest.

mod adopt;
mod fetch;
mod settings;
mod updates;
#[cfg(test)]
mod tests;

pub use adopt::{adopt, Adopted};
pub use fetch::{install_all, resolve_everywhere, update_all};
pub use settings::{
    remove, requests_from, set_auto_update, set_channel, set_ignored, set_pinned, unsupported,
};
pub use updates::{auto_update_requests, check_updates, update_requests, UpdateStatus};

use std::path::{Path, PathBuf};

use super::catalog::CatalogError;
use super::install::{InstallError, Ledger};
use crate::backup::BackupError;
// The tests reach the ledger writer, the pin rule and the adoption grouping.
#[cfg(test)]
use {adopt::*, fetch::*, updates::*};

/// Where an operation writes, and what it is for.
#[derive(Debug, Clone)]
pub struct Options {
    /// Catalog flavor: `retail`, `classic`, `classic_era`.
    pub flavor: String,
    /// The product's `Interface/AddOns` directory.
    pub addons_dir: PathBuf,
    /// Where this product's ledger lives.
    pub ledger_path: PathBuf,
    /// Install the file each request names and hold the addon there, instead
    /// of the newest: a version the user picked. Never for an import.
    pub pinned: bool,
    /// Install even when the ledger says the addon is already on this file:
    /// the ledger cannot tell whether its folders are intact. "Re-install".
    pub reinstall: bool,
    /// Which builds to accept for an addon that has no opinion of its own.
    pub channel: super::Channel,
    /// Back up the game's settings before the first installed addon is
    /// replaced. `None` for callers with no game to protect, or none to ask.
    pub backup: Option<crate::backup::Plan>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Resolving { count: usize },
    Resolved { ok: usize, unavailable: usize },
    Downloading { name: String },
    Installed { name: String, folders: Vec<String> },
    /// Already at this version; nothing fetched.
    UpToDate { name: String },
    /// Could not be installed, and why, opt-out, missing, or a failed fetch.
    Unavailable { name: String, reason: String },
}

/// An addon that could not be installed, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub name: String,
    pub reason: String,
    /// The project page: for an opt-out, the only way to get the addon.
    pub website_url: Option<String>,
    /// `owner/repo` of the author's own GitHub releases, for an opt-out whose
    /// project names one. See [`crate::addons::Resolved::github_repo`].
    pub github: Option<String>,
}

/// What a run achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub installed: Vec<String>,
    pub up_to_date: Vec<String>,
    pub unavailable: Vec<Unavailable>,
}

impl Report {
    /// True when every addon asked for is now present at the wanted version.
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_empty()
    }
}

/// The ledger path for one install of a product.
pub fn ledger_path(
    data_dir: &Path,
    addons_dir: &Path,
    code: &crate::product::ProductCode,
) -> PathBuf {
    Ledger::path_for(data_dir, addons_dir, code)
}

#[derive(Debug, thiserror::Error)]
pub enum ManageError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),

    #[error(transparent)]
    Install(#[from] InstallError),

    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error("the catalog reported this addon as available but gave no download URL")]
    NoDownload,

    #[error("{name} downloaded as {actual}, expected {expected} — the package is damaged")]
    Checksum { name: String, expected: String, actual: String },

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not back up the game's settings first, so nothing was replaced")]
    Backup(#[from] BackupError),

    #[error("the settings backup stopped unexpectedly: {0}")]
    Join(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ManageError + '_ {
    move |source| ManageError::Io { path: path.to_path_buf(), source }
}
