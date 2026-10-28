//! Where Lightforge's bytes live, how many there are, and what is safe to drop.
//!
//! Read-only apart from [`clear_cache`], which only ever touches Lightforge's
//! own cache directory. Game files and the prefix are measured, never deleted
//! from here.

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
    /// Available to an unprivileged user, what a download can actually use,
    /// not counting blocks reserved for root.
    pub free: u64,
}

/// One thing taking up space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// An owned string rather than a `&'static str`: most rows are fixed, but
    /// the games Battle.net installed are named after their own folders.
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
    /// The mount point `path` is on. Two libraries with the same one share a
    /// disk, and its free space is one number, not two.
    pub mount: PathBuf,
    pub state: crate::library::State,
    /// The standard install target. The prefix is not here by implication,
    /// it is always at the Blizzard root.
    pub is_default: bool,
    /// The drive letters this library occupies inside the prefix, for display.
    pub letters: (String, String),
    /// `None` when the filesystem could not be queried, a missing drive, or
    /// one `statvfs` refused.
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
    /// Lightforge's own directories. These follow XDG rather than the
    /// libraries, so they are on one drive however many libraries there are,
    /// which is why they are listed apart rather than attributed to one.
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

/// Measure every location Lightforge writes to. Walks the game installs, so it
/// belongs on a blocking thread.
pub fn report(layout: &Layout, dirs: &AppDirs) -> Report {
    let item = |label: &str, path: PathBuf, clearable| Usage {
        label: label.to_owned(),
        bytes: dir_size(&path),
        path,
        clearable,
    };

    let prefix_on = layout.compat_data();
    // Everything Battle.net installed for itself, named and measured, so it
    // can be attributed to the drive it is actually on instead of disappearing
    // into one "Wine prefix" figure.
    let installed = battle_net_games(layout);
    // The library the prefix belongs to: the one whose root contains it. The
    // prefix never moves between drives, so this is a fact about the layout
    // rather than a choice.
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
            // A missing drive is not measured: walking its mount point would
            // report whatever the filesystem *under* it holds as if it were
            // the library's, and 0 bytes free as if the disk were full.
            let holds_prefix = prefix_owner == Some(index);
            let usage = match state.is_present() {
                false => Vec::new(),
                true => {
                    // Every WoW flavor in this library at once: they share one
                    // `Data/` store, so a per-flavor size would be a fiction.
                    let wow = dir_size(&install);
                    let mut usage =
                        vec![Usage { label: WOW.to_owned(), path: install.clone(), bytes: wow, clearable: false }];

                    // Battle.net's own titles on this drive, each by name.
                    // One is on this drive if its files are under the library's
                    // `games/`, or, for the older arrangement, which is what
                    // every existing install still has, inside the prefix.
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

                    // Whatever else is in `games/`: a folder no product.db
                    // record names, which is what a half-finished install or a
                    // hand-copied game looks like.
                    let in_games: u64 = mine
                        .iter()
                        .filter(|g| g.path.starts_with(library.games()))
                        .map(|g| g.bytes)
                        .sum();
                    // The library's own marker lives in `games/` and is a few
                    // hundred bytes, which is enough to keep this row from
                    // reaching zero and make an empty drive claim to hold
                    // something.
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
                        // The prefix minus the games inside it. Without the
                        // subtraction this row swallows Hearthstone and every
                        // other Battle.net title and reports them as Wine.
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
/// Today that is Battle.net's own leftovers, the bootstrapper's staging copy
/// and Agent's directory cache, both of which pre-flight already removes when
/// it gets the chance. Game files are never touched from here.
pub fn clear_library(layout: &Layout, library: &crate::library::Library) -> Result<u64, StorageError> {
    if !library.state().is_present() {
        return Ok(0);
    }
    // Pre-flight deletes these only once the prefix is quiet: Agent and the
    // bootstrapper use them while they run, an install in progress included.
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
    /// What to call it. The install folder's own name, which is what Blizzard
    /// named it and what the user would call it, the alternative is the
    /// product code (`hs_beta`, `w1r`), which names nothing to anyone.
    name: String,
    path: PathBuf,
    bytes: u64,
}

/// Every title Battle.net records as installed, resolved to a real directory.
///
/// Read out of Agent's own `product.db`, so this says what the client thinks
/// rather than what a directory walk guesses. Battle.net's plumbing is left
/// out, and so is WoW: Lightforge measures that itself, per library, and the
/// `C:` path recorded for it is the symlink to the one already counted.
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

/// Free and total space on the filesystem holding `path`, or on its nearest
/// existing ancestor, a library root that does not exist yet still has a
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

/// Whether nothing has been installed under this library root yet, the only
/// state in which moving it is a config change rather than a data migration.
pub fn library_is_empty(layout: &Layout) -> bool {
    [layout.games_root(), layout.compat_data()].iter().all(|dir| is_empty_dir(dir))
}

/// Whether a library already holds a WoW install.
///
/// What the install picker asks of every library: one that holds an install
/// shares its `.build.info` and its CASC store with whatever is added to it,
/// and one that does not is starting a second, independent install.
pub fn holds_wow(layout: &Layout, library: &crate::library::Library) -> bool {
    // `is_present` rather than `is_ready`: a library that has never been
    // claimed can still have a game in it, and on an upgrade from before
    // multi-drive storage that is exactly the case, the first library is
    // seeded unclaimed and has 150 GB in it.
    library.state().is_present() && !is_empty_dir(&layout.wow_install_in(library))
}

/// Whether a directory has nothing of ours in it.
///
/// A library's own marker does not count: it is there from the moment the
/// library is set up, long before a game is, so counting it would mean no
/// library ever reads as empty.
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
/// Shader caches are kept: they are what spares the game a stutter while it
/// recompiles every shader, and dropping them is never the fix for a full
/// disk. Everything else here, theming art, the Battle.net installer, the
/// addon catalogue and the Proton release list, is re-fetched on demand.
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

    /// The games Battle.net installed for itself live inside the prefix, and
    /// used to be reported as part of it, a 15.4 GiB "Wine prefix" row that
    /// was mostly Hearthstone. Each is now named, measured where it is, and
    /// subtracted from the prefix.
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
        // Everything under the prefix *except* the game, which is the whole
        // point: before this, the game's bytes were reported as Wine's.
        assert_eq!(
            row("Wine prefix and Battle.net"),
            Some(dir_size(&layout.compat_data()) - 4000),
        );
        // An empty games folder is empty, marker and all.
        assert_eq!(row("Other games"), Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A drive that is away is reported as away, not as full and not as
    /// empty. Measuring it would walk the mount point and attribute whatever
    /// filesystem is *under* it to the library.
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

