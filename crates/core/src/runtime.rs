//! Whether this machine can run 32-bit Windows binaries at all.
//!
//! Proton is two wines. Everything Lightforge does to build a prefix is
//! 64-bit and works on a bare 64-bit system; `Battle.net-Setup.exe` is the
//! only 32-bit binary in the whole sequence. Without a 32-bit userspace the
//! prefix provisions fine and then the Battle.net step dies in under a
//! second with nothing to explain it. Stock Debian does this, because it
//! ships no i386 packages until something enables the architecture.
//! (Installing Steam appears to fix Lightforge; it doesn't. The `steam`
//! package is multi-arch, so it runs `dpkg --add-architecture i386` as a
//! side effect.)
//!
//! Two checks, because they catch different failures at different cost:
//!
//! 1. [`check_loader`] stats for a 32-bit dynamic loader. No Proton needed,
//!    so it runs before a 400 MB download the user could not have used.
//! 2. [`check_wine`] starts Proton's own 32-bit wine. Catches the case where
//!    i386 is enabled but a library is missing; the loader names that
//!    library, so its stderr is kept rather than summarised.
//!
//! Both only establish a floor. They cannot see the 32-bit *graphics* stack,
//! which is dlopened at run time and so appears in neither the loader's view
//! nor `ldd`'s: measured on Fedora 44, `glibc.i686` + `freetype.i686` turns
//! both checks green while the installer still exits 1 for want of 32-bit
//! X11 and GL. That is why [`remedy_for`] names the distribution's 32-bit
//! *Wine* package instead of a library list: Wine's dependencies are the
//! closure we need, maintained by people who package it for a living.
//!
//! When the floor is met and the install still fails,
//! [`crate::battlenet::BattleNetError::InstallerExited`] carries the
//! installer's own stderr. That is the backstop, and it found both bugs here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

/// Where a 32-bit ELF interpreter lives, across the layouts in use.
///
/// Debian and Ubuntu put it at `/lib/ld-linux.so.2` (multiarch, from
/// `libc6:i386`); Arch and Fedora use the `lib32`/`usr/lib32` spellings. Any
/// one of them is enough. This is a question of "is there a 32-bit userspace
/// here", not of which distro we are on.
const LOADER_CANDIDATES: &[&str] = &[
    "/lib/ld-linux.so.2",
    "/lib32/ld-linux.so.2",
    "/usr/lib/ld-linux.so.2",
    "/usr/lib32/ld-linux.so.2",
];

