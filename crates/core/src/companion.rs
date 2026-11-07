//! Companion apps: Windows programs that run beside a game in its prefix,
//! such as Wowhead's client or Hearthstone Deck Tracker.
//!
//! Lightforge runs the installer the user picks and starts what it put
//! there; it never reaches into the game. A running companion counts as a
//! game in the prefix ([`crate::preflight::games_running`]), so pre-flight
//! goes beside it and the prefix is not shut down under it when the game
//! exits: an uploader does its work after logout.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::product::ProductCode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Companion {
    /// What the page calls it: the executable's name.
    pub name: String,
    /// Relative to the prefix's `drive_c` when it is inside it, so a moved
    /// prefix keeps it; absolute otherwise.
    pub exe: PathBuf,
    /// The game it goes with: that game's Proton build runs it, and that
    /// game's launch can start it.
    pub game: ProductCode,
    #[serde(default)]
    pub start_with_game: bool,
}

impl Companion {
    pub fn new(drive_c: &Path, exe: &Path, game: ProductCode) -> Self {
        let name = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let exe = exe.strip_prefix(drive_c).map(Path::to_path_buf).unwrap_or_else(|_| exe.to_path_buf());
        Self { name, exe, game, start_with_game: false }
    }

    /// Where the program is on this machine.
    pub fn path(&self, drive_c: &Path) -> PathBuf {
        match self.exe.is_absolute() {
            true => self.exe.clone(),
            false => drive_c.join(&self.exe),
        }
    }
}

/// The folders under `drive_c` installers write programs to; `users` holds
/// `AppData`, where per-user installers (Squirrel, Electron) go.
const PROGRAM_DIRS: [&str; 4] = ["Program Files", "Program Files (x86)", "ProgramData", "users"];

/// Deep enough for `users/<user>/AppData/Local/Programs/<app>/<app>.exe`.
const MAX_DEPTH: usize = 7;

/// Every `.exe` under the folders installers write to, relative to `drive_c`.
///
/// Symlinks are not followed: the WoW link under `Program Files (x86)` leads
/// into a whole game install.
pub fn programs(drive_c: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    for dir in PROGRAM_DIRS {
        walk(drive_c, &drive_c.join(dir), 1, &mut found);
    }
    found
}

fn walk(drive_c: &Path, dir: &Path, depth: usize, found: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() && depth < MAX_DEPTH {
            walk(drive_c, &path, depth + 1, found);
        } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
            if let Ok(relative) = path.strip_prefix(drive_c) {
                found.insert(relative.to_path_buf());
            }
        }
    }
}

/// Folders Wine, Proton and Battle.net fill every prefix with: never a tool
/// of the user's. All 32 programs in a prefix with nothing else added are in
/// one of these (GE-Proton11-7, client 17986, 2026-11-07).
const PREFIX_OWN: [&str; 7] = [
    "Battle.net",
    "Battle.net_components",
    "Common Files",
    "Internet Explorer",
    "Steam",
    "Windows Media Player",
    "Windows NT",
];

/// The programs in the prefix someone put there, not yet in `added`, for a
/// program installed before, or for another game, to be found again.
pub fn found_in_prefix(drive_c: &Path, added: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let theirs: BTreeSet<PathBuf> = programs(drive_c)
        .into_iter()
        .filter(|exe| !exe.components().any(|c| PREFIX_OWN.iter().any(|own| c.as_os_str().eq_ignore_ascii_case(own))))
        .collect();
    installed(added, &theirs)
}

/// Executables an installer drops beside the program, never the program.
const NOT_THE_PROGRAM: [&str; 6] = ["unins", "uninstall", "update", "crashpad", "crashreport", "createdump"];

