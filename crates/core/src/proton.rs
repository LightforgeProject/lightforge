//! Proton-GE version model.
//!
//! Policy is **pinned + notify-only**: Lightforge never swaps a working Proton
//! build out from under the user. Discovery, download, and launching land in
//! Milestones 2 and 7; Milestone 1 fixes the model the whole feature hangs off.
//!
//! All products share one prefix, so a pin selects which Proton-GE binary runs
//! at launch rather than isolating a prefix. The stale-shader safety step on a
//! version swap therefore clears only that product's
//! [`crate::paths::AppDirs::shader_cache_dir`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// GitHub repository Proton-GE releases are polled from.
pub const GE_RELEASES_REPO: &str = "GloriousEggroll/proton-ge-custom";

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
/// Only [`ProtonFlavor::GloriousEggroll`] is a supported launch target;
/// everything else is detected so the UI can show what is installed and
/// explain why it isn't offered, rather than silently omitting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonFlavor {
    /// Proton-GE, the supported target.
    GloriousEggroll,
    /// Valve's official Proton, including Experimental.
    Valve,
    /// umu's Proton build. Out of scope — umu-launcher is not a runtime target.
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
        } else if v.starts_with("proton") || v.starts_with("experimental") {
            Self::Valve
        } else {
            Self::Other
        }
    }

    /// Whether Lightforge will launch games with this flavor.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::GloriousEggroll)
    }
}

/// An installed Proton build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtonRelease {
    /// Upstream release tag, e.g. `GE-Proton11-6`.
    ///
    /// Read from the build's `version` file rather than its directory name —
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
    /// Returns `None` when the directory has no `proton` entry point — partial
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

/// Parse a Proton `version` file: `<unix timestamp> <tag>`.
fn parse_version_file(contents: &str) -> Option<(Option<u64>, String)> {
    let line = contents.lines().next()?.trim();
    match line.split_once(char::is_whitespace) {
        Some((ts, tag)) => Some((ts.parse().ok(), tag.trim().to_owned())),
        // Some builds ship only the tag.
        None if !line.is_empty() => Some((None, line.to_owned())),
        None => None,
    }
}

/// Every Proton build installed on the system, across all Steam libraries.
///
/// Unsupported flavors are included; filter with [`ProtonFlavor::is_supported`]
/// at the point of use.
pub fn discover_installed(steam: &crate::steam::SteamRoot) -> Vec<ProtonRelease> {
    let mut found = Vec::new();

    for library in steam.library_paths() {
        let sources = [
            (library.join("compatibilitytools.d"), ProtonSource::SystemCompatTools),
            (library.join("steamapps").join("common"), ProtonSource::SteamApp),
        ];

        for (dir, source) in sources {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                if let Some(release) = ProtonRelease::read_from_dir(&entry.path(), source) {
                    found.push(release);
                }
            }
        }
    }

    found.sort_by(|a, b| a.version.cmp(&b.version));
    found.dedup_by(|a, b| a.version == b.version);
    found
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_real_world_tags() {
        // Tags observed on a live system.
        assert_eq!(ProtonFlavor::classify("GE-Proton11-6"), ProtonFlavor::GloriousEggroll);
        assert_eq!(ProtonFlavor::classify("GE-Proton10-34"), ProtonFlavor::GloriousEggroll);
        assert_eq!(ProtonFlavor::classify("UMU-Proton-10.0-4"), ProtonFlavor::Umu);
        assert_eq!(ProtonFlavor::classify("proton-11.0-2-x86_64"), ProtonFlavor::Valve);
        assert_eq!(
            ProtonFlavor::classify("experimental-11.0-20260910b-x86_64"),
            ProtonFlavor::Valve
        );
    }

    #[test]
    fn only_ge_is_a_launch_target() {
        assert!(ProtonFlavor::GloriousEggroll.is_supported());
        assert!(!ProtonFlavor::Umu.is_supported());
        assert!(!ProtonFlavor::Valve.is_supported());
        assert!(!ProtonFlavor::Other.is_supported());
    }

    #[test]
    fn version_file_splits_timestamp_from_tag() {
        assert_eq!(
            parse_version_file("1787951532 GE-Proton11-6\n"),
            Some((Some(1787951532), "GE-Proton11-6".to_owned()))
        );
    }

    #[test]
    fn version_file_tolerates_a_bare_tag() {
        assert_eq!(parse_version_file("GE-Proton9-20"), Some((None, "GE-Proton9-20".to_owned())));
        assert_eq!(parse_version_file(""), None);
    }

    #[test]
    fn pin_round_trips_through_an_optional_string() {
        assert_eq!(ProtonPin::from(None), ProtonPin::Default);
        assert_eq!(ProtonPin::Default.version(), None);
        assert_eq!(ProtonPin::from(Some("GE-Proton11-6".to_owned())).version(), Some("GE-Proton11-6"));
    }
}
