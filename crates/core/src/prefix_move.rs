//! Moving the Wine prefix, and Battle.net inside it, to another folder or drive.
//!
//! Only the compat-data folder moves, and nothing in it names its own path: on
//! a GE-Proton11-7 prefix (2026-11-07) every absolute link leads out to Proton
//! or a game, and `dosdevices/c:` is relative. Battle.net records drive letters.
//!
//! So that a crash leaves a prefix in at least one place:
//!
//! 1. [`place`] puts it at the target: a rename on one filesystem, else a copy
//!    that is compared with the original before it takes the target's name.
//! 2. The caller records the target in the config, or calls [`Moved::undo`].
//! 3. [`Moved::release`] deletes the original, if a copy left one.

use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::fsutil::{copy_tree, dir_size};
use crate::paths::Layout;

#[derive(Debug, thiserror::Error)]
pub enum PrefixMoveError {
    #[error("the Wine prefix is already at {0}")]
    AlreadyThere(PathBuf),

    #[error("the Wine prefix at {0} is not there; mount its drive first, or remove prefix_dir from the config to start over")]
    Away(PathBuf),

    #[error("close these first, they are running in the prefix: {}", .0.join(", "))]
    Busy(Vec<String>),

    #[error("{0} already has files in it")]
    Occupied(PathBuf),

    #[error("{target} and the prefix at {prefix} would be inside one another")]
    Overlap { target: PathBuf, prefix: PathBuf },

    #[error("the game library at {0} is inside the prefix; move it to another drive first")]
    LibraryInside(PathBuf),

    #[error("{path} cannot hold a Wine prefix: {reason}")]
    Unfit { path: PathBuf, reason: &'static str },

    #[error("{path} has {free} bytes free and the prefix needs {needed}")]
    NoSpace { path: PathBuf, needed: u64, free: u64 },

    #[error("the copy at {0} does not match the prefix, so it was discarded")]
    Mismatch(PathBuf),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn at(path: &Path) -> impl FnOnce(io::Error) -> PrefixMoveError + '_ {
    move |source| PrefixMoveError::Io { path: path.to_path_buf(), source }
}

/// How the prefix got to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// There was none yet; the target is an empty folder for setup to fill.
    NothingToMove,
    Renamed,
    Copied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub from: PathBuf,
    pub to: PathBuf,
    pub how: How,
}

impl Moved {
    /// Delete the original, once the config names the target.
    pub fn release(&self) -> io::Result<()> {
        match self.how {
            How::Copied => std::fs::remove_dir_all(&self.from),
            How::NothingToMove | How::Renamed => Ok(()),
        }
    }

    /// Go back to how it was, when the config could not be saved.
    pub fn undo(&self) -> io::Result<()> {
        match self.how {
            How::NothingToMove | How::Copied => std::fs::remove_dir_all(&self.to),
            How::Renamed => std::fs::rename(&self.to, &self.from),
        }
    }
}

/// Where the prefix lands for a chosen folder: a folder of its own inside it,
/// since the choice is often a shared `prefixes/`. The Blizzard root means
/// back to where setup puts it.
pub fn target_in(layout: &Layout, chosen: &Path) -> PathBuf {
    match chosen == layout.root() {
        true => layout.default_compat_data(),
        false => chosen.join("lightforge"),
    }
}

/// Put the prefix in `chosen`. Blocks for as long as a copy takes.
pub fn place(layout: &Layout, chosen: &Path) -> Result<Moved, PrefixMoveError> {
    let from = layout.compat_data();
    let to = target_in(layout, chosen);
    // Not "nothing to move": this would orphan a prefix on an unmounted drive.
    if layout.prefix_away() {
        return Err(PrefixMoveError::Away(from));
    }
    if to == from {
        return Err(PrefixMoveError::AlreadyThere(to));
    }
    if to.starts_with(&from) || from.starts_with(&to) {
        return Err(PrefixMoveError::Overlap { target: to, prefix: from });
    }
    if std::fs::read_dir(&to).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(PrefixMoveError::Occupied(to));
    }
    if let Some(library) = layout.libraries().all().iter().find(|l| l.games().starts_with(&from)) {
        return Err(PrefixMoveError::LibraryInside(library.games().to_path_buf()));
    }
    let running = crate::preflight::scan(layout);
    if !running.is_empty() {
        return Err(PrefixMoveError::Busy(running.into_iter().map(|p| p.name).collect()));
    }

    let parent = to.parent().unwrap_or(chosen);
    std::fs::create_dir_all(parent).map_err(at(parent))?;
    check_fit(parent)?;

    if !from.is_dir() {
        std::fs::create_dir_all(&to).map_err(at(&to))?;
        return Ok(Moved { from, to, how: How::NothingToMove });
    }
    // Over an empty target, if there is one.
    match std::fs::rename(&from, &to) {
        Ok(()) => Ok(Moved { from, to, how: How::Renamed }),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_across(&from, &to, parent)?;
            Ok(Moved { from, to, how: How::Copied })
        }
        Err(e) => Err(at(&from)(e)),
    }
}

