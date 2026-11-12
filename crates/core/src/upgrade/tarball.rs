//! The installer's route: Lightforge replaces its own files in the home
//! folder, as running `install.sh` again would.
//!
//! The script's steps: binaries into `~/.local/bin`, the menu entry, icons.
//! Each binary is renamed over the old one; the running process keeps its
//! file until it exits, so a restart finishes the job.

use std::path::{Path, PathBuf};

use super::{fetch_asset, io, UpgradeError};
use crate::net::Http;

/// The menu entry's file name, which is also the app id.
const ENTRY: &str = "pro.tuxcore.Lightforge.desktop";

/// beta.11 unpacks to 39 MB.
const MAX_UNPACKED: u64 = 512 << 20;

/// What every release tarball holds at its top.
const BINARIES: [&str; 2] = ["lightforge-gui", "lightforge"];

/// Download `version`'s tarball, check it, and install it over the copy in
/// `bin_dir`; the menu entry and icons go where `install.sh` puts them,
/// under `$XDG_DATA_HOME`. `scratch` is where the download waits.
pub async fn install(
    http: &Http,
    version: &str,
    bin_dir: &Path,
    scratch: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), UpgradeError> {
    let data_dir = directories::BaseDirs::new()
        .map(|d| d.data_dir().to_path_buf())
        .ok_or_else(|| UpgradeError::Io { path: bin_dir.to_path_buf(), source: std::io::ErrorKind::NotFound.into() })?;
    let suffix = format!("-{}-linux.tar.gz", std::env::consts::ARCH);
    let archive = fetch_asset(http, version, &suffix, scratch, on_progress).await?;
    let (bin_dir, held) = (bin_dir.to_path_buf(), archive.clone());
    let installed = tokio::task::spawn_blocking(move || put_in_place(&held, &bin_dir, &data_dir))
        .await
        .map_err(|e| UpgradeError::Io { path: archive.clone(), source: std::io::Error::other(e) });
    let _ = std::fs::remove_file(&archive);
    installed?
}

/// Unpack `archive` beside the binaries it replaces, then move each file
/// into place.
fn put_in_place(archive: &Path, bin_dir: &Path, data_dir: &Path) -> Result<(), UpgradeError> {
    std::fs::create_dir_all(bin_dir).map_err(io(bin_dir))?;
    // In `bin_dir` itself, so every move below is a rename on one filesystem.
    let staging = crate::staging::create_in(bin_dir).map_err(io(bin_dir))?;
    let release = unpack(archive, staging.path())?;

    for name in BINARIES {
        let staged = release.join(name);
        set_executable(&staged).map_err(io(&staged))?;
        std::fs::rename(&staged, bin_dir.join(name)).map_err(io(&bin_dir.join(name)))?;
    }

    // Full path: `~/.local/bin` is not on every session's PATH.
    let apps = data_dir.join("applications");
    std::fs::create_dir_all(&apps).map_err(io(&apps))?;
    let entry = std::fs::read_to_string(release.join(ENTRY)).map_err(io(&release.join(ENTRY)))?;
    let entry = with_full_path(&entry, &bin_dir.join("lightforge-gui"));
    let written = staging.path().join(ENTRY);
    std::fs::write(&written, entry).map_err(io(&written))?;
    // Not a rename: the applications folder can be on another filesystem.
    std::fs::copy(&written, apps.join(ENTRY)).map_err(io(&apps.join(ENTRY)))?;

    crate::fsutil::copy_tree(&release.join("icons/hicolor"), &data_dir.join("icons/hicolor"))
        .map_err(|(path, source)| UpgradeError::Io { path, source })?;
    refresh_menu(&apps);
    Ok(())
}

