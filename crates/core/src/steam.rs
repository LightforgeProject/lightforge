//! Steam installation discovery.
//!
//! Lightforge targets Proton-GE from a Steam library, so it needs to find the
//! Steam root and enumerate library folders. Both are read-only operations —
//! Lightforge never writes into Steam's configuration.

use std::path::{Path, PathBuf};

/// Candidate Steam roots, in preference order. The `.steam` entries are
/// symlinks maintained by Steam itself and are the most reliable answer when
/// present; the rest cover native and Flatpak installs.
const STEAM_ROOT_CANDIDATES: &[&str] = &[
    ".steam/steam",
    ".steam/root",
    ".local/share/Steam",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
];

/// A located Steam installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamRoot {
    path: PathBuf,
}

impl SteamRoot {
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Find Steam under the given home directory, if installed.
    pub fn discover_in(home: &Path) -> Option<Self> {
        STEAM_ROOT_CANDIDATES
            .iter()
            .map(|c| home.join(c))
            .find(|p| p.join("steamapps").is_dir())
            .map(|p| Self::with_path(p.canonicalize().unwrap_or(p.clone())))
    }

    /// Find Steam under the current user's home directory.
    pub fn discover() -> Option<Self> {
        let dirs = directories::UserDirs::new()?;
        Self::discover_in(dirs.home_dir())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where third-party compatibility tools (Proton-GE and friends) live.
    pub fn compat_tools_dir(&self) -> PathBuf {
        self.path.join("compatibilitytools.d")
    }

    /// Valve's own Proton builds, installed as regular Steam apps.
    pub fn steamapps_common(&self) -> PathBuf {
        self.path.join("steamapps").join("common")
    }

    /// Every library folder Steam knows about, **filtered to those that
    /// currently exist**.
    ///
    /// Library entries routinely point at unmounted drives — external disks,
    /// other distros' partitions. A missing path is normal, not an error.
    pub fn library_paths(&self) -> Vec<PathBuf> {
        let mut out = vec![self.path.clone()];

        for vdf in [
            self.path.join("config").join("libraryfolders.vdf"),
            self.path.join("steamapps").join("libraryfolders.vdf"),
        ] {
            let Ok(contents) = std::fs::read_to_string(&vdf) else {
                continue;
            };
            for path in parse_library_paths(&contents) {
                if !out.contains(&path) {
                    out.push(path);
                }
            }
        }

        out.retain(|p| p.is_dir());
        out
    }
}

/// Extract `"path"` values from a `libraryfolders.vdf`.
///
/// Deliberately a targeted extractor rather than a general KeyValues parser:
/// one field is needed, and a tolerant line scan degrades gracefully if Valve
/// changes the surrounding schema — which they have done before.
fn parse_library_paths(contents: &str) -> Vec<PathBuf> {
    contents
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
            match (parts.next(), parts.next()) {
                (Some("path"), Some(value)) => Some(PathBuf::from(value.replace("\\\\", "/"))),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VDF: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/kenny/.local/share/Steam"
		"label"		""
		"contentid"		"123"
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
		"totalsize"		"2000398934016"
	}
}
"#;

    #[test]
    fn extracts_every_library_path() {
        assert_eq!(
            parse_library_paths(VDF),
            vec![
                PathBuf::from("/home/kenny/.local/share/Steam"),
                PathBuf::from("/mnt/games/SteamLibrary"),
            ]
        );
    }

    #[test]
    fn ignores_other_quoted_fields() {
        // `label` and `contentid` are also quoted pairs; only `path` counts.
        assert!(parse_library_paths("\t\"label\"\t\t\"path\"\n").is_empty());
    }

    #[test]
    fn tolerates_junk() {
        assert!(parse_library_paths("not a vdf at all").is_empty());
        assert!(parse_library_paths("").is_empty());
    }

    #[test]
    fn discovery_requires_a_steamapps_dir() {
        let tmp = std::env::temp_dir().join("lightforge-steam-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".steam/steam")).unwrap();
        // No steamapps/ yet — not a real Steam root.
        assert!(SteamRoot::discover_in(&tmp).is_none());

        std::fs::create_dir_all(tmp.join(".steam/steam/steamapps")).unwrap();
        assert!(SteamRoot::discover_in(&tmp).is_some());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
