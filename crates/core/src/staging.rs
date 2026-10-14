//! Staging folders: write somewhere nobody reads, then rename into place.
//!
//! A staging folder is made inside the directory its contents will end up
//! in (or beside it), never in `/tmp`, so the final step is a rename on one
//! filesystem rather than a copy that can fail halfway. It is a `TempDir`, so
//! it is removed on every path out of the install, including an error.
//!
//! A crash still leaves one behind. The next install in the same place
//! clears any that are old enough that no live install can own them.

use std::io;
use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;

/// What every staging folder's name starts with. A leading dot, so a file
/// manager hides it and nothing that lists builds or addons mistakes it for
/// one.
pub(crate) const PREFIX: &str = ".lightforge-staging-";

/// A staging folder this old was left by a run that died, not by one still
/// going: no unpack takes an hour.
const STALE: Duration = Duration::from_secs(60 * 60);

/// A fresh staging folder in `dir`, after clearing stale ones there.
pub(crate) fn create_in(dir: &Path) -> io::Result<TempDir> {
    sweep_stale(dir);
    tempfile::Builder::new().prefix(PREFIX).tempdir_in(dir)
}

/// Remove staging folders a crashed run left in `dir`. Best effort: a
/// leftover costs disk space, never correctness, so failing here is not a
/// failed install.
fn sweep_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_name().to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
