//! Proton-GE version model.
//!
//! Policy is **pinned + notify-only**: Lightforge never swaps a working Proton
//! build out from under the user.
//!
//! All products share one prefix, so a pin selects which Proton-GE binary runs
//! at launch rather than isolating a prefix. The stale-shader safety step on a
//! version swap therefore clears only that product's
//! [`crate::paths::AppDirs::shader_cache_dir`].

mod discover;
mod prefix_fit;
mod steam_tool;
mod upstream;
#[cfg(test)]
mod tests;

pub use discover::{
    discover_all, discover_installed, discover_managed, select, wants_tested, TESTED,
};
pub use prefix_fit::{prefix_build, prefix_fit, PrefixFit};
pub use steam_tool::{SteamTool, STEAM_TOOLS};
pub use upstream::{
    check_for_update, fetch_releases, install_release, latest_stable, remove_build, size_on_disk,
    ProtonFamily, ReleaseAsset, UpstreamRelease,
};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use discover::parse_version_file;
// The tests reach the parsers and the unpacker directly.
#[cfg(test)]
use {discover::*, upstream::*};

/// The build WoW's in-game shop is known to work under.
///
/// Newer GE builds start the shop's browser (`Utils/BlizzardBrowser.exe`) but
/// lose its GPU child, the grey box instead of a page. The other half of the
/// fix is the install's own drive ([`crate::paths::Layout::wow_drive`]).
/// Measured 2026-10-02. A recommendation only: other builds play fine
/// without a shop.
pub const SHOP_BUILD: &str = "GE-Proton11-7";

/// Where a given Proton build comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonSource {
    /// Downloaded and unpacked by Lightforge into its own data directory.
    Managed,
    /// An existing install in the user's Steam `compatibilitytools.d`.
    SystemCompatTools,
    /// A Valve Proton installed as a regular Steam app.
    SteamApp,
}

/// Which Proton lineage a build belongs to.
///
/// [`ProtonFlavor::GloriousEggroll`], [`ProtonFlavor::CachyOs`] and Valve's
/// own are the supported launch targets; everything else is detected so the UI can show what is installed and
/// explain why it isn't offered, rather than silently omitting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonFlavor {
    /// Proton-GE, the supported target.
    GloriousEggroll,
    /// CachyOS's Proton. Launches like GE; not what the shop was tested on.
    CachyOs,
    /// Valve's official Proton, including Experimental.
    Valve,
    /// umu's Proton build. Out of scope, umu-launcher is not a runtime target.
    Umu,
    /// Anything else found in `compatibilitytools.d`.
    Other,
}

impl ProtonFlavor {
    /// Classify a build by its version tag.
    ///
    /// Matching is case-insensitive: Valve tags their builds in lowercase
    /// (`proton-11.0-2-x86_64`, `experimental-11.0-20260910b-x86_64`) while
    /// GE and umu use mixed case.
    pub fn classify(version: &str) -> Self {
        let v = version.to_ascii_lowercase();

        // Order matters: umu builds are Proton derivatives and would otherwise
        // match the lineage checks below.
        if v.starts_with("umu-") {
            Self::Umu
        } else if v.starts_with("ge-proton") || v.starts_with("proton-ge") {
            Self::GloriousEggroll
        } else if v.starts_with("cachyos") || v.starts_with("proton-cachyos") {
            Self::CachyOs
        } else if v.starts_with("proton") || v.starts_with("experimental") {
            Self::Valve
        } else {
            Self::Other
        }
    }

    /// Whether Lightforge will launch games with this flavor, when it is
    /// pinned to one.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::GloriousEggroll | Self::CachyOs | Self::Valve)
    }

    /// Whether Lightforge may pick a build of this flavor on its own.
    ///
    /// Narrower than [`ProtonFlavor::is_supported`] on purpose: the other
    /// lineages are launchable because someone chose them, but a machine that
    /// has only Steam's Proton must still be given the tested GE build, and a
    /// missing pin must not quietly land on a lineage nobody tested.
    pub fn is_default_candidate(&self) -> bool {
        matches!(self, Self::GloriousEggroll)
    }
}

