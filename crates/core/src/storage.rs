//! Where Lightforge's bytes live, how many there are, and what is safe to drop.
//!
//! Read-only apart from [`clear_cache`], which only ever touches Lightforge's
//! own cache directory. Game files and the prefix are measured, never deleted
//! from here.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::paths::{AppDirs, Layout};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("cannot read free space for {path}: {source}")]
    Disk { path: PathBuf, source: std::io::Error },
    #[error("cannot remove {path}: {source}")]
    Remove { path: PathBuf, source: std::io::Error },
}

/// Capacity of the filesystem a path lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Disk {
    pub total: u64,
    /// Available to an unprivileged user — what a download can actually use,
    /// not counting blocks reserved for root.
    pub free: u64,
}

/// One thing taking up space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub label: &'static str,
    pub path: PathBuf,
    pub bytes: u64,
    /// Whether [`clear_cache`] removes it.
    pub clearable: bool,
}

/// Everything the Storage section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub root: PathBuf,
    /// `None` when the filesystem could not be queried; the rest still stands.
    pub disk: Option<Disk>,
    pub usage: Vec<Usage>,
}

/// Measure every location Lightforge writes to. Walks the game install, so it
/// belongs on a blocking thread.
pub fn report(layout: &Layout, dirs: &AppDirs) -> Report {
    let item = |label, path: PathBuf, clearable| Usage {
        label,
        bytes: dir_size(&path),
        path,
        clearable,
    };
    Report {
        root: layout.root().to_path_buf(),
        disk: disk(layout.root()).ok(),
        usage: vec![
            // Every WoW flavor at once: they share one `Data/` store, so a
            // per-flavor size would be a fiction.
            item("World of Warcraft", layout.wow_install(), false),
            // Games Battle.net installs itself (Hearthstone and the like)
            // land inside the prefix, and are most of what it weighs.
            item("Wine prefix, Battle.net and its games", layout.compat_data(), false),
            item("Proton builds", dirs.proton_dir(), false),
            item("Shader caches", dirs.cache_dir().join(SHADER_CACHE_DIR), false),
            Usage {
                label: "Lightforge cache",
                path: dirs.cache_dir().to_path_buf(),
                bytes: clearable_entries(dirs).iter().map(|p| entry_size(p)).sum(),
                clearable: true,
            },
        ],
    }
}

/// The directory under the cache root that holds DXVK state caches — see
/// [`AppDirs::shader_cache_dir`].
const SHADER_CACHE_DIR: &str = "dxvk";

