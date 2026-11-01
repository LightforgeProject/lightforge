//! The AppImage's route: one file, downloaded beside itself and renamed over
//! itself. The running copy keeps its mount until it exits, so a restart
//! finishes the job.

use std::path::{Path, PathBuf};

use super::{fetch_asset, io, UpgradeError};
use crate::net::Http;

/// The release asset for this processor, without a version, so
/// `releases/latest/download/` always has it and the file keeps the name the
/// user's shortcuts know: `Lightforge-x86_64.AppImage`.
pub fn asset() -> String {
    format!("Lightforge-{}.AppImage", std::env::consts::ARCH)
}

/// The AppImage this process runs from. The runtime sets `$APPIMAGE`;
/// `current_exe` is inside its mount, which is gone once Lightforge closes.
pub fn running() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|path| path.is_file())
}

/// Download `version`'s AppImage, check it, and put it in place of `path`.
pub async fn install(
    http: &Http,
    version: &str,
    path: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), UpgradeError> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    // In the same folder, so the swap is a rename on one filesystem.
    let staging = crate::staging::create_in(dir).map_err(io(dir))?;
    let fresh = fetch_asset(http, version, &asset(), staging.path(), on_progress).await?;
    replace(&fresh, path)
}

fn replace(fresh: &Path, path: &Path) -> Result<(), UpgradeError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(fresh, std::fs::Permissions::from_mode(0o755)).map_err(io(fresh))?;
    std::fs::rename(fresh, path).map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_appimage_takes_the_old_one_s_place_and_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(asset());
        std::fs::write(&path, b"old").unwrap();
        let staging = crate::staging::create_in(dir.path()).unwrap();
        let fresh = staging.path().join(asset());
        std::fs::write(&fresh, b"new").unwrap();

        replace(&fresh, &path).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
        drop(staging);
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, [std::ffi::OsString::from(asset())]);
    }
}
