//! What a downloaded tarball may unpack, checked entry by entry before it is
//! written: plain files and folders, links only where they stay inside the
//! archive's top folder, and no more bytes than the caller allows.
//!
//! Every GE-Proton build checked (11-5, 11-7, 11-12) holds about 1,900
//! relative symlinks, all inside the build, and no hardlinks or devices.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("archive contains an unsafe path: {}", .0.display())]
    UnsafePath(PathBuf),
    #[error("{} in the archive is a {kind}, which is never unpacked", .path.display())]
    EntryType { path: PathBuf, kind: String },
    #[error("{} in the archive links to {}, outside it", .path.display(), .target.display())]
    LinkOutside { path: PathBuf, target: PathBuf },
    #[error("the archive unpacks to more than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("could not read the archive's entry headers")]
    Header(#[source] std::io::Error),
}

/// The checks one unpack runs, and the bytes it has left.
pub(crate) struct Admission {
    links: bool,
    limit: u64,
    left: u64,
}

impl Admission {
    /// Plain files and folders only: our own release tarball.
    pub(crate) fn files_only(limit: u64) -> Self {
        Self { links: false, limit, left: limit }
    }

    /// Links too, each kept inside the top folder: a Proton build.
    pub(crate) fn with_links(limit: u64) -> Self {
        Self { links: true, limit, left: limit }
    }

    pub(crate) fn check<R: Read>(&mut self, entry: &tar::Entry<'_, R>) -> Result<(), ArchiveError> {
        let path = entry.path().map_err(ArchiveError::Header)?.into_owned();
        if !stays_relative(&path) {
            return Err(ArchiveError::UnsafePath(path));
        }
        let kind = entry.header().entry_type();
        match kind {
            tar::EntryType::Regular | tar::EntryType::Directory => {}
            tar::EntryType::Symlink | tar::EntryType::Link if self.links => {
                let target = entry.link_name().map_err(ArchiveError::Header)?.unwrap_or_default().into_owned();
                // A symlink resolves from its own folder, a hardlink from the
                // archive's root.
                let from = match kind {
                    tar::EntryType::Symlink => path.parent().unwrap_or(Path::new("")),
                    _ => Path::new(""),
                };
                if !inside_top_folder(from, &target) {
                    return Err(ArchiveError::LinkOutside { path, target });
                }
            }
            other => return Err(ArchiveError::EntryType { path, kind: format!("{other:?}").to_lowercase() }),
        }
        self.left = self.left.checked_sub(entry.size()).ok_or(ArchiveError::TooLarge { limit: self.limit })?;
        Ok(())
    }
}

fn stays_relative(path: &Path) -> bool {
    path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Whether `target`, followed from `from`, stays below the archive's first
/// folder.
fn inside_top_folder(from: &Path, target: &Path) -> bool {
    let mut depth = 0usize;
    for component in from.components().chain(target.components()) {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 1 => depth -= 1,
            _ => return false,
        }
    }
    depth >= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tarball with one entry, `kind` at `path`, linking to `target`.
    fn one_entry(kind: tar::EntryType, path: &str, target: &str, bytes: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        if !target.is_empty() {
            header.set_link_name(target).unwrap();
        }
        builder.append_data(&mut header, path, bytes).unwrap();
        builder.into_inner().unwrap()
    }

    fn checked(admission: &mut Admission, tarball: &[u8]) -> Result<(), ArchiveError> {
        let mut archive = tar::Archive::new(tarball);
        let entry = archive.entries().unwrap().next().unwrap().unwrap();
        admission.check(&entry)
    }

    #[test]
    fn a_protonfixes_link_into_a_sibling_folder_is_kept() {
        // As in GE-Proton11-7: protonfixes/gamefixes-umu/umu-306130.py.
        let tarball = one_entry(
            tar::EntryType::Symlink,
            "GE-Proton11-7/protonfixes/gamefixes-umu/umu-306130.py",
            "../gamefixes-egs/umu-306130.py",
            b"",
        );
        assert!(checked(&mut Admission::with_links(1 << 20), &tarball).is_ok());
    }

    #[test]
    fn a_link_out_of_the_build_is_refused() {
        for target in ["../../../.bashrc", "/home/user/.bashrc"] {
            let tarball = one_entry(tar::EntryType::Symlink, "GE-Proton11-7/files/bin/wine", target, b"");
            let refused = checked(&mut Admission::with_links(1 << 20), &tarball);
            assert!(matches!(refused, Err(ArchiveError::LinkOutside { .. })), "{target}");
        }
    }

    #[test]
    fn a_release_tarball_takes_no_links_at_all() {
        let tarball = one_entry(tar::EntryType::Symlink, "lightforge-1.0.0/lightforge", "lightforge-gui", b"");
        assert!(matches!(checked(&mut Admission::files_only(1 << 20), &tarball), Err(ArchiveError::EntryType { .. })));
    }

    #[test]
    fn a_device_node_is_refused() {
        let tarball = one_entry(tar::EntryType::Char, "GE-Proton11-7/null", "", b"");
        assert!(matches!(checked(&mut Admission::with_links(1 << 20), &tarball), Err(ArchiveError::EntryType { .. })));
    }

    #[test]
    fn an_archive_past_its_limit_is_refused() {
        let tarball = one_entry(tar::EntryType::Regular, "lightforge-1.0.0/lightforge", "", &[0; 2048]);
        let refused = checked(&mut Admission::files_only(1024), &tarball);
        assert!(matches!(refused, Err(ArchiveError::TooLarge { limit: 1024 })));
    }
}
