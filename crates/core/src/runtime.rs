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

// ---------------------------------------------------------------------
// Telling the user what to run
// ---------------------------------------------------------------------

/// A distribution-specific way to get the 32-bit libraries.
///
/// Lightforge never runs this itself. Installing system packages needs root,
/// and a launcher that invokes a package manager as root on a guess about the
/// user's distribution is a trust escalation this project does not need to
/// make — especially since on the distributions where a button would work the
/// user can run one command, and on the ones where help is most needed
/// (read-only SteamOS, rpm-ostree, NixOS) no button could work at all.
///
/// So this is text to copy, and [`Remedy::distro`] is shown beside it so the
/// user can see what was guessed and ignore it if the guess is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remedy {
    /// What this system appears to be, in its own words.
    pub distro: String,
    /// The command to run, when there is one worth printing.
    pub command: Option<String>,
    /// What the command alone does not cover.
    pub note: Option<String>,
}

/// The relevant fields of `/etc/os-release`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub id_like: String,
    pub pretty_name: String,
}

impl OsRelease {
    pub fn read() -> Self {
        Self::parse(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
    }

    pub fn parse(text: &str) -> Self {
        let mut os = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_owned();
            match key.trim() {
                "ID" => os.id = value,
                "ID_LIKE" => os.id_like = value,
                "PRETTY_NAME" => os.pretty_name = value,
                _ => {}
            }
        }
        os
    }

    /// Whether this system is, or is derived from, `family`.
    fn is(&self, family: &str) -> bool {
        self.id == family || self.id_like.split_whitespace().any(|l| l == family)
    }
}

/// What to tell this machine's owner to run.
pub fn remedy() -> Remedy {
    remedy_for(&OsRelease::read())
}

/// The table, kept deliberately short.
///
/// Every entry here is a maintenance liability — package names move and
/// distributions fork — so this covers the families that actually ship WoW
/// players and says nothing at all about the rest, rather than guessing.
/// An unrecognised system gets [`ADVICE`] and no command, which is honest and
/// still actionable.
pub fn remedy_for(os: &OsRelease) -> Remedy {
    let distro = match os.pretty_name.is_empty() {
        false => os.pretty_name.clone(),
        true => match os.id.is_empty() {
            false => os.id.clone(),
            true => "this system".to_owned(),
        },
    };

    let (command, note) = if os.id == "nixos" {
        // Declarative: there is no imperative command to copy, and telling
        // someone to run one would be actively wrong.
        (
            None,
            Some(
                "NixOS is configured declaratively — add `wine` to \
                 `environment.systemPackages` (or enable `programs.steam`, \
                 which pulls the same 32-bit libraries) and rebuild"
                    .to_owned(),
            ),
        )
    } else if os.id == "steamos" || os.is("steamos") {
        // SteamOS already ships the 32-bit stack, so reaching this is odd —
        // and its rootfs is read-only, so `pacman` is the wrong instruction.
        (
            None,
            Some(
                "SteamOS ships these libraries already and its system \
                 partition is read-only — if they are genuinely missing, \
                 `steamos-readonly disable` is the documented route, at your \
                 own risk"
                    .to_owned(),
            ),
        )
    } else if os.is("debian") {
        // Debian and Ubuntu ship no i386 packages at all until the
        // architecture is enabled, which is the whole reason this check
        // exists. Installing Steam does these three steps as a side effect.
        (
            Some(
                "sudo dpkg --add-architecture i386 && sudo apt update && sudo apt install wine"
                    .to_owned(),
            ),
            None,
        )
    } else if os.is("arch") {
        (
            Some("sudo pacman -S wine".to_owned()),
            Some(
                "`wine` lives in the multilib repository — if pacman cannot \
                 find it, uncomment the `[multilib]` section in \
                 /etc/pacman.conf first"
                    .to_owned(),
            ),
        )
    } else if os.id == "fedora" || os.is("fedora") {
        (Some("sudo dnf install wine".to_owned()), None)
    } else if os.is("suse") || os.is("opensuse") {
        (Some("sudo zypper install wine".to_owned()), None)
    } else {
        (None, None)
    };

    Remedy { distro, command, note }
}

#[cfg(test)]
mod remedy_tests {
    use super::*;

    fn os(id: &str, id_like: &str) -> OsRelease {
        OsRelease {
            id: id.into(),
            id_like: id_like.into(),
            pretty_name: format!("{id} (test)"),
        }
    }

    #[test]
    fn os_release_parses_quotes_and_ignores_the_rest() {
        let text = "NAME=\"Debian GNU/Linux\"\nID=debian\nPRETTY_NAME=\"Debian GNU/Linux 13\"\nVERSION_ID=\"13\"\n";
        let parsed = OsRelease::parse(text);
        assert_eq!(parsed.id, "debian");
        assert_eq!(parsed.pretty_name, "Debian GNU/Linux 13");
        assert!(parsed.id_like.is_empty());
    }

    /// The case that started all this: Ubuntu is `ID=ubuntu ID_LIKE=debian`,
    /// and must get Debian's answer rather than falling through to nothing.
    #[test]
    fn derivatives_inherit_their_family() {
        let ubuntu = remedy_for(&os("ubuntu", "debian"));
        assert!(ubuntu.command.as_deref().unwrap().contains("add-architecture i386"));

        let mint = remedy_for(&os("linuxmint", "ubuntu debian"));
        assert_eq!(mint.command, ubuntu.command);

        let endeavour = remedy_for(&os("endeavouros", "arch"));
        assert!(endeavour.command.as_deref().unwrap().contains("pacman"));
    }

    #[test]
    fn debian_enables_the_architecture_before_installing() {
        let remedy = remedy_for(&os("debian", ""));
        let command = remedy.command.unwrap();
        // Order matters: `apt install wine` alone does nothing useful on a
        // machine that has never had i386 enabled.
        let add = command.find("add-architecture").unwrap();
        let install = command.find("apt install").unwrap();
        assert!(add < install, "the architecture has to be enabled first: {command}");
    }

    #[test]
    fn arch_warns_about_multilib() {
        let remedy = remedy_for(&os("arch", ""));
        assert!(remedy.note.unwrap().contains("multilib"));
    }

    /// Distributions where running a package manager would be wrong get a
    /// note and no command, so the UI shows no button to press.
    #[test]
    fn immutable_and_declarative_systems_get_no_command() {
        for id in ["nixos", "steamos"] {
            let remedy = remedy_for(&os(id, ""));
            assert!(remedy.command.is_none(), "{id} must not be handed a command");
            assert!(remedy.note.is_some(), "{id} still has to say something useful");
        }
    }

    #[test]
    fn an_unknown_system_says_nothing_rather_than_guessing() {
        let remedy = remedy_for(&os("某", ""));
        assert!(remedy.command.is_none());
        assert!(remedy.note.is_none());
    }

    #[test]
    fn the_distro_name_falls_back_sensibly() {
        assert_eq!(remedy_for(&OsRelease::default()).distro, "this system");
        let id_only = OsRelease { id: "debian".into(), ..Default::default() };
        assert_eq!(remedy_for(&id_only).distro, "debian");
    }
}
