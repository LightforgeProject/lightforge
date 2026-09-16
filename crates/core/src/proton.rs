//! Proton-GE version model.
//!
//! Policy is **pinned + notify-only**: Lightforge never swaps a working Proton
//! build out from under the user. Discovery, download, and launching land in
//! Milestones 2 and 7; Milestone 1 fixes the model the whole feature hangs off.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// GitHub repository Proton-GE releases are polled from.
pub const GE_RELEASES_REPO: &str = "GloriousEggroll/proton-ge-custom";

/// Where a given Proton-GE build comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ProtonSource {
    /// Downloaded and unpacked by Lightforge into its own data directory.
    Managed,
    /// An existing install in the user's Steam `compatibilitytools.d`.
    SystemCompatTools { path: PathBuf },
}

/// An installed Proton-GE build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtonRelease {
    /// Release tag as published, e.g. `GE-Proton9-20`.
    pub version: String,
    pub source: ProtonSource,
    /// Directory containing the `proton` entry-point script.
    pub path: PathBuf,
}

/// Which Proton build a product is pinned to.
///
/// The Battle.net prefix carries its own pin, independent of any WoW product —
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

/// A newer upstream release, surfaced passively — never applied automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNotice {
    pub current: String,
    pub available: String,
    pub release_url: String,
}
