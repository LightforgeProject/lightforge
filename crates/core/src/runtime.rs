//! Whether this machine can run 32-bit Windows binaries at all.
//!
//! Proton is two wines: a 64-bit one and a 32-bit one. Everything Lightforge
//! does to *build* a prefix — `wineboot -u`, the registry edits, `wineserver`
//! — is 64-bit and works on a bare 64-bit system. `Battle.net-Setup.exe` is
//! 32-bit, and it is the first thing in the whole sequence that is.
//!
//! So a machine with no 32-bit userspace gets all the way through Proton
//! provisioning and prefix creation looking perfectly healthy, then dies in
//! under a second on the Battle.net step with nothing in the UI to explain
//! it. That is not hypothetical: it is what a stock Debian does, because
//! Debian ships no i386 packages until something enables the architecture.
//! Installing Steam appears to "fix" Lightforge, which is a coincidence — the
//! `steam` package is multi-arch, so installing it runs
//! `dpkg --add-architecture i386` and pulls in the 32-bit chain as a side
//! effect. Nobody should need Steam to run this launcher.
//!
//! Two checks, because they catch different failures and cost different
//! amounts:
//!
//! 1. [`check_loader`] — is there a 32-bit dynamic loader at all? One `stat`,
//!    no Proton required, so it runs *before* a 400 MB download the user
//!    could not have used.
//! 2. [`check_wine`] — can Proton's own 32-bit wine actually start? Needs
//!    Proton on disk, and catches the subtler case where i386 is enabled but
//!    an individual library is missing. The loader names that library, so its
//!    stderr is kept rather than summarised.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

/// Where a 32-bit ELF interpreter lives, across the layouts in use.
///
/// Debian and Ubuntu put it at `/lib/ld-linux.so.2` (multiarch, from
/// `libc6:i386`); Arch and Fedora use the `lib32`/`usr/lib32` spellings. Any
/// one of them is enough — this is a question of "is there a 32-bit userspace
/// here", not of which distro we are on.
const LOADER_CANDIDATES: &[&str] = &[
    "/lib/ld-linux.so.2",
    "/lib32/ld-linux.so.2",
    "/usr/lib/ld-linux.so.2",
    "/usr/lib32/ld-linux.so.2",
];

/// What to tell someone whose 32-bit runtime is missing.
///
/// Deliberately points at the distribution's own `wine` package rather than
/// naming individual libraries. Lightforge never invokes that wine — it wants
/// the dependency closure that installing it pulls in. One command, it exists
/// on every distribution, and unlike a hand-maintained list of
/// `libfreetype6:i386`-style names it does not rot as distros rename things.
pub const ADVICE: &str = "install your distribution's `wine` package: Lightforge does not use it, \
but installing it pulls in the 32-bit libraries Proton needs";

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(
        "this system has no 32-bit runtime, which Proton needs to install and run Battle.net \
         (no 32-bit loader at any of {candidates}) — {ADVICE}"
    )]
    NoLoader { candidates: String },

    #[error(
        "Proton's 32-bit wine at {wine} could not start{}, so Battle.net cannot be installed — \
         {ADVICE}\n{stderr}",
        match code { Some(c) => format!(" (exit {c})"), None => String::new() }
    )]
    WineFailed {
        wine: PathBuf,
        code: Option<i32>,
        stderr: String,
    },

    #[error("could not run {wine}")]
    Spawn {
        wine: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Is there a 32-bit dynamic loader on this system?
///
/// Cheap enough to call before anything is downloaded. Only meaningful on
/// x86-64: on any other architecture Proton is not going to run regardless,
/// and a missing `ld-linux.so.2` says nothing useful, so the check passes
/// rather than inventing a failure it cannot explain.
pub fn check_loader() -> Result<(), RuntimeError> {
    if !cfg!(target_arch = "x86_64") {
        return Ok(());
    }
    if LOADER_CANDIDATES.iter().any(|p| Path::new(p).exists()) {
        return Ok(());
    }
    Err(RuntimeError::NoLoader {
        candidates: LOADER_CANDIDATES.join(", "),
    })
}

/// Can Proton's 32-bit wine actually start?
///
/// `dist` is Proton's `files`/`dist` directory. Runs `wine --version`, which
/// touches no prefix and returns immediately; what matters is whether the
/// dynamic loader can resolve the binary's libraries, and its complaint is
/// passed through verbatim because it names the library that is missing.
///
/// A Proton build with no 32-bit wine at all is not an error here — some
/// builds are 64-bit only, and that is a question about the build rather than
/// about the system this code is checking.
pub fn check_wine(dist: &Path) -> Result<(), RuntimeError> {
    let wine = dist.join("bin").join("wine");
    if !wine.is_file() {
        return Ok(());
    }

    let output = Command::new(&wine)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|source| RuntimeError::Spawn { wine: wine.clone(), source })?;

    if output.status.success() {
        return Ok(());
    }

    Err(RuntimeError::WineFailed {
        wine,
        code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// Whether a 32-bit runtime is present, for a debug bundle or a settings
/// panel that wants to state a fact rather than fail.
pub fn has_32bit_loader() -> bool {
    check_loader().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_advice_names_no_distro_specific_package() {
        // The whole point of pointing at `wine` is that it is the one name
        // every distribution shares. A library name creeping in here is the
        // beginning of a list that will rot.
        for rotting in ["i386", "apt", "dnf", "pacman", "libc6", "dpkg"] {
            assert!(
                !ADVICE.contains(rotting),
                "advice mentions `{rotting}`, which ties it to one distribution"
            );
        }
    }

    #[test]
    fn a_missing_wine_binary_is_not_a_system_problem() {
        let empty = std::env::temp_dir().join("lightforge-runtime-empty");
        let _ = std::fs::create_dir_all(&empty);
        assert!(check_wine(&empty).is_ok());
    }

    #[test]
    fn a_wine_that_fails_reports_its_own_stderr() {
        let dir = std::env::temp_dir().join("lightforge-runtime-failing");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let wine = bin.join("wine");
        std::fs::write(
            &wine,
            "#!/bin/sh\necho 'ld: libfreetype.so.6: cannot open shared object file' >&2\nexit 127\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wine, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let err = check_wine(&dir).unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("libfreetype.so.6"),
            "the loader names the missing library; that must survive to the user: {rendered}"
        );
        assert!(rendered.contains("wine"), "the advice must be in the message: {rendered}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_machine_is_reported_consistently() {
        assert_eq!(has_32bit_loader(), check_loader().is_ok());
    }
}
