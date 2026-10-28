//! Whether this machine can run 32-bit Windows binaries at all.
//!
//! `Battle.net-Setup.exe` is the first 32-bit binary in setup. Without a
//! 32-bit userspace the prefix builds fine and the installer dies in under a
//! second: stock Debian, until something enables i386 (installing Steam does,
//! via `dpkg --add-architecture i386`).
//!
//! Two checks:
//!
//! 1. [`check_loader`] stats for a 32-bit dynamic loader. No Proton needed,
//!    so it runs before a 400 MB download the user could not have used.
//! 2. [`check_wine`] starts Proton's own 32-bit wine. Catches the case where
//!    i386 is enabled but a library is missing; the loader names that
//!    library, so its stderr is kept, not summarised.
//!
//! Both are only a floor: the 32-bit graphics stack is dlopened and invisible
//! to them. On Fedora 44 `glibc.i686` + `freetype.i686` passes both and the
//! installer still exits 1. So [`remedy_for`] names the 32-bit *Wine*
//! package, whose dependencies are the closure needed. Past the floor,
//! [`crate::battlenet::BattleNetError::InstallerExited`] carries stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

/// Where a 32-bit ELF interpreter lives, across the layouts in use.
///
/// Debian and Ubuntu: `/lib/ld-linux.so.2`; Arch and Fedora: `lib32`. Any one
/// is enough.
const LOADER_CANDIDATES: &[&str] = &[
    "/lib/ld-linux.so.2",
    "/lib32/ld-linux.so.2",
    "/usr/lib/ld-linux.so.2",
    "/usr/lib32/ld-linux.so.2",
];