/// What an installer added between `before` and `after`, without its
/// uninstaller and updater, shallowest first: the program sits above its helpers.
pub fn installed(before: &BTreeSet<PathBuf>, after: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut added: Vec<PathBuf> = after
        .difference(before)
        .filter(|path| {
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            !NOT_THE_PROGRAM.iter().any(|helper| stem.contains(helper))
        })
        .cloned()
        .collect();
    added.sort_by_key(|path| path.components().count());
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_in_the_prefix_is_only_what_someone_put_there() {
        let dir = tempfile::tempdir().unwrap();
        let drive_c = dir.path();
        for exe in [
            "Program Files (x86)/Battle.net/Battle.net.17986/Battle.net.exe",
            "ProgramData/Battle.net/Agent/Agent.9852/Agent.exe",
            "ProgramData/Battle.net_components/battlenet_helpersvc/BlizzardHelper.exe",
            "Program Files (x86)/Internet Explorer/iexplore.exe",
            "Program Files/Windows NT/Accessories/wordpad.exe",
            "Program Files (x86)/Steam/steam.exe",
            "users/steamuser/AppData/Local/HearthstoneDeckTracker/HearthstoneDeckTracker.exe",
            "users/steamuser/AppData/Local/HearthstoneDeckTracker/Update.exe",
            "Program Files/Wowhead Client/Wowhead Client.exe",
        ] {
            std::fs::create_dir_all(drive_c.join(exe).parent().unwrap()).unwrap();
            std::fs::write(drive_c.join(exe), b"MZ").unwrap();
        }
        let added: BTreeSet<PathBuf> = [PathBuf::from("Program Files/Wowhead Client/Wowhead Client.exe")].into();

        assert_eq!(
            found_in_prefix(drive_c, &added),
            [PathBuf::from("users/steamuser/AppData/Local/HearthstoneDeckTracker/HearthstoneDeckTracker.exe")]
        );
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"MZ").unwrap();
    }

    #[test]
    fn an_installer_is_seen_by_what_it_added() {
        let dir = tempfile::tempdir().unwrap();
        let drive_c = dir.path();
        touch(drive_c, "Program Files (x86)/Battle.net/Battle.net.exe");
        let before = programs(drive_c);

        // A per-user Electron install, as Squirrel lays it out.
        touch(drive_c, "users/steamuser/AppData/Local/Uploader/Update.exe");
        touch(drive_c, "users/steamuser/AppData/Local/Uploader/app-1.2.0/Uploader.exe");
        touch(drive_c, "users/steamuser/AppData/Local/Uploader/app-1.2.0/Uploader Uninstall.exe");
        touch(drive_c, "users/steamuser/AppData/Local/Uploader/app-1.2.0/resources/elevate.exe");
        let after = programs(drive_c);

        assert_eq!(
            installed(&before, &after),
            [
                PathBuf::from("users/steamuser/AppData/Local/Uploader/app-1.2.0/Uploader.exe"),
                PathBuf::from("users/steamuser/AppData/Local/Uploader/app-1.2.0/resources/elevate.exe"),
            ]
        );
    }

    #[test]
    fn the_game_behind_its_link_is_not_walked() {
        let dir = tempfile::tempdir().unwrap();
        let games = dir.path().join("games/World of Warcraft/_retail_");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(games.join("Wow.exe"), b"MZ").unwrap();
        let drive_c = dir.path().join("drive_c");
        std::fs::create_dir_all(drive_c.join("Program Files (x86)")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("games/World of Warcraft"), drive_c.join("Program Files (x86)/World of Warcraft"))
            .unwrap();

        assert!(programs(&drive_c).is_empty());
    }

    #[test]
    fn a_program_in_the_prefix_is_kept_relative_to_it() {
        let drive_c = Path::new("/games/prefix/pfx/drive_c");
        let inside = Companion::new(drive_c, &drive_c.join("Program Files/Tracker/Tracker.exe"), ProductCode::Retail);
        assert_eq!(inside.name, "Tracker");
        assert_eq!(inside.exe, PathBuf::from("Program Files/Tracker/Tracker.exe"));
        assert_eq!(inside.path(Path::new("/moved/pfx/drive_c")), Path::new("/moved/pfx/drive_c/Program Files/Tracker/Tracker.exe"));

        let outside = Companion::new(drive_c, Path::new("/home/u/Tools/Portable.exe"), ProductCode::Retail);
        assert_eq!(outside.path(drive_c), Path::new("/home/u/Tools/Portable.exe"));
    }
}
