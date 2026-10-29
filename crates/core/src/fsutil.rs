//! Filesystem operations several modules share: atomic replace, copying a
//! tree, directory size.

use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Replace `path` with `bytes` via a temp file in the same directory and a
/// rename, so a crash leaves the old file or the new one, never a torn one.
///
/// An existing file keeps its mode. A new one gets 0644, not tempfile's 0600.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mode = match std::fs::metadata(path) {
        Ok(meta) => meta.permissions().mode() & 0o7777,
        Err(_) => 0o644,
    };
    write_atomic_with_mode(path, bytes, mode)
}

/// [`write_atomic`] at a fixed mode, for game files Wine and the client read.
pub(crate) fn write_atomic_with_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;

    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().set_permissions(PermissionsExt::from_mode(mode))?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;

    // Durability of the rename, not of the contents.
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// Copy every file under `from` into the same place under `to`, modes
/// included. The error names the path it failed on.
///
/// Symlinks stay links with the same target text: a `WTF` or `AddOns` linked
/// to a shared folder must not become a second copy of it.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(), (PathBuf, io::Error)> {
    fn at(path: &Path) -> impl FnOnce(io::Error) -> (PathBuf, io::Error) + '_ {
        move |e| (path.to_path_buf(), e)
    }
    std::fs::create_dir_all(to).map_err(at(to))?;
    for entry in std::fs::read_dir(from).map_err(at(from))?.filter_map(Result::ok) {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        match entry.file_type().map_err(at(&source))? {
            kind if kind.is_dir() => copy_tree(&source, &target)?,
            kind if kind.is_file() => {
                std::fs::copy(&source, &target).map_err(at(&target))?;
            }
            kind if kind.is_symlink() => {
                let link = std::fs::read_link(&source).map_err(at(&source))?;
                std::os::unix::fs::symlink(&link, &target).map_err(at(&target))?;
                // A relative link into a sibling flavour of the old install.
                if !target.exists() {
                    tracing::warn!(
                        "{} points at {}, which is not there from {}",
                        source.display(),
                        link.display(),
                        target.display()
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Recursive size, best-effort: an unreadable entry counts as nothing rather
/// than failing the whole report.
///
/// Symlinks are not followed. The prefix holds one pointing at the game
/// install, and following it would count the game twice.
pub(crate) fn dir_size(dir: &Path) -> u64 {
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

    #[test]
    fn an_extracted_file_lands_readable_with_its_directories_made() {
        let dir = std::env::temp_dir().join(format!("lightforge-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Utils").join("swiftshader").join("libEGL.dll");

        write_atomic_with_mode(&path, b"MZ...", 0o644).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"MZ...");
        // tempfile would leave these 0600; Wine and the client have to read them.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "{mode:o}");

        // Overwriting an existing file works, and leaves no temporary behind.
        write_atomic_with_mode(&path, b"MZ newer", 0o644).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"MZ newer");
        let siblings: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().collect();
        assert_eq!(siblings.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copied_tree_keeps_its_symlinks_as_links() {
        let root = tempfile::tempdir().unwrap();
        let (from, to, shared) = (root.path().join("from"), root.path().join("to"), root.path().join("shared"));
        std::fs::create_dir_all(from.join("Interface")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(from.join("Config.wtf"), b"SET locale \"enUS\"").unwrap();
        std::fs::write(from.join("Interface/AddOns.txt"), b"BigWigs: enabled").unwrap();
        std::os::unix::fs::symlink(&shared, from.join("WTF")).unwrap();
        std::os::unix::fs::symlink("Config.wtf", from.join("Config.link")).unwrap();

        copy_tree(&from, &to).unwrap();

        assert_eq!(std::fs::read(to.join("Config.wtf")).unwrap(), b"SET locale \"enUS\"");
        assert_eq!(std::fs::read(to.join("Interface/AddOns.txt")).unwrap(), b"BigWigs: enabled");
        for (name, target) in [("WTF", shared.as_path()), ("Config.link", Path::new("Config.wtf"))] {
            let link = to.join(name);
            assert!(link.symlink_metadata().unwrap().file_type().is_symlink(), "{name} is still a link");
            assert_eq!(std::fs::read_link(&link).unwrap(), target);
        }
    }
}