/// Refuse a filesystem Wine cannot use, before gigabytes go to it.
fn check_fit(dir: &Path) -> Result<(), PrefixMoveError> {
    let unfit = |reason| PrefixMoveError::Unfit { path: dir.to_path_buf(), reason };
    let probe = crate::staging::create_in(dir).map_err(at(dir))?;
    // `dosdevices/c:` is both: FAT and exFAT have neither.
    symlink("../drive_c", probe.path().join("c:"))
        .map_err(|_| unfit("its filesystem has no symlinks, or no ':' in file names"))?;
    // Wine maps a prefix's Windows DLLs as executable.
    if crate::storage::is_noexec(dir).map_err(at(dir))? {
        return Err(unfit("it is mounted noexec"));
    }
    Ok(())
}

fn copy_across(from: &Path, to: &Path, parent: &Path) -> Result<(), PrefixMoveError> {
    let (needed, free) = (dir_size(from), crate::storage::disk(parent).map(|d| d.free).unwrap_or(u64::MAX));
    if needed > free {
        return Err(PrefixMoveError::NoSpace { path: parent.to_path_buf(), needed, free });
    }

    // Removed on every way out, and takes the target's name only when whole.
    let staging = crate::staging::create_in(parent).map_err(at(parent))?;
    let copy = staging.path().join("prefix");
    copy_tree(from, &copy).map_err(|(path, source)| PrefixMoveError::Io { path, source })?;

    if listing(from).map_err(at(from))? != listing(&copy).map_err(at(&copy))? {
        return Err(PrefixMoveError::Mismatch(to.to_path_buf()));
    }
    std::fs::rename(&copy, to).map_err(at(to))
}

#[derive(Debug, PartialEq, Eq)]
enum Item {
    Dir,
    File(u64),
    Link(PathBuf),
}

