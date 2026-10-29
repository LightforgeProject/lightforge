//! Where Lightforge's bytes live, how many there are, and what is safe to drop.
//!
//! Read-only apart from [`clear_cache`] and [`clear_library`], which never
//! touch game files.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::fsutil::dir_size;
use crate::paths::{AppDirs, Layout};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("cannot read free space for {path}: {source}")]
    Disk { path: PathBuf, source: std::io::Error },
    #[error("cannot remove {path}: {source}")]
    Remove { path: PathBuf, source: std::io::Error },
    #[error("Battle.net or a game is running in the prefix; close it before clearing its files")]
    PrefixBusy,
}

/// Capacity of the filesystem a path lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Disk {
    pub total: u64,
    /// Available to an unprivileged user, root's reserve excluded.
    pub free: u64,
}

/// One thing taking up space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// Owned: Battle.net's games are named after their folders.
    pub label: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// Whether [`clear_cache`] removes it.
    pub clearable: bool,
}

/// One storage library, measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryUsage {
    pub id: String,
    pub label: String,
    /// The library's `games/` directory.
    pub path: PathBuf,
    /// The mount point `path` is on; libraries sharing one share free space.
    pub mount: PathBuf,
    pub state: crate::library::State,
    /// The standard install target.
    pub is_default: bool,
    /// The drive letters this library occupies inside the prefix, for display.
    pub letters: (String, String),
    /// `None` when the filesystem could not be queried.
    pub disk: Option<Disk>,
    pub usage: Vec<Usage>,
}

impl LibraryUsage {
    /// Everything Lightforge has put on this drive.
    pub fn total(&self) -> u64 {
        self.usage.iter().map(|u| u.bytes).sum()
    }

    /// What can be reclaimed here.
    pub fn clearable(&self) -> u64 {
        self.usage.iter().filter(|u| u.clearable).map(|u| u.bytes).sum()
    }
}

/// Everything the Storage section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// One per configured library, main first.
    pub libraries: Vec<LibraryUsage>,
    /// Lightforge's own directories, under XDG, so listed apart from the libraries.
    pub app: Vec<Usage>,
    /// The drive the entries in `app` are on.
    pub app_root: PathBuf,
    pub app_disk: Option<Disk>,
}

impl Report {
    pub fn library(&self, id: &str) -> Option<&LibraryUsage> {
        self.libraries.iter().find(|l| l.id == id)
    }
}