/// What to tell someone whose 32-bit runtime is missing and whose
/// distribution we do not recognise.
///
/// Names what is required, not a package: `dnf install wine` on Fedora 44
/// installs nothing 32-bit.
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
/// Cheap enough to call before any download. Passes off x86-64, where it
/// means nothing.
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
/// `dist` is Proton's `files`/`dist`. Runs `wine --version`, which touches no
/// prefix; the loader's complaint names the missing library and is kept
/// verbatim. A build without a 32-bit wine passes.
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
/// panel that wants to state a fact, not fail.
pub fn has_32bit_loader() -> bool {
    check_loader().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_advice_names_no_package_manager() {
        // For unknown distributions: libraries, never a package manager.
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
/// Text to copy, never run: root on a guess is a trust escalation, and the
/// systems needing help most (SteamOS, rpm-ostree, NixOS) could not use a
/// button. [`Remedy::distro`] shows what was guessed.
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
    /// Booted from an OSTree image (Silverblue, Bazzite...), which
    /// `/etc/os-release` reports as plain `fedora`.
    pub atomic: bool,
}

impl OsRelease {
    /// The running system's, or the file `LIGHTFORGE_OS_RELEASE` names (for
    /// development; `atomic` still reads the real boot).
    pub fn read() -> Self {
        let path = std::env::var_os("LIGHTFORGE_OS_RELEASE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/etc/os-release".into());
        Self {
            // OSTree writes it at boot; rpm-ostree checks it too.
            atomic: std::path::Path::new("/run/ostree-booted").exists(),
            ..Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
        }
    }

    /// Valve's own SteamOS, on a Deck, a Steam Machine, or a desktop.
    ///
    /// The ID alone, not `ID_LIKE`.
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

    /// What the system calls itself, for "on <name>: run this".
    fn name(&self) -> String {
        match (self.pretty_name.is_empty(), self.id.is_empty()) {
            (false, _) => self.pretty_name.clone(),
            (true, false) => self.id.clone(),
            (true, true) => "this system".to_owned(),
        }
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

/// The table: only the common families; the rest get [`ADVICE`] and no command.
pub fn remedy_for(os: &OsRelease) -> Remedy {
    let distro = os.name();

    let (command, note) = if os.id == "nixos" {
        // Declarative: no command to copy.
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
        // UNVERIFIED. A read-only image: layer the package. `--apply-live`
        // avoids a reboot mid-setup; some images refuse it (see the note).
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
        // Another image-based system: no command we can name with confidence.
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
        // UNVERIFIED. No i386 packages until the architecture is enabled;
        // `wine32` pulls the 32-bit stack.
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
        // VERIFIED on Fedora 44. `wine` alone is x86_64 only; glibc.i686 +
        // freetype.i686 passes our checks but not the installer. `wine.i686`
        // pulls the whole closure.
        (Some("sudo dnf install wine.i686".to_owned()), None)
    } else if os.is("suse") || os.is("opensuse") {
        // UNVERIFIED.
        (Some("sudo zypper install wine-32bit".to_owned()), None)
    } else {
        (None, None)
    };

    Remedy { distro, command, note }
}

/// What to run for the NVIDIA driver's 32-bit libraries, which Battle.net
/// needs and which the driver packages leave out. `driver` is the loaded
/// module's version; Ubuntu names its packages by the major.
pub fn nvidia_remedy(driver: &str) -> Remedy {
    nvidia_remedy_for(&OsRelease::read(), driver)
}

pub fn nvidia_remedy_for(os: &OsRelease, driver: &str) -> Remedy {
    let major = driver.split('.').next().unwrap_or(driver);
    let apt = |package: &str| {
        format!("sudo dpkg --add-architecture i386 && sudo apt update && sudo apt install {package}:i386")
    };

    // All UNVERIFIED: no NVIDIA machine has run them yet.
    let (command, note) = if os.id == "nixos" {
        (None, Some("set `hardware.graphics.enable32Bit = true;` and rebuild".to_owned()))
    } else if os.atomic {
        (
            None,
            Some("this system boots from a read-only image; use an image built with the NVIDIA driver".to_owned()),
        )
    } else if os.is("ubuntu") {
        (Some(apt(&format!("libnvidia-gl-{major}"))), None)
    } else if os.is("debian") {
        (Some(apt("nvidia-driver-libs")), None)
    } else if os.is("arch") {
        (
            Some("sudo pacman -S lib32-nvidia-utils".to_owned()),
            Some("it lives in the multilib repository, enabled in /etc/pacman.conf".to_owned()),
        )
    } else if os.is_fedora() {
        (
            Some("sudo dnf install xorg-x11-drv-nvidia-libs.i686".to_owned()),
            Some("from RPM Fusion, where the driver itself comes from".to_owned()),
        )
    } else {
        (None, None)
    };

    Remedy { distro: os.name(), command, note }
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

    /// Fedora 44's `dnf install wine` installs nothing 32-bit: the arch is named.
    #[test]
    fn fedora_names_the_i686_arch_explicitly() {
        let command = remedy_for(&os("fedora", "")).command.unwrap();
        assert!(command.contains("i686"), "must name the 32-bit arch: {command}");
        assert!(
            !command.ends_with("install wine"),
            "bare `install wine` resolves to x86_64 on Fedora and fixes nothing: {command}"
        );
    }

    /// Every command asks for a 32-bit *Wine*, not a list of libraries: those
    /// pass our checks and still fail the installer.
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
            // Arch's `wine` is multilib: no suffix, and the note names the repository.
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
    fn ubuntu_names_the_gl_package_by_the_driver_major() {
        let command = nvidia_remedy_for(&os("linuxmint", "ubuntu debian"), "580.82.07").command.unwrap();
        assert!(command.ends_with("apt install libnvidia-gl-580:i386"), "{command}");
        assert!(command.starts_with("sudo dpkg --add-architecture i386"), "{command}");
    }

    #[test]
    fn every_nvidia_command_asks_for_a_32_bit_build() {
        for (id, like) in [("debian", ""), ("ubuntu", "debian"), ("cachyos", "arch"), ("fedora", "")] {
            let command = nvidia_remedy_for(&os(id, like), "580.82.07").command.unwrap();
            assert!(
                ["i386", "i686", "lib32"].iter().any(|m| command.contains(m)),
                "{id} names no 32-bit build: {command}"
            );
        }
    }

    #[test]
    fn an_image_based_system_gets_no_nvidia_command() {
        assert!(nvidia_remedy_for(&atomic("bazzite", "fedora"), "580.82.07").command.is_none());
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
    /// and must get Debian's answer instead of falling through to nothing.
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
// Wine does HTTPS through a dlopened host gnutls. SteamOS has no 32-bit one:
// the installer fails in a second (`BLZBNTBTS00000028`, "no connection"), and
// the client draws its login (CEF's own TLS) but never signs in. Steam avoids
// it by running Proton in the Steam Linux Runtime, so Battle.net goes there
// when needed. The games are 64-bit and stay on the host.

/// The runtime a Proton build asks Steam to run it in, from its
/// `toolmanifest.vdf`: `4183110` (Steam Linux Runtime 4.0) for GE-Proton 11.
pub fn required_runtime(proton_root: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(proton_root.join("toolmanifest.vdf")).ok()?;
    vdf_value(&text, "require_tool_appid")?.parse().ok()
}

/// The runtime's entry point, if Steam has `appid` installed in any of
/// `libraries`. Found through the app manifest's `installdir`, not a
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
    has_32bit_lib("libgnutls.so.30")
}

/// Whether the dynamic loader's cache has a 32-bit `lib`; `None` when that
/// cache cannot be read.
pub fn has_32bit_lib(lib: &str) -> Option<bool> {
    ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"].iter().find_map(|ldconfig| {
        let output = Command::new(ldconfig).arg("-p").stdin(Stdio::null()).output().ok()?;
        output.status.success().then(|| has_32bit(&String::from_utf8_lossy(&output.stdout), lib))
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
