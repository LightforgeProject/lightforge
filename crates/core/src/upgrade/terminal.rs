//! The package managers' route: their own command, in a terminal.
//!
//! Lightforge fetches and checks what will be installed, writes a short
//! script beside it and opens a terminal on it. The package manager asks for
//! the password itself, where the user can see exactly what it runs, and the
//! script waits for Enter at the end so its last lines stay readable.
//!
//! Linux has no one way to say "open a terminal and run this". The newer
//! `xdg-terminal-exec` is asked first, then `$TERMINAL`, then the terminals
//! the common desktops ship, each with its own way of being handed a command.
//! None of them is a requirement: without one, the dialog shows the command.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{fetch_asset, fetch_pkgbuild, Package, UpgradeError};
use crate::net::Http;

/// Terminals the common desktops ship, and what each needs before the
/// command it runs.
const TERMINALS: &[(&str, &[&str])] = &[
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("ptyxis", &["--"]),
    ("kgx", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("mate-terminal", &["-x"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("foot", &[]),
    ("wezterm", &["start", "--"]),
    ("ghostty", &["-e"]),
    ("xterm", &["-e"]),
];

/// The script's name inside the work folder.
const SCRIPT: &str = "update.sh";

/// Fetch what `package` needs for `version` into `work`, emptied first, and
/// write the script that installs it. Returns the script, and the command it
/// runs, for the dialog to show when no terminal can be opened.
pub async fn prepare(
    http: &Http,
    package: &Package,
    version: &str,
    work: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(PathBuf, String), UpgradeError> {
    let io = |source| UpgradeError::Io { path: work.to_path_buf(), source };
    // Whatever the last update left: nothing reads it once its terminal is
    // closed, and Lightforge cannot tell when that was.
    match std::fs::remove_dir_all(work) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(e)),
        _ => {}
    }
    std::fs::create_dir_all(work).map_err(io)?;

    let command = match package {
        // `makepkg` builds in the folder it is run from and asks for the
        // password only to install, through pacman.
        Package::Pacman { package } => {
            let pkgbuild = fetch_pkgbuild(http, package, version).await?;
            let path = work.join("PKGBUILD");
            std::fs::write(&path, pkgbuild).map_err(|source| UpgradeError::Io { path, source })?;
            "makepkg -si --noconfirm".to_owned()
        }
        Package::Deb => {
            let deb = fetch_asset(http, version, "_amd64.deb", work, on_progress).await?;
            format!("sudo apt install {}", quote(&deb))
        }
        Package::Rpm => {
            let rpm = fetch_asset(http, version, ".x86_64.rpm", work, on_progress).await?;
            format!("sudo dnf install {}", quote(&rpm))
        }
    };

    let script = work.join(SCRIPT);
    std::fs::write(&script, script_text(work, version, &command))
        .map_err(|source| UpgradeError::Io { path: script.clone(), source })?;
    Ok((script, command))
}

/// Open a terminal on `script`. Returns as soon as it is started: some
/// terminals hand the window to a server and exit at once, so waiting tells
/// nothing about the update.
pub fn open(script: &Path) -> Result<(), UpgradeError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let chosen = std::env::var_os("TERMINAL");
    let mut argv = terminal(&path, chosen.as_deref()).ok_or(UpgradeError::NoTerminal)?;
    argv.extend(["sh".into(), script.as_os_str().to_owned()]);

    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(script.parent().unwrap_or(Path::new("/")))
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|source| UpgradeError::Io { path: PathBuf::from(&argv[0]), source })?;
    // Reaped on the side, so a terminal that stays in the foreground does
    // not linger as a zombie once it closes.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// The terminal to use and the arguments before the command, or `None`.
fn terminal(path: &std::ffi::OsStr, chosen: Option<&std::ffi::OsStr>) -> Option<Vec<OsString>> {
    let found = |name: &str| std::env::split_paths(path).map(|dir| dir.join(name)).find(|p| p.is_file());
    if let Some(exec) = found("xdg-terminal-exec") {
        return Some(vec![exec.into()]);
    }
    // `$TERMINAL -e` is the long-standing convention for a terminal someone
    // picked themselves.
    if let Some(chosen) = chosen.filter(|c| !c.is_empty()) {
        let program = match Path::new(chosen).is_absolute() {
            true => Some(PathBuf::from(chosen)),
            false => found(&chosen.to_string_lossy()),
        };
        if let Some(program) = program.filter(|p| p.is_file()) {
            return Some(vec![program.into(), "-e".into()]);
        }
    }
    TERMINALS.iter().find_map(|(name, before)| {
        let program = found(name)?;
        Some(std::iter::once(program.into()).chain(before.iter().map(OsString::from)).collect())
    })
}

/// The script: run `command`, say how it went, and wait for Enter.
fn script_text(work: &Path, version: &str, command: &str) -> String {
    format!(
        "#!/bin/sh\n\
         cd {work} || exit 1\n\
         printf '\\n==> Updating Lightforge to {version}\\n\\n'\n\
         if {command}; then\n    \
             printf '\\nDone. Close this window and restart Lightforge to use {version}.\\n'\n\
         else\n    \
             printf '\\nThe update did not finish; the lines above say why.\\n'\n\
         fi\n\
         printf 'Press Enter to close this window. '\n\
         read -r _\n",
        work = quote(work),
    )
}

/// `path` as one single-quoted shell word.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin_dir(name: &str, programs: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-terminals-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for program in programs {
            std::fs::write(dir.join(program), "").unwrap();
        }
        dir
    }

    #[test]
    fn the_standard_launcher_wins_then_the_users_pick_then_the_list() {
        let all = bin_dir("all", &["xdg-terminal-exec", "konsole", "kitty"]);
        let argv = terminal(all.as_os_str(), Some("kitty".as_ref())).unwrap();
        assert_eq!(argv, [OsString::from(all.join("xdg-terminal-exec"))]);

        let picked = bin_dir("picked", &["konsole", "kitty"]);
        let argv = terminal(picked.as_os_str(), Some("kitty".as_ref())).unwrap();
        assert_eq!(argv, [OsString::from(picked.join("kitty")), "-e".into()]);

        // A $TERMINAL that is not there is no reason to give up.
        let argv = terminal(picked.as_os_str(), Some("nonexistent".as_ref())).unwrap();
        assert_eq!(argv, [OsString::from(picked.join("konsole")), "-e".into()]);

        let gnome = bin_dir("gnome", &["gnome-terminal"]);
        let argv = terminal(gnome.as_os_str(), None).unwrap();
        assert_eq!(argv, [OsString::from(gnome.join("gnome-terminal")), "--".into()]);

        let none = bin_dir("none", &[]);
        assert_eq!(terminal(none.as_os_str(), None), None);
        for dir in [all, picked, gnome, none] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn the_script_waits_for_enter_whatever_happened() {
        let script = script_text(Path::new("/home/u/.cache/lightforge/upgrade"), "1.0.0-beta.7", "makepkg -si --noconfirm");
        assert!(script.contains("cd '/home/u/.cache/lightforge/upgrade' || exit 1"), "{script}");
        assert!(script.contains("if makepkg -si --noconfirm; then"), "{script}");
        // After the `fi`, so a failure waits as well.
        let (_, after) = script.split_once("fi\n").unwrap();
        assert!(after.contains("read -r _"), "{script}");
    }

    #[test]
    fn a_path_is_one_shell_word_whatever_it_contains() {
        assert_eq!(quote(Path::new("/tmp/it's here.deb")), r"'/tmp/it'\''s here.deb'");
    }
}
