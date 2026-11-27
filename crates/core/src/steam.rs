//! Steam installation discovery.
//!
//! Read-only; the one write is Lightforge's own library entry
//! ([`crate::steam_shortcut`]), when asked.

use std::path::{Path, PathBuf};

/// Candidate Steam roots, in preference order. The `.steam` entries are
/// symlinks maintained by Steam itself and are the most reliable answer when
/// present; the rest cover native, Flatpak and Snap installs.
const FLATPAK_ID: &str = "com.valvesoftware.Steam";

/// Whether Flathub's Steam may use `flatpak-spawn --host`: its manifest does
/// not grant `org.freedesktop.Flatpak`, so only an override the user made
/// lets it start Lightforge.
pub fn flatpak_reaches_host() -> bool {
    let Some(dirs) = directories::UserDirs::new() else {
        return false;
    };
    let home = dirs.home_dir();
    [
        home.join(".local/share/flatpak/overrides").join(FLATPAK_ID),
        Path::new("/var/lib/flatpak/overrides").join(FLATPAK_ID),
        home.join(".local/share/flatpak/app").join(FLATPAK_ID).join("current/active/metadata"),
        Path::new("/var/lib/flatpak/app").join(FLATPAK_ID).join("current/active/metadata"),
    ]
    .iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .any(|text| text.lines().any(|line| line.trim().replace(' ', "") == "org.freedesktop.Flatpak=talk"))
}

const STEAM_ROOT_CANDIDATES: &[&str] = &[
    ".steam/steam",
    ".steam/root",
    ".local/share/Steam",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    "snap/steam/common/.local/share/Steam",
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
        Self::discover_all_in(home).into_iter().next()
    }

    /// Every Steam under `home`, in preference order: a native and a Flatpak
    /// one can sit side by side, each with its own library.
    pub fn discover_all_in(home: &Path) -> Vec<Self> {
        let mut found: Vec<Self> = Vec::new();
        for path in STEAM_ROOT_CANDIDATES.iter().map(|c| home.join(c)).filter(|p| p.join("steamapps").is_dir()) {
            // `.steam/steam` and `.steam/root` point at one of the others.
            let root = Self::with_path(path.canonicalize().unwrap_or(path));
            if !found.contains(&root) {
                found.push(root);
            }
        }
        found
    }

    /// Find Steam under the current user's home directory.
    pub fn discover() -> Option<Self> {
        let dirs = directories::UserDirs::new()?;
        Self::discover_in(dirs.home_dir())
    }

    /// Every Steam under the current user's home directory.
    pub fn discover_all() -> Vec<Self> {
        directories::UserDirs::new().map_or_else(Vec::new, |dirs| Self::discover_all_in(dirs.home_dir()))
    }

    /// Flathub's Steam, whose sandbox cannot start a program outside it
    /// except through `flatpak-spawn --host`.
    pub fn is_flatpak(&self) -> bool {
        self.path.components().any(|c| c.as_os_str() == FLATPAK_ID)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every library folder Steam knows about **that exists now**: unmounted
    /// drives are normal.
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
/// A line scan for one field, tolerant of schema changes.
pub(crate) fn parse_library_paths(contents: &str) -> Vec<PathBuf> {
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

    #[test]
    fn a_native_and_a_flatpak_steam_are_both_found_once() {
        let home = tempfile::tempdir().unwrap();
        let native = home.path().join(".local/share/Steam");
        let flatpak = home.path().join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        for root in [&native, &flatpak] {
            std::fs::create_dir_all(root.join("steamapps")).unwrap();
        }
        std::fs::create_dir_all(home.path().join(".steam")).unwrap();
        std::os::unix::fs::symlink(&native, home.path().join(".steam/steam")).unwrap();

        let found = SteamRoot::discover_all_in(home.path());

        assert_eq!(found.len(), 2, "{found:?}");
        assert!(!found[0].is_flatpak() && found[1].is_flatpak());
    }

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
        // No steamapps/ yet, not a real Steam root.
        assert!(SteamRoot::discover_in(&tmp).is_none());

        std::fs::create_dir_all(tmp.join(".steam/steam/steamapps")).unwrap();
        assert!(SteamRoot::discover_in(&tmp).is_some());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