/// What to tell someone whose 32-bit runtime is missing and whose
/// distribution we do not recognise.
///
/// This used to say "install your distribution's wine package", on the theory
/// that it would pull the 32-bit dependency closure as a side effect. That is
/// true on Debian and **false on Fedora 44**, where `dnf install wine`
/// installs only `noarch` and `x86_64` packages and leaves the machine with
/// no 32-bit loader at all. A generic trick that is wrong on a current
/// mainstream distribution is worse than no trick, so this now names what is
/// actually required and lets the user find it in their own package manager.
pub const ADVICE: &str = "install the 32-bit (i686/i386) build of Wine that your distribution \
ships — Lightforge does not use it, but it pulls in the whole 32-bit library stack Proton needs";

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

    #[error(
        "Battle.net would have no internet here: this system has no 32-bit gnutls, which Wine \
         needs for HTTPS{}",
        match install {
            Some(appid) => format!(
                ", and the Steam Linux Runtime that would bring its own is not installed \
                 (install it in Steam: steam://install/{appid})"
            ),
            None => format!(" — {ADVICE}"),
        }
    )]
    NoTls {
        /// The Steam Linux Runtime the Proton build asks for, when Steam is
        /// here to install it.
        install: Option<u32>,
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
/// A Proton build with no 32-bit wine at all is not an error here, some
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
    fn the_fallback_advice_names_no_package_manager() {
        // The fallback is for distributions this table does not know, so it
        // must not assume one's tooling. It may name the *libraries*: those
        // are the actual requirement and every distro ships them under some
        // spelling, but naming a package manager would make it wrong for
        // everyone it is meant to serve.
        for rotting in ["apt", "dnf", "pacman", "zypper", "dpkg", "emerge"] {
            assert!(
                !ADVICE.contains(rotting),
                "fallback advice mentions `{rotting}`, which ties it to one distribution"
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
/// make, especially since on the distributions where a button would work the
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

/// The relevant fields of `/etc/os-release`, and whether the system is an
/// image-based one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub id_like: String,
    pub pretty_name: String,
    /// Booted from an OSTree image: Silverblue, Kinoite, Bazzite and the
    /// other Fedora Atomic variants. `/etc/os-release` says `fedora` on all
    /// of them, so it cannot tell them apart from a plain install, and there
    /// the package manager everyone knows cannot install anything.
    pub atomic: bool,
}

impl OsRelease {
    /// The running system's, or the file `LIGHTFORGE_OS_RELEASE` names.
    ///
    /// The override is for development: it lets one machine walk the paths
    /// meant for another, the SteamOS setup question on an Arch desktop,
    /// without a VM. Only the file is swapped; `atomic` is still read from
    /// the real boot.
    pub fn read() -> Self {
        let path = std::env::var_os("LIGHTFORGE_OS_RELEASE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/etc/os-release".into());
        Self {
            // The marker OSTree itself writes at boot, and the one
            // rpm-ostree checks before it agrees to run.
            atomic: std::path::Path::new("/run/ostree-booted").exists(),
            ..Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
        }
    }

    /// Valve's own SteamOS, on a Deck, a Steam Machine, or a desktop.
    ///
    /// The ID alone, not `ID_LIKE`: distributions built on SteamOS's base
    /// are their own thing and are not asked the SteamOS question.
    pub fn is_steamos(&self) -> bool {
        self.id == "steamos"
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

    fn is_fedora(&self) -> bool {
        self.is("fedora")
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
/// Every entry here is a maintenance liability, package names move and
/// distributions fork, so this covers the families that actually ship WoW
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
                "NixOS is configured declaratively — set \
                 `hardware.graphics.enable32Bit = true;` (named \
                 `hardware.opengl.driSupport32Bit` before 24.11) and \
                 rebuild. Note that nixpkgs' `wine` is 64-bit only; \
                 `wineWowPackages` is the 32-bit-capable set"
                    .to_owned(),
            ),
        )
    } else if os.id == "steamos" || os.is("steamos") {
        // SteamOS already ships the 32-bit stack, so reaching this is odd,
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
    } else if os.atomic && os.is_fedora() {
        // UNVERIFIED. The root is a read-only image, so `dnf install` fails
        // outright; packages are layered onto a new deployment instead.
        // `--apply-live` also applies it to the running system, because this
        // advice appears in the middle of Battle.net setup and a reboot there
        // breaks the flow. It is meant for exactly this, a pure addition, but
        // some images refuse it, hence the fallback in the note. Same package
        // as plain Fedora, for the same reason: the i686 Wine pulls the whole
        // 32-bit closure the installer needs.
        (
            Some("rpm-ostree install --apply-live wine.i686".to_owned()),
            Some(
                "Fedora Atomic layers packages onto its image; `--apply-live` \
                 makes them usable without a restart. If it refuses, run the \
                 same command without it and restart. `dnf install` cannot \
                 work on this system"
                    .to_owned(),
            ),
        )
    } else if os.atomic {
        // An image-based system that is not Fedora's: no layering tool we
        // could name with any confidence, and a mutable-system command would
        // be wrong.
        (
            None,
            Some(
                "This system boots from a read-only image, so the usual package \
                 manager cannot install these. Add a 32-bit Wine the way your \
                 distribution layers extra packages"
                    .to_owned(),
            ),
        )
    } else if os.is("debian") {
        // UNVERIFIED. Debian and Ubuntu ship no i386 packages at all until
        // the architecture is enabled, which is why the first half exists;
        // `wine32` is the i386 build and pulls the 32-bit stack with it.
        // Installing Steam performs the equivalent as a side effect, which is
        // why Steam appears to "fix" Lightforge on Debian.
        (
            Some(
                "sudo dpkg --add-architecture i386 && sudo apt update && \
                 sudo apt install wine32:i386"
                    .to_owned(),
            ),
            None,
        )
    } else if os.is("arch") {
        // UNVERIFIED. Arch's `wine` is built against multilib and pulls its
        // lib32-* dependencies, so no separate 32-bit package exists.
        (
            Some("sudo pacman -S wine".to_owned()),
            Some(
                "`wine` lives in the multilib repository — if pacman cannot \
                 find it, uncomment the `[multilib]` section in \
                 /etc/pacman.conf first"
                    .to_owned(),
            ),
        )
    } else if os.is_fedora() {
        // VERIFIED on Fedora 44, after two wrong answers worth recording:
        //
        // `dnf install wine` resolves to x86_64 and installs nothing 32-bit
        // at all, not even glibc.i686, so the machine still has no loader.
        //
        // `dnf install glibc.i686 freetype.i686` produces the loader and is
        // enough for `wine --version`, so both of this module's checks go
        // green, and the Battle.net installer still exits 1, because it is a
        // GUI application and needs the 32-bit X11, GL and audio stacks that
        // those two packages do not bring.
        //
        // Naming the i686 build of wine is what pulls the whole closure.
        (Some("sudo dnf install wine.i686".to_owned()), None)
    } else if os.is("suse") || os.is("opensuse") {
        // UNVERIFIED.
        (Some("sudo zypper install wine-32bit".to_owned()), None)
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
            atomic: false,
        }
    }

    fn atomic(id: &str, id_like: &str) -> OsRelease {
        OsRelease { atomic: true, ..os(id, id_like) }
    }

    /// Bazzite and Silverblue report `ID=fedora`, and `dnf install` on a
    /// read-only image is an instruction that cannot work.
    #[test]
    fn atomic_fedora_layers_with_rpm_ostree() {
        for (id, like) in [("fedora", ""), ("bazzite", "fedora")] {
            let remedy = remedy_for(&atomic(id, like));
            let command = remedy.command.unwrap();
            assert!(command.starts_with("rpm-ostree install --apply-live"), "{id}: {command}");
            assert!(command.contains("wine.i686"), "{id}: {command}");
            // The fallback when an image refuses a live apply.
            assert!(remedy.note.unwrap().contains("restart"), "{id} must name the reboot fallback");
        }
    }

    #[test]
    fn a_plain_fedora_still_uses_dnf() {
        let command = remedy_for(&os("fedora", "")).command.unwrap();
        assert!(command.starts_with("sudo dnf install"), "{command}");
    }

    #[test]
    fn another_image_based_system_gets_a_note_and_no_command() {
        let remedy = remedy_for(&atomic("endless", ""));
        assert!(remedy.command.is_none());
        assert!(remedy.note.is_some());
    }

    /// The regression this table was corrected for, twice over.
    ///
    /// `dnf install wine` on Fedora 44 resolves to x86_64 and installs
    /// nothing 32-bit, so the arch has to be named. The command must survive
    /// both mistakes: no bare `install wine`, and an explicit i686.
    #[test]
    fn fedora_names_the_i686_arch_explicitly() {
        let command = remedy_for(&os("fedora", "")).command.unwrap();
        assert!(command.contains("i686"), "must name the 32-bit arch: {command}");
        assert!(
            !command.ends_with("install wine"),
            "bare `install wine` resolves to x86_64 on Fedora and fixes nothing: {command}"
        );
    }

    /// Every command asks for a 32-bit *Wine*, not a hand-picked list of
    /// libraries.
    ///
    /// Picking libraries individually was tried and is a trap: glibc plus
    /// freetype is enough to make this module's own checks pass, and still
    /// not enough to run the Battle.net installer, which is a GUI program
    /// wanting the 32-bit X11, GL and audio stacks too. Letting the
    /// distribution's 32-bit Wine package resolve that closure is the only
    /// version that does not require us to maintain a graphics-stack
    /// dependency list per distro.
    #[test]
    fn every_command_installs_a_32_bit_wine() {
        for id in ["debian", "ubuntu", "arch", "fedora", "opensuse-tumbleweed"] {
            let os = OsRelease {
                id: id.into(),
                id_like: match id {
                    "ubuntu" => "debian".into(),
                    "opensuse-tumbleweed" => "suse".into(),
                    _ => String::new(),
                },
                pretty_name: id.into(),
                atomic: false,
            };
            let remedy = remedy_for(&os);
            let command = remedy.command.unwrap_or_default();
            assert!(command.contains("wine"), "{id} installs no wine: {command}");
            // Arch is the exception that proves the rule: its `wine` is built
            // against multilib, so the package name carries no arch suffix
            // and the note explains the repository instead.
            let names_arch = ["i386", "i686", "32bit", "lib32"]
                .iter()
                .any(|m| command.contains(m));
            assert!(
                names_arch || remedy.note.is_some_and(|n| n.contains("multilib")),
                "{id} neither names a 32-bit build nor explains why it need not: {command}"
            );
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

    /// SteamOS by its own ID only; something built on it is not asked.
    #[test]
    fn steamos_is_recognised_by_its_id_alone() {
        assert!(os("steamos", "arch").is_steamos());
        assert!(!os("arch", "").is_steamos());
        assert!(!os("chimeraos", "steamos arch").is_steamos());
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

// ---------------------------------------------------------------------
// Steam's container, for the TLS the host lacks
// ---------------------------------------------------------------------
//
// Wine does HTTPS through the host's gnutls, which it dlopens, so neither
// check above sees it. SteamOS ships the 32-bit stack Steam itself needs
// and no 32-bit gnutls, and there `Battle.net-Setup.exe` fails its first
// request in a second (`BLZBNTBTS00000028`) and calls it "no connection".
// Steam never hits this because it runs Proton inside the Steam Linux
// Runtime, which brings its own libraries. The installer is not the only
// 32-bit program: `Battle.net.exe` and `Agent.exe` are too, and without a
// TLS stack the client still draws its login page (that is the embedded
// browser's own TLS) and then never finishes signing in. So everything
// Battle.net runs goes into the container, and only when it has to. The
// games are 64-bit and stay on the host.

/// The runtime a Proton build asks Steam to run it in, from its
/// `toolmanifest.vdf`: `4183110` (Steam Linux Runtime 4.0) for GE-Proton 11.
pub fn required_runtime(proton_root: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(proton_root.join("toolmanifest.vdf")).ok()?;
    vdf_value(&text, "require_tool_appid")?.parse().ok()
}

/// The runtime's entry point, if Steam has `appid` installed in any of
/// `libraries`. Found through the app manifest's `installdir` rather than a
/// folder name, which differs between runtimes.
pub fn find_steam_runtime(libraries: &[PathBuf], appid: u32) -> Option<PathBuf> {
    libraries.iter().find_map(|library| {
        let steamapps = library.join("steamapps");
        let manifest =
            std::fs::read_to_string(steamapps.join(format!("appmanifest_{appid}.acf"))).ok()?;
        let entry = steamapps.join("common").join(vdf_value(&manifest, "installdir")?).join("_v2-entry-point");
        entry.is_file().then_some(entry)
    })
}

/// Whether the dynamic loader can find a 32-bit gnutls, from its own cache.
///
/// `None` when that cache cannot be read: no `ldconfig` (NixOS), or one that
/// fails. Not knowing is not a reason to change how setup runs.
pub fn has_32bit_tls() -> Option<bool> {
    ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"].iter().find_map(|ldconfig| {
        let output = Command::new(ldconfig).arg("-p").stdin(Stdio::null()).output().ok()?;
        output
            .status
            .success()
            .then(|| has_32bit(&String::from_utf8_lossy(&output.stdout), "libgnutls.so.30"))
    })
}

/// Where to run Battle.net, its installer and its client alike: `None`
/// straight on the host, as the games run, or the Steam Linux Runtime entry
/// point to run it through when the host has no 32-bit gnutls.
pub fn tls_container(
    proton_root: &Path,
    steam: Option<&crate::steam::SteamRoot>,
) -> Result<Option<PathBuf>, RuntimeError> {
    if has_32bit_tls() != Some(false) {
        return Ok(None);
    }
    let appid = required_runtime(proton_root);
    let found = steam
        .zip(appid)
        .and_then(|(steam, appid)| find_steam_runtime(&steam.library_paths(), appid));
    match found {
        Some(entry) => Ok(Some(entry)),
        None => Err(RuntimeError::NoTls { install: appid.filter(|_| steam.is_some()) }),
    }
}

/// Whether `ldconfig -p` lists a 32-bit `lib`. A 64-bit entry is tagged
/// `(libc6,x86-64)`, a 32-bit one plain `(libc6)`.
fn has_32bit(ldconfig: &str, lib: &str) -> bool {
    ldconfig.lines().any(|line| {
        line.trim_start()
            .strip_prefix(lib)
            .is_some_and(|rest| rest.trim_start().starts_with("(libc6)"))
    })
}

/// One quoted `"key" "value"` pair from a Valve KeyValues file, by key.
fn vdf_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
        match (parts.next(), parts.next()) {
            (Some(k), Some(value)) if k == key => Some(value),
            _ => None,
        }
    })
}