/// Everything [`copy_tree`] carries across, by path below `root`.
fn listing(root: &Path) -> io::Result<BTreeMap<PathBuf, Item>> {
    fn walk(dir: &Path, root: &Path, items: &mut BTreeMap<PathBuf, Item>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let (path, kind) = (entry.path(), entry.file_type()?);
            let item = if kind.is_dir() {
                walk(&path, root, items)?;
                Item::Dir
            } else if kind.is_file() {
                Item::File(entry.metadata()?.len())
            } else if kind.is_symlink() {
                Item::Link(std::fs::read_link(&path)?)
            } else {
                continue;
            };
            items.insert(path.strip_prefix(root).unwrap_or(&path).to_path_buf(), item);
        }
        Ok(())
    }

    let mut items = BTreeMap::new();
    walk(root, root, &mut items)?;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parts of a real prefix a move has to keep.
    fn fake_prefix(layout: &Layout) {
        let pfx = layout.prefix();
        std::fs::create_dir_all(pfx.join("drive_c/windows")).unwrap();
        std::fs::create_dir_all(pfx.join("dosdevices")).unwrap();
        std::fs::write(pfx.join("system.reg"), "WINE REGISTRY Version 2").unwrap();
        std::fs::write(layout.compat_data().join("version"), "GE-Proton11-7").unwrap();
        symlink("../drive_c", pfx.join("dosdevices/c:")).unwrap();
        symlink("/games/World of Warcraft", pfx.join("dosdevices/d:")).unwrap();
    }

    #[test]
    fn a_chosen_folder_gets_a_folder_of_its_own_and_the_root_means_home() {
        let layout = Layout::with_root("/home/u/Games/Blizzard");
        assert_eq!(target_in(&layout, Path::new("/mnt/x/prefixes")), Path::new("/mnt/x/prefixes/lightforge"));
        assert_eq!(target_in(&layout, layout.root()), layout.default_compat_data());
    }

    #[test]
    fn a_rename_keeps_everything_and_leaves_nothing_to_release() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        fake_prefix(&layout);
        let before = listing(&layout.compat_data()).unwrap();

        let moved = place(&layout, &dir.path().join("prefixes")).unwrap();

        assert_eq!(moved.how, How::Renamed);
        assert_eq!(listing(&moved.to).unwrap(), before);
        assert!(!moved.from.exists());
        moved.release().unwrap();
        assert!(moved.to.join("pfx/system.reg").is_file());
    }

    #[test]
    fn undoing_a_rename_puts_the_prefix_back() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        fake_prefix(&layout);

        let moved = place(&layout, &dir.path().join("prefixes")).unwrap();
        moved.undo().unwrap();

        assert!(layout.prefix().join("system.reg").is_file());
        assert!(!moved.to.exists());
    }

    #[test]
    fn a_copy_reads_the_same_and_the_original_goes_only_on_release() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        fake_prefix(&layout);
        let (from, parent) = (layout.compat_data(), dir.path().join("prefixes"));
        std::fs::create_dir_all(&parent).unwrap();
        let to = parent.join("lightforge");

        copy_across(&from, &to, &parent).unwrap();

        assert_eq!(listing(&to).unwrap(), listing(&from).unwrap());
        assert_eq!(std::fs::read_link(to.join("pfx/dosdevices/c:")).unwrap(), Path::new("../drive_c"));
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1, "no staging folder stays behind");

        let moved = Moved { from: from.clone(), to, how: How::Copied };
        assert!(from.is_dir());
        moved.release().unwrap();
        assert!(!from.exists());
    }

    #[test]
    fn a_folder_with_files_in_it_is_refused_and_the_prefix_stays() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        fake_prefix(&layout);
        let taken = dir.path().join("prefixes/lightforge");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("someone-elses"), "x").unwrap();

        let result = place(&layout, &dir.path().join("prefixes"));
        assert!(matches!(result, Err(PrefixMoveError::Occupied(_))), "{result:?}");
        assert!(layout.prefix().join("system.reg").is_file());
    }

    #[test]
    fn a_target_inside_the_prefix_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        fake_prefix(&layout);

        let result = place(&layout, &layout.prefix());
        assert!(matches!(result, Err(PrefixMoveError::Overlap { .. })), "{result:?}");
    }

    #[test]
    fn moving_where_it_already_is_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        fake_prefix(&layout);

        let result = place(&layout, layout.root());
        assert!(matches!(result, Err(PrefixMoveError::AlreadyThere(_))), "{result:?}");
    }

    #[test]
    fn a_library_inside_the_prefix_blocks_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Blizzard");
        let layout = Layout::with_root(&root).with_games(Some(root.join("prefix/pfx/drive_c/games")));
        fake_prefix(&layout);

        let result = place(&layout, &dir.path().join("prefixes"));
        assert!(matches!(result, Err(PrefixMoveError::LibraryInside(_))), "{result:?}");
    }

    #[test]
    fn a_prefix_on_an_unmounted_drive_is_not_taken_for_none() {
        let dir = tempfile::tempdir().unwrap();
        let away = dir.path().join("unmounted/lightforge");
        let layout = Layout::with_root(dir.path().join("Blizzard")).with_compat_data(Some(away));

        let result = place(&layout, &dir.path().join("prefixes"));
        assert!(matches!(result, Err(PrefixMoveError::Away(_))), "{result:?}");
        assert!(!dir.path().join("prefixes").exists());
    }

    #[test]
    fn before_setup_there_is_only_a_folder_to_make() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));

        let moved = place(&layout, &dir.path().join("prefixes")).unwrap();

        assert_eq!(moved.how, How::NothingToMove);
        assert!(moved.to.is_dir());
    }
}
