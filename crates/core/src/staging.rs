//! Staging folders: write somewhere nobody reads, then rename into place.
//!
//! Made in or beside the destination, never `/tmp`, so the last step is a
//! rename. A `TempDir`, removed on every path out; a crash's leftovers are
//! cleared once stale.

use std::io;
use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;

/// What every staging folder's name starts with; the dot hides it.
pub(crate) const PREFIX: &str = ".lightforge-staging-";

/// A staging folder this old was left by a dead run.
const STALE: Duration = Duration::from_secs(60 * 60);

/// A fresh staging folder in `dir`, after clearing stale ones there.
pub(crate) fn create_in(dir: &Path) -> io::Result<TempDir> {
    sweep_stale(dir);
    tempfile::Builder::new().prefix(PREFIX).tempdir_in(dir)
}

/// Remove staging folders a crashed run left in `dir`, best effort.
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