/// The menu entry naming `gui` by its full path, as `install.sh` writes it:
/// `Exec` quoted the way the Desktop Entry spec asks, `TryExec` a plain
/// string. A home folder with a space in it is the case that happens.
fn with_full_path(entry: &str, gui: &Path) -> String {
    let path = gui.display().to_string();
    // Backslashes first: the other escapes add their own.
    let quoted = path
        .replace('\\', r"\\\\")
        .replace('"', r#"\\""#)
        .replace('`', r"\\`")
        .replace('$', r"\\$")
        .replace('%', "%%");
    let mut written = String::with_capacity(entry.len() + 2 * path.len());
    for line in entry.lines() {
        match line {
            "Exec=lightforge-gui" => written.push_str(&format!("Exec=\"{quoted}\"")),
            "TryExec=lightforge-gui" => written.push_str(&format!("TryExec={}", path.replace('\\', r"\\"))),
            other => written.push_str(other),
        }
        written.push('\n');
    }
    written
}

/// Extract a release tarball into `into`, returning its one top folder.
fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, UpgradeError> {
    let file = std::fs::File::open(archive).map_err(io(archive))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    // Files only, so making the binaries executable follows no link.
    let mut admission = crate::archive::Admission::files_only(MAX_UNPACKED);
    for entry in tar.entries().map_err(io(archive))? {
        let mut entry = entry.map_err(io(archive))?;
        admission.check(&entry)?;
        entry.unpack_in(into).map_err(io(archive))?;
    }

    let top = std::fs::read_dir(into).map_err(io(archive))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("lightforge-")));
    match top {
        Some(top) if BINARIES.iter().all(|b| top.join(b).is_file()) && top.join(ENTRY).is_file() => Ok(top),
        _ => Err(UpgradeError::NotARelease(archive.to_path_buf())),
    }
}

fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// Plasma and GNOME watch the applications folder themselves; these only
/// make the menu catch up sooner where the tools exist, as in `install.sh`.
fn refresh_menu(apps: &Path) {
    let quiet = |program: &str, args: &[&std::ffi::OsStr]| {
        let _ = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    quiet("update-desktop-database", &["-q".as_ref(), apps.as_os_str()]);
    quiet("kbuildsycoca6", &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tarball shaped like a release, written with the same crates that
    /// read it.
    fn release_tarball(dir: &Path, top: &str, entry: &str) -> PathBuf {
        let tree = dir.join("tree").join(top);
        std::fs::create_dir_all(tree.join("icons/hicolor/64x64/apps")).unwrap();
        std::fs::write(tree.join("lightforge-gui"), "new gui").unwrap();
        std::fs::write(tree.join("lightforge"), "new cli").unwrap();
        std::fs::write(tree.join(ENTRY), entry).unwrap();
        std::fs::write(tree.join("icons/hicolor/64x64/apps/pro.tuxcore.Lightforge.png"), "png").unwrap();

        let path = dir.join("release.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        tar.append_dir_all(top, &tree).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn the_entry_quotes_a_home_folder_as_install_sh_does() {
        let entry = "[Desktop Entry]\nExec=lightforge-gui\nTryExec=lightforge-gui\nName=Lightforge\n";
        let written = with_full_path(entry, Path::new("/home/a b/x|y&z$q/lightforge-gui"));
        assert_eq!(
            written,
            "[Desktop Entry]\n\
             Exec=\"/home/a b/x|y&z\\\\$q/lightforge-gui\"\n\
             TryExec=/home/a b/x|y&z$q/lightforge-gui\n\
             Name=Lightforge\n"
        );
    }

    #[test]
    fn a_release_replaces_both_binaries_the_entry_and_the_icons() {
        let dir = std::env::temp_dir().join(format!("lightforge-tarball-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (bin, data) = (dir.join("home/.local/bin"), dir.join("home/.local/share"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("lightforge-gui"), "old gui").unwrap();
        let archive = release_tarball(
            &dir,
            "lightforge-1.0.0-beta.7",
            "[Desktop Entry]\nExec=lightforge-gui\nTryExec=lightforge-gui\n",
        );

        put_in_place(&archive, &bin, &data).unwrap();

        assert_eq!(std::fs::read_to_string(bin.join("lightforge-gui")).unwrap(), "new gui");
        assert_eq!(std::fs::read_to_string(bin.join("lightforge")).unwrap(), "new cli");
        let entry = std::fs::read_to_string(data.join("applications").join(ENTRY)).unwrap();
        let gui = bin.join("lightforge-gui");
        assert!(entry.contains(&format!("Exec=\"{}\"", gui.display())), "{entry}");
        assert!(entry.contains(&format!("TryExec={}", gui.display())), "{entry}");
        assert!(data.join("icons/hicolor/64x64/apps/pro.tuxcore.Lightforge.png").is_file());
        // Nothing of the unpack is left beside the binaries.
        let left: Vec<_> = std::fs::read_dir(&bin).unwrap().filter_map(Result::ok).map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn something_that_is_not_a_release_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("lightforge-tarball-not-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("lightforge-gui"), "old gui").unwrap();
        let archive = release_tarball(&dir, "something-else", "[Desktop Entry]\n");

        assert!(matches!(put_in_place(&archive, &bin, &dir.join("share")), Err(UpgradeError::NotARelease(_))));
        assert_eq!(std::fs::read_to_string(bin.join("lightforge-gui")).unwrap(), "old gui");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
