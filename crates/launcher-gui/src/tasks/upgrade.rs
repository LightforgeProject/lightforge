//! Updating Lightforge itself, off the UI thread; see
//! [`lightforge_core::upgrade`] for which copy is updated how.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lightforge_core::net::Http;
use lightforge_core::paths::AppDirs;
use lightforge_core::upgrade::{self, terminal, Origin, Route};

use super::Problem;

/// How far an update got without the user.
#[derive(Debug, Clone, PartialEq)]
pub enum Prepared {
    /// The new files are in place.
    Installed,
    /// A terminal is running `command`.
    Terminal { command: String },
    /// No terminal was found: `command` is for the user to run.
    NoTerminal { command: String },
}

/// Which file a path is, and when it was last written: a package manager
/// replaces the binary with a new file, an in-place write changes the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    inode: u64,
    modified: SystemTime,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { inode: meta.ino(), modified: meta.modified().ok()? })
}

/// Take `origin`'s route to `version`, publishing download progress to `tx`.
pub async fn run(
    origin: Origin,
    version: String,
    tx: tokio::sync::watch::Sender<(u64, Option<u64>)>,
) -> Result<Prepared, Problem> {
    let dirs = AppDirs::discover().map_err(|e| Problem::of(&e))?;
    let http = Http::new().map_err(|e| Problem::of(&e))?;
    let progress = move |done, total| {
        let _ = tx.send((done, total));
    };
    match (origin.route(&version), origin) {
        (Route::InPlace, Origin::Installer { bin_dir }) => {
            let scratch = dirs.cache_dir().join("downloads");
            upgrade::tarball::install(&http, &version, &bin_dir, &scratch, progress)
                .await
                .map_err(|e| Problem::of(&e))?;
            Ok(Prepared::Installed)
        }
        (Route::Terminal(package), _) => {
            let work = dirs.cache_dir().join("upgrade");
            let (script, command) = terminal::prepare(&http, &package, &version, &work, progress)
                .await
                .map_err(|e| Problem::of(&e))?;
            Ok(match terminal::open(&script) {
                Ok(()) => Prepared::Terminal { command },
                Err(_) => Prepared::NoTerminal { command: format!("sh {}", script.display()) },
            })
        }
        _ => Err("this copy of Lightforge is not one it can update itself".into()),
    }
}

/// Start `exe` on its own, for the old process to close behind it.
pub fn restart(exe: &PathBuf) -> std::io::Result<()> {
    std::process::Command::new(exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}