/// Measure every location Lightforge writes to, walking the game installs:
/// a blocking-thread job.
pub fn report(layout: &Layout, dirs: &AppDirs) -> Report {
    let item = |label: &str, path: PathBuf, clearable| Usage {
        label: label.to_owned(),
        bytes: dir_size(&path),
        path,
        clearable,
    };

    let prefix_on = layout.compat_data();
    // Battle.net's own titles, attributed to the drive they are on.
    let installed = battle_net_games(layout);
    // The library whose root contains the prefix.
    let prefix_owner = layout
        .libraries()
        .all()
        .iter()
        .position(|library| prefix_on.starts_with(library.root()))
        .or_else(|| layout.libraries().all().iter().position(|l| l.is_default));
    let mut libraries: Vec<LibraryUsage> = layout
        .libraries()
        .all()
        .iter()
        .enumerate()
        .map(|(index, library)| {
            let install = layout.wow_install_in(library);
            let state = library.state();
            // A missing drive is not measured: its mount point shows the
            // filesystem underneath.
            let holds_prefix = prefix_owner == Some(index);
            let usage = match state.is_present() {
                false => Vec::new(),
                true => {
                    // All WoW flavors at once: they share one `Data/` store.
                    let wow = dir_size(&install);
                    let mut usage =
                        vec![Usage { label: WOW.to_owned(), path: install.clone(), bytes: wow, clearable: false }];

                    // Battle.net's titles on this drive: under `games/`, or
                    // inside the prefix as older installs have them.
                    let mine: Vec<&Game> = installed
                        .iter()
                        .filter(|game| {
                            game.path.starts_with(library.games())
                                || (holds_prefix && game.path.starts_with(&prefix_on))
                        })
                        .collect();
                    for game in &mine {
                        usage.push(Usage {
                            label: game.name.clone(),
                            path: game.path.clone(),
                            bytes: game.bytes,
                            clearable: false,
                        });
                    }

                    // Folders in `games/` no `product.db` record names.
                    let in_games: u64 = mine
                        .iter()
                        .filter(|g| g.path.starts_with(library.games()))
                        .map(|g| g.bytes)
                        .sum();
                    // Without the marker, or an empty drive shows a few bytes.
                    let marker = std::fs::metadata(library.marker())
                        .map(|m| m.len())
                        .unwrap_or(0);
                    usage.push(Usage {
                        label: "Other games".to_owned(),
                        path: library.games().to_path_buf(),
                        bytes: dir_size(library.games())
                            .saturating_sub(wow)
                            .saturating_sub(in_games)
                            .saturating_sub(marker),
                        clearable: false,
                    });

                    if holds_prefix {
                        // The prefix minus Battle.net's games inside it.
                        usage.push(Usage {
                            label: "Wine prefix and Battle.net".to_owned(),
                            bytes: dir_size(&prefix_on).saturating_sub(
                                mine.iter()
                                    .filter(|g| g.path.starts_with(&prefix_on))
                                    .map(|g| g.bytes)
                                    .sum(),
                            ),
                            path: prefix_on.clone(),
                            clearable: false,
                        });
                        usage.push(Usage {
                            label: "Battle.net leftovers".to_owned(),
                            bytes: [layout.battle_net_staging(), layout.agent_cache_dir()]
                                .iter()
                                .map(|p| dir_size(p))
                                .sum(),
                            path: layout.battle_net_dir(),
                            clearable: true,
                        });
                    }
                    usage
                }
            };
            LibraryUsage {
                id: library.id.clone(),
                label: library.label.clone(),
                path: library.games().to_path_buf(),
                mount: crate::library::mount_point_of(library.games()),
                state,
                is_default: library.is_default,
                letters: (library.games_letter.clone(), library.wow_letter.clone()),
                disk: state.is_present().then(|| disk(library.games()).ok()).flatten(),
                usage,
            }
        })
        .collect();
    libraries.sort_by_key(|l| !l.is_default);
    // Libraries on one disk sit together, under whichever came first.
    let mut grouped: Vec<LibraryUsage> = Vec::with_capacity(libraries.len());
    while !libraries.is_empty() {
        let first = libraries.remove(0);
        let mount = first.mount.clone();
        grouped.push(first);
        let (same, rest): (Vec<_>, Vec<_>) = libraries.into_iter().partition(|l| l.mount == mount);
        grouped.extend(same);
        libraries = rest;
    }
    let libraries = grouped;

    Report {
        libraries,
        app: vec![
            item("Proton builds", dirs.proton_dir(), false),
            item("Shader caches", dirs.cache_dir().join(SHADER_CACHE_DIR), false),
            Usage {
                label: "Lightforge cache".to_owned(),
                path: dirs.cache_dir().to_path_buf(),
                bytes: clearable_entries(dirs).iter().map(|p| entry_size(p)).sum(),
                clearable: true,
            },
        ],
        app_root: dirs.data_dir().to_path_buf(),
        app_disk: disk(dirs.data_dir()).ok(),
    }
}

/// Delete what [`report`] marked clearable on one library.
///
/// Battle.net's leftovers: the bootstrapper's staging copy and Agent's cache.
pub fn clear_library(layout: &Layout, library: &crate::library::Library) -> Result<u64, StorageError> {
    if !library.state().is_present() {
        return Ok(0);
    }
    // Only with a quiet prefix, as pre-flight does.
    if !crate::preflight::scan(layout).is_empty() {
        return Err(StorageError::PrefixBusy);
    }
    let mut freed = 0;
    for path in [layout.battle_net_staging(), layout.agent_cache_dir()] {
        let bytes = dir_size(&path);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => freed += bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(StorageError::Remove { path, source }),
        }
    }
    Ok(freed)
}

/// Label for the row covering every WoW flavor in one library.
const WOW: &str = "World of Warcraft";

