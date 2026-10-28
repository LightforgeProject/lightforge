//! Proton-GE version model.
//!
//! Policy is **pinned + notify-only**: Lightforge never swaps a working Proton
//! build out from under the user.
//!
//! All products share one prefix, so a pin only picks which Proton binary
//! runs at launch. A version swap clears just that product's
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

/// The build WoW's in-game shop works under (measured 2026-10-02). Newer GE
/// builds start `Utils/BlizzardBrowser.exe` but lose its GPU child: a grey
/// box. The other half of the fix is [`crate::paths::Layout::wow_drive`].
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
/// GE, CachyOS and Valve's own are launch targets; the rest are detected so
/// the UI can say why they are not offered.
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
    /// Classify a build by its version tag, ignoring case: Valve's are lower
    /// case (`proton-11.0-2-x86_64`), GE's and umu's mixed.
    pub fn classify(version: &str) -> Self {
        let v = version.to_ascii_lowercase();

        // First: umu builds would also match the checks below.
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

    /// Whether Lightforge may pick a build of this flavor on its own. Narrower
    /// than [`ProtonFlavor::is_supported`]: the other lineages need a pin.
    pub fn is_default_candidate(&self) -> bool {
        matches!(self, Self::GloriousEggroll)
    }
}

/// An installed Proton build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtonRelease {
    /// Upstream release tag, e.g. `GE-Proton11-6`, from the `version` file:
    /// the directory is `GE-Proton11-6-x86_64/`, and only the tag matches
    /// GitHub's releases.
    pub version: String,
    pub flavor: ProtonFlavor,
    pub source: ProtonSource,
    /// Directory containing the `proton` entry-point script.
    pub path: PathBuf,
    /// Build timestamp from the `version` file, seconds since the epoch.
    pub built_at: Option<u64>,
}

impl ProtonRelease {
    /// Read a build from a compatibility-tool directory; `None` without a
    /// `proton` script (stray and half-extracted folders are common).
    pub fn read_from_dir(dir: &Path, source: ProtonSource) -> Option<Self> {
        if !dir.join("proton").is_file() {
            return None;
        }

        let from_file = std::fs::read_to_string(dir.join("version"))
            .ok()
            .and_then(|s| parse_version_file(&s));

        // Valve's `Proton - Experimental` has no `version` file.
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

/// Which Proton build a product is pinned to. Battle.net has its own pin:
/// "latest" is not reliably best for Agent and CEF.
///
/// Serialized as an optional string, so the TOML is one flat line
/// (`default_proton = "GE-Proton9-20"`), not a table to order around `[[products]]`.
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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ProtonError + '_ {
    move |source| ProtonError::Io { path: path.to_path_buf(), source }
}