/// An installed Proton build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtonRelease {
    /// Upstream release tag, e.g. `GE-Proton11-6`.
    ///
    /// Read from the build's `version` file rather than its directory name,
    /// they disagree in practice (`GE-Proton11-6-x86_64/` ships
    /// `version` = `GE-Proton11-6`), and only the tag matches what the GitHub
    /// Releases API reports.
    pub version: String,
    pub flavor: ProtonFlavor,
    pub source: ProtonSource,
    /// Directory containing the `proton` entry-point script.
    pub path: PathBuf,
    /// Build timestamp from the `version` file, seconds since the epoch.
    pub built_at: Option<u64>,
}

impl ProtonRelease {
    /// Read a build from a compatibility-tool directory.
    ///
    /// Returns `None` when the directory has no `proton` entry point, partial
    /// extractions and stray directories are common in `compatibilitytools.d`.
    pub fn read_from_dir(dir: &Path, source: ProtonSource) -> Option<Self> {
        if !dir.join("proton").is_file() {
            return None;
        }

        let from_file = std::fs::read_to_string(dir.join("version"))
            .ok()
            .and_then(|s| parse_version_file(&s));

        // Fall back to the directory name when `version` is absent, which is
        // the case for Valve's `Proton - Experimental`.
        let (built_at, version) = match from_file {
            Some(parsed) => parsed,
            None => (None, dir.file_name()?.to_string_lossy().into_owned()),
        };

        Some(Self {
            flavor: ProtonFlavor::classify(&version),
            version,
            source,
            path: dir.to_path_buf(),
            built_at,
        })
    }

    /// The `proton` entry-point script invoked to run anything in the prefix.
    pub fn entry_point(&self) -> PathBuf {
        self.path.join("proton")
    }
}

/// Which Proton build a product is pinned to.
///
/// The Battle.net prefix carries its own pin, independent of any WoW product,
/// "latest" is not reliably best for Agent.exe and CEF.
///
/// Serialized as an optional string rather than a tagged enum, so TOML stays a
/// flat `default_proton = "GE-Proton9-20"` line instead of a nested table that
/// would have to be ordered against the `[[products]]` array.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Option<String>", into = "Option<String>")]
pub enum ProtonPin {
    /// Follow the app-wide default.
    #[default]
    Default,
    /// Stay on exactly this release until the user changes it.
    Pinned(String),
}

impl ProtonPin {
    /// The pinned version, or `None` when following the app default.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Pinned(v) => Some(v),
        }
    }
}

impl From<Option<String>> for ProtonPin {
    fn from(value: Option<String>) -> Self {
        match value {
            Some(v) => Self::Pinned(v),
            None => Self::Default,
        }
    }
}

impl From<ProtonPin> for Option<String> {
    fn from(pin: ProtonPin) -> Self {
        match pin {
            ProtonPin::Default => None,
            ProtonPin::Pinned(v) => Some(v),
        }
    }
}

/// A newer upstream release, surfaced passively, never applied automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNotice {
    pub current: String,
    pub available: String,
    pub release_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtonError {
    #[error("{0} is not a build Lightforge installed")]
    NotOurs(PathBuf),

    #[error("{version} is still pinned by {users}")]
    StillPinned { version: String, users: String },

    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error("release {0} publishes no .tar.gz asset for {arch}", arch = std::env::consts::ARCH)]
    NoTarball(String),

    #[error("release {0} publishes no checksum; refusing to install unverified")]
    NoChecksum(String),

    #[error("checksum file {0} is malformed")]
    MalformedChecksum(String),

    #[error("checksum mismatch for {version}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        version: String,
        expected: String,
        actual: String,
    },

    #[error("archive contains an unsafe path: {0}")]
    UnsafeArchivePath(PathBuf),

    #[error("archive {0} is empty")]
    EmptyArchive(PathBuf),

    #[error("{0} is not a Proton build")]
    NotAProtonBuild(PathBuf),

    #[error("extraction task failed: {0}")]
    Join(String),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