/// A game Battle.net installed, measured where it actually is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Game {
    /// The install folder's name, not the product code (`hs_beta`, `w1r`).
    name: String,
    path: PathBuf,
    bytes: u64,
}

/// Every title Battle.net records as installed, resolved to a real directory.
///
/// From Agent's `product.db`. Not Battle.net's plumbing, and not WoW, which
/// is measured per library.
fn battle_net_games(layout: &Layout) -> Vec<Game> {
    let Ok(records) = crate::tact::productdb::read(&layout.agent_dir().join("product.db")) else {
        return Vec::new();
    };
    let wow_installs: Vec<PathBuf> =
        layout.libraries().all().iter().map(|l| layout.wow_install_in(l)).collect();

    let mut games: Vec<Game> = Vec::new();
    for record in records {
        if record.is_infrastructure() {
            continue;
        }
        let Some(path) = layout.resolve_windows(&record.install_path) else {
            continue;
        };
        // Two WoW flavors share one folder, and so could two of anything else.
        if wow_installs.contains(&path) || games.iter().any(|g| g.path == path) {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if name == WOW {
            continue;
        }
        let bytes = dir_size(&path);
        if bytes == 0 {
            continue;
        }
        games.push(Game { name, path, bytes });
    }
    games.sort_by_key(|game| std::cmp::Reverse(game.bytes));
    games
}

/// The directory under the cache root that holds the shader caches, see
/// [`AppDirs::shader_cache_dir`].
const SHADER_CACHE_DIR: &str = "dxvk";

/// Free and total space on the filesystem holding `path`, or its nearest
/// existing ancestor.
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

/// Whether nothing is installed under this library root, so moving it is
/// only a config change.
pub fn library_is_empty(layout: &Layout) -> bool {
    [layout.games_root(), layout.compat_data()].iter().all(|dir| is_empty_dir(dir))
}

/// Whether a library already holds a WoW install.
///
/// A product added to it shares its `.build.info` and CASC store.
pub fn holds_wow(layout: &Layout, library: &crate::library::Library) -> bool {
    // `is_present`: an upgrade seeds the first library unclaimed, game and all.
    library.state().is_present() && !is_empty_dir(&layout.wow_install_in(library))
}

/// Whether a directory has nothing of ours in it.
///
/// The library's own marker does not count.
fn is_empty_dir(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .all(|entry| entry.file_name() == crate::library::MARKER),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Delete everything in Lightforge's cache that is only a cache, returning the
/// bytes freed.
///
/// Shader caches are kept (dropping them means stutter). Everything else is
/// re-fetched on demand.
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

    /// A `product.db` record with only the two fields this file reads.
    fn record(product: &str, install_path: &str) -> crate::tact::productdb::ProductInstall {
        crate::tact::productdb::ProductInstall {
            product: product.to_owned(),
            install_path: install_path.to_owned(),
            region: String::new(),
            locale: String::new(),
            account_country: String::new(),
            geo_country: String::new(),
            flavor_dir: String::new(),
            version: String::new(),
            build_key: String::new(),
            tags: String::new(),
            key_ring: None,
            installed_at_ms: 0,
            family: String::new(),
        }
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
        std::fs::write(shaders.join("Wow.dxvk.bin"), [0u8; 70]).unwrap();

        assert_eq!(clear_cache(&dirs).unwrap(), 150);
        assert!(!assets.exists());
        assert!(shaders.join("Wow.dxvk.bin").exists());
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
    fn a_library_holds_wow_only_once_a_flavor_is_in_it() {
        let root = scratch("games");
        let layout = Layout::with_root(root.join("Blizzard"))
            .with_games(Some(root.join("ssd")));
        let mut library = layout.default_library().unwrap().clone();
        assert!(!holds_wow(&layout, &library), "nothing is in it yet");
        library.claim().unwrap();
        assert!(!holds_wow(&layout, &library));

        // The link target `ensure_link` pre-creates is still empty.
        std::fs::create_dir_all(layout.wow_install_in(&library)).unwrap();
        assert!(!holds_wow(&layout, &library));
        std::fs::create_dir_all(layout.product_dir(&crate::product::ProductCode::Retail)).unwrap();
        assert!(holds_wow(&layout, &library));

        // A drive that went away holds nothing, whatever is under its old
        // path, the library it would have to read from is not there. The
        // claim is what tells that apart from a library nobody has set up.
        std::fs::remove_file(library.marker()).unwrap();
        assert!(!holds_wow(&layout, &library));
        // Unclaimed and full is an upgrade from before libraries existed, and
        // that one does hold a game.
        library.claimed = false;
        assert!(holds_wow(&layout, &library));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Battle.net's own games are named and subtracted from the prefix: a
    /// 15.4 GiB "Wine prefix" row was mostly Hearthstone.
    #[test]
    fn battle_net_games_are_named_and_not_counted_as_wine() {
        use crate::tact::productdb;

        let root = scratch("bnet-games");
        let dirs = dirs_in(&root);
        let layout = Layout::with_root(root.join("Blizzard"));
        let mut library = layout.default_library().unwrap().clone();
        library.claim().unwrap();
        let layout = layout.with_libraries(crate::library::Libraries::new(vec![library]));

        // A prefix with Wine in it, a game Battle.net installed, and the `c:`
        // entry Wine writes, a relative symlink, which is what makes the
        // resolver worth having.
        let program_files = layout.program_files_x86();
        std::fs::create_dir_all(program_files.join("Hearthstone")).unwrap();
        std::fs::write(program_files.join("Hearthstone").join("Hearthstone.exe"), [0u8; 4000])
            .unwrap();
        std::fs::create_dir_all(layout.prefix().join("windows")).unwrap();
        std::fs::write(layout.prefix().join("windows").join("wine.dll"), [0u8; 1000]).unwrap();
        std::fs::create_dir_all(layout.dosdevices()).unwrap();
        std::os::unix::fs::symlink("../drive_c", layout.dosdevices().join("c:")).unwrap();

        std::fs::create_dir_all(layout.agent_dir()).unwrap();
        std::fs::write(
            layout.agent_dir().join("product.db"),
            productdb::encode(&[
                record("hs_beta", r"C:\Program Files (x86)\Hearthstone"),
                // Battle.net's own plumbing is not a game anyone plays.
                record("battle.net", r"C:\Program Files (x86)\Battle.net"),
            ]),
        )
        .unwrap();

        let report = report(&layout, &dirs);
        let library = &report.libraries[0];
        let row = |label: &str| {
            library.usage.iter().find(|u| u.label == label).map(|u| u.bytes)
        };

        assert_eq!(row("Hearthstone"), Some(4000), "the game is named and measured");
        assert_eq!(row("Battle.net"), None, "the client is not a game");
        // Everything under the prefix except the game.
        assert_eq!(
            row("Wine prefix and Battle.net"),
            Some(dir_size(&layout.compat_data()) - 4000),
        );
        // An empty games folder is empty, marker and all.
        assert_eq!(row("Other games"), Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A drive that is away is reported as away, not full or empty.
    #[test]
    fn a_missing_library_is_reported_rather_than_measured() {
        use crate::library::{Libraries, Library};

        let root = scratch("missing");
        let dirs = dirs_in(&root);
        let mut away = Library::on_partition(&root.join("ssd"), "nvme2", None);
        away.games_letter = "f:".to_owned();
        away.wow_letter = "g:".to_owned();
        away.claimed = true;
        // Something is under the mount point, as there would be on the
        // filesystem a mount point lives on.
        std::fs::create_dir_all(away.games()).unwrap();
        std::fs::write(away.games().join("stray.bin"), [0u8; 4096]).unwrap();

        let layout = Layout::with_root(root.join("Blizzard"))
            .with_libraries(Libraries::new(vec![away.clone()]));
        let report = report(&layout, &dirs);

        let measured = report.library(&away.id).unwrap();
        assert_eq!(measured.state, crate::library::State::Missing);
        assert!(measured.disk.is_none(), "an absent drive has no free space to report");
        assert_eq!(measured.total(), 0, "nothing under the mount point was counted");
        assert_eq!(measured.letters, ("f:".to_owned(), "g:".to_owned()));
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