/// Free and total space on the filesystem holding `path`, or on its nearest
/// existing ancestor — a library root that does not exist yet still has a
/// disk it would land on.
// The statvfs fields are u64 on 64-bit targets and narrower elsewhere; the
// conversions below are no-ops only on the former.
#[allow(clippy::useless_conversion)]
pub fn disk(path: &Path) -> Result<Disk, StorageError> {
    let existing = path.ancestors().find(|p| p.exists()).unwrap_or(Path::new("/"));
    let err = |source| StorageError::Disk { path: existing.to_path_buf(), source };

    let c_path = CString::new(existing.as_os_str().as_bytes())
        .map_err(|e| err(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` is a
    // correctly sized out-parameter that statvfs fully initialises on success.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(err(std::io::Error::last_os_error()));
    }
    // SAFETY: statvfs returned 0, so the struct is initialised.
    let stat = unsafe { stat.assume_init() };
    let block = u64::from(stat.f_frsize);
    Ok(Disk {
        total: u64::from(stat.f_blocks) * block,
        free: u64::from(stat.f_bavail) * block,
    })
}

/// Whether nothing has been installed under this library root yet — the only
/// state in which moving it is a config change rather than a data migration.
pub fn library_is_empty(layout: &Layout) -> bool {
    [layout.games_root(), layout.compat_data()].iter().all(|dir| is_empty_dir(dir))
}

/// Whether no WoW version is installed yet — the only state in which the
/// game folder can move by re-pointing a link rather than migrating data.
pub fn game_folder_is_empty(layout: &Layout) -> bool {
    is_empty_dir(&layout.wow_install())
}

fn is_empty_dir(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Delete everything in Lightforge's cache that is only a cache, returning the
/// bytes freed.
///
/// Shader caches are kept: they are what spares the game a stutter while it
/// recompiles every shader, and dropping them is never the fix for a full
/// disk. Everything else here — theming art, the Battle.net installer, the
/// addon catalogue and the Proton release list — is re-fetched on demand.
pub fn clear_cache(dirs: &AppDirs) -> Result<u64, StorageError> {
    let mut freed = 0;
    for path in clearable_entries(dirs) {
        let bytes = entry_size(&path);
        let removed = match path.is_dir() && !path.is_symlink() {
            true => std::fs::remove_dir_all(&path),
            false => std::fs::remove_file(&path),
        };
        match removed {
            Ok(()) => freed += bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(StorageError::Remove { path, source }),
        }
    }
    Ok(freed)
}

fn clearable_entries(dirs: &AppDirs) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dirs.cache_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name() != SHADER_CACHE_DIR)
        .map(|entry| entry.path())
        .collect()
}

fn entry_size(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => dir_size(path),
        Ok(meta) if meta.is_file() => meta.len(),
        _ => 0,
    }
}

/// Recursive size, best-effort: an unreadable entry counts as nothing rather
/// than failing the whole report.
///
/// Symlinks are not followed. The prefix holds one pointing at the game
/// install, and following it would count the game twice.
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_size(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("lightforge-storage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dirs_in(root: &Path) -> AppDirs {
        AppDirs::with_dirs(root.join("config"), root.join("cache"), root.join("data"))
    }

    #[test]
    fn clearing_the_cache_keeps_shader_caches() {
        let root = scratch("clear");
        let dirs = dirs_in(&root);
        let assets = dirs.cache_dir().join("assets");
        let shaders = dirs.shader_cache_dir(&crate::product::ProductCode::Retail);
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::create_dir_all(&shaders).unwrap();
        std::fs::write(assets.join("bg.jpg"), [0u8; 100]).unwrap();
        std::fs::write(dirs.cache_dir().join("Battle.net-Setup.exe"), [0u8; 50]).unwrap();
        std::fs::write(shaders.join("Wow.dxvk-cache"), [0u8; 70]).unwrap();

        assert_eq!(clear_cache(&dirs).unwrap(), 150);
        assert!(!assets.exists());
        assert!(shaders.join("Wow.dxvk-cache").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_library_with_nothing_installed_is_empty() {
        let root = scratch("empty");
        let layout = Layout::with_root(root.join("Blizzard"));
        // Not created yet.
        assert!(library_is_empty(&layout));
        // Created, but nothing in it.
        std::fs::create_dir_all(layout.games_root()).unwrap();
        assert!(library_is_empty(&layout));
        // A prefix is an install.
        std::fs::create_dir_all(layout.prefix()).unwrap();
        assert!(!library_is_empty(&layout));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_game_folder_is_movable_until_a_game_is_in_it() {
        let root = scratch("games");
        let layout = Layout::with_root(root.join("Blizzard"))
            .with_games(Some(root.join("ssd")));
        assert!(game_folder_is_empty(&layout));
        // The link target `ensure_link` pre-creates is still empty.
        std::fs::create_dir_all(layout.wow_install()).unwrap();
        assert!(game_folder_is_empty(&layout));
        std::fs::create_dir_all(layout.product_dir(&crate::product::ProductCode::Retail)).unwrap();
        assert!(!game_folder_is_empty(&layout));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sizes_do_not_follow_symlinks() {
        let root = scratch("links");
        std::fs::write(root.join("game.bin"), [0u8; 40]).unwrap();
        let prefix = root.join("prefix");
        std::fs::create_dir_all(&prefix).unwrap();
        std::os::unix::fs::symlink(&root, prefix.join("World of Warcraft")).unwrap();
        assert_eq!(dir_size(&prefix), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_root_that_does_not_exist_reports_the_disk_it_would_land_on() {
        let disk = disk(Path::new("/definitely/not/here/Blizzard")).unwrap();
        assert!(disk.total > 0);
    }
}