#[cfg(test)]
mod container_tests {
    use super::*;

    const LDCONFIG: &str = "\
1234 libs found in cache `/etc/ld.so.cache'
\tlibgnutls.so.30 (libc6,x86-64) => /usr/lib/libgnutls.so.30
\tlibgnutlsxx.so.30 (libc6) => /usr/lib32/libgnutlsxx.so.30
";

    #[test]
    fn only_a_plain_libc6_entry_is_32_bit() {
        // SteamOS: the 64-bit library, and a 32-bit one whose name merely
        // starts the same way.
        assert!(!has_32bit(LDCONFIG, "libgnutls.so.30"));
        let arch = format!("{LDCONFIG}\tlibgnutls.so.30 (libc6) => /usr/lib32/libgnutls.so.30\n");
        assert!(has_32bit(&arch, "libgnutls.so.30"));
    }

    #[test]
    fn the_runtime_comes_from_the_tool_manifest() {
        let dir = std::env::temp_dir().join(format!("lightforge-toolmanifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("toolmanifest.vdf"),
            "\"manifest\"\n{\n  \"version\" \"2\"\n  \"commandline\" \"/proton %verb%\"\n  \
             \"require_tool_appid\" \"4183110\"\n  \"use_sessions\" \"1\"\n}\n",
        )
        .unwrap();
        assert_eq!(required_runtime(&dir), Some(4183110));
        assert_eq!(required_runtime(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_runtime_is_found_by_its_manifest_in_any_library() {
        let root = std::env::temp_dir().join(format!("lightforge-steam-runtime-{}", std::process::id()));
        let (first, second) = (root.join("one"), root.join("two"));
        let steamapps = second.join("steamapps");
        std::fs::create_dir_all(first.join("steamapps")).unwrap();
        std::fs::create_dir_all(steamapps.join("common/SteamLinuxRuntime_4")).unwrap();
        std::fs::write(
            steamapps.join("appmanifest_4183110.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"4183110\"\n\t\"installdir\"\t\t\"SteamLinuxRuntime_4\"\n}\n",
        )
        .unwrap();
        let libraries = [first, second];

        // Listed but not downloaded yet: no entry point, no runtime.
        assert_eq!(find_steam_runtime(&libraries, 4183110), None);

        let entry = steamapps.join("common/SteamLinuxRuntime_4/_v2-entry-point");
        std::fs::write(&entry, "").unwrap();
        assert_eq!(find_steam_runtime(&libraries, 4183110), Some(entry));
        assert_eq!(find_steam_runtime(&libraries, 1628350), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn without_steam_the_message_names_the_32_bit_wine() {
        let rendered = RuntimeError::NoTls { install: None }.to_string();
        assert!(rendered.contains("gnutls") && rendered.contains(ADVICE), "{rendered}");
        let rendered = RuntimeError::NoTls { install: Some(4183110) }.to_string();
        assert!(rendered.contains("steam://install/4183110"), "{rendered}");
    }
}
