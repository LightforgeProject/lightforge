//! A game's launch options, written the way Steam takes them: a command line
//! with `%command%` where the game goes, such as `~/bin/wow.sh %command%`.
//!
//! A game Lightforge starts itself is wrapped for real, outside every
//! wrapper of its own. A game Battle.net starts cannot be: the launch is a
//! message to the running client, and the game appears in the prefix with
//! nothing on the host around it. There the line runs once the game is up,
//! `%command%` standing for a command that waits for it to exit, so a script
//! that starts helpers, runs `"$@"` and cleans up after works for both.
//!
//! **Nothing here touches the game**: these are host programs around it.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Child, Command, Stdio};

use crate::preflight::GameProcess;
use crate::product::ProductCode;

/// Where the game's command goes, spelled as on Steam.
pub const PLACEHOLDER: &str = "%command%";

/// What the shell is told it is called, so its errors say who ran them:
/// `lightforge: 1: wow.sh: not found`.
const SHELL_NAME: &str = "lightforge";

/// The most of a failed line's stderr that is logged: enough for `command
/// not found` or a unit's error, not a flood.
const STDERR_KEPT: u64 = 4096;

/// The script for `sh -c`: the line with `%command%` as `"$@"`, so the
/// game's command arrives as arguments and the shell never parses it.
///
/// `None` when there is nowhere to put the game. Steam appends such a line
/// to the game's arguments, which a game Battle.net starts cannot be given.
pub fn script(line: &str) -> Option<String> {
    let line = line.trim();
    line.contains(PLACEHOLDER).then(|| line.replace(PLACEHOLDER, "\"$@\""))
}

/// Rebuild `cmd`, a game Lightforge starts itself, to run inside the launch
/// options. Unchanged when [`script`] has nothing.
pub fn wrap(cmd: Command, line: &str) -> Command {
    let Some(script) = script(line) else {
        return cmd;
    };
    let shell = [OsStr::new("sh"), OsStr::new("-c"), OsStr::new(&script), OsStr::new(SHELL_NAME)];
    crate::tweaks::wrapped(cmd, &shell)
}

/// Start the launch options beside `game`, which Battle.net started,
/// without waiting for them. `true` when they were started.
///
/// `%command%` is `tail --pid`, which returns once the game is gone: the game
/// is no child of the shell, so `wait` cannot see it. Their exit status, and
/// what they said on stderr when they failed, go to the log.
pub fn run_beside(line: &str, product: &ProductCode, game: &GameProcess) -> bool {
    let Some(script) = script(line) else {
        return false;
    };
    tracing::info!("running the launch options for {} beside pid {}: {}", product.as_str(), game.pid, line.trim());
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&script)
        .arg(SHELL_NAME)
        .args(["tail", &format!("--pid={}", game.pid), "-f", "/dev/null"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Its own process group: a script's cleanup after `%command%` should
    // still run when the game exits after a Ctrl+C to the launcher in its
    // terminal.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(child) => reap(child),
        Err(e) => {
            tracing::warn!("could not start the launch options for {}: {e}", product.as_str());
            return false;
        }
    }
    true
}

/// Wait for the launch options off-thread and log how they ended.
///
/// stderr is read on a thread of its own: a script that starts a daemon
/// with `&` hands it the pipe, and waiting for that to close would leave
/// the shell a zombie for as long as the daemon runs.
fn reap(mut child: Child) {
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = pipe.by_ref().take(STDERR_KEPT).read_to_end(&mut said);
            // Drained to the end, not dropped: the next write to a closed
            // pipe is a SIGPIPE, which ended a chatty program in the line
            // (a Qt app's warnings) partway through the game.
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            String::from_utf8_lossy(&said).trim().to_owned()
        })
    });
    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => tracing::info!("the launch options finished"),
        Ok(status) => {
            let said = stderr.and_then(|reader| reader.join().ok()).unwrap_or_default();
            match said.is_empty() {
                true => tracing::warn!("the launch options failed ({status})"),
                false => tracing::warn!("the launch options failed ({status}): {said}"),
            }
        }
        Err(e) => tracing::warn!("could not wait for the launch options: {e}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A game that has already exited, so `%command%` returns at once.
    fn gone() -> GameProcess {
        let mut child = Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        GameProcess { pid: child.id(), name: "Wow.exe".to_owned() }
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(done(), "{what}");
    }

    #[test]
    fn a_line_without_the_placeholder_runs_nothing() {
        assert_eq!(script("   "), None);
        assert_eq!(script("gamemoderun"), None, "nowhere to put the game");
        assert!(!run_beside("gamemoderun", &ProductCode::Retail, &gone()));
        assert_eq!(wrap(Command::new("/proton/proton"), "gamemoderun").get_program(), "/proton/proton");
    }

    // The shell never reads the game's command as text, so nothing in it is
    // expanded, split or taken for the placeholder.
    #[test]
    fn the_game_command_arrives_untouched() {
        let mut game = Command::new("printf");
        game.args(["%s|", "two words", "$HOME", "it's", "%command%"]);
        let out = wrap(game, "env LIGHTFORGE_TEST=1 %command% --last").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "two words|$HOME|it's|%command%|--last|");
    }

    // Proton finds the prefix through the environment, and a launch may set
    // the folder it runs in: both have to reach the game through the shell.
    #[test]
    fn the_wrapped_game_keeps_its_environment_and_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut game = Command::new("sh");
        game.arg("-c").arg("printf '%s|%s' \"$STEAM_COMPAT_DATA_PATH\" \"$(pwd)\"");
        game.env("STEAM_COMPAT_DATA_PATH", "/compat").current_dir(dir.path());
        let out = wrap(game, "%command%").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("/compat|{}", dir.path().display()));
    }

    // The shape of a Steam user's script: helpers up, the game, helpers down.
    #[test]
    fn a_script_runs_around_a_wrapped_game() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let mut game = Command::new("sh");
        game.arg("-c").arg(format!("echo game >> '{}'", log.display()));
        let line = format!("echo up >> '{0}'; %command%; echo down >> '{0}'", log.display());
        assert!(wrap(game, &line).status().unwrap().success());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "up\ngame\ndown\n");
    }

    // Beside a game Battle.net started, `%command%` lasts as long as the
    // game, and the caller does not wait for any of it.
    #[test]
    fn beside_a_running_game_the_command_lasts_until_it_exits() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let read = || std::fs::read_to_string(&log).unwrap_or_default();
        let mut stand_in = Command::new("sleep").arg("30").spawn().unwrap();
        let game = GameProcess { pid: stand_in.id(), name: "Wow.exe".to_owned() };

        let line = format!("echo up >> '{0}'; %command%; echo down >> '{0}'", log.display());
        assert!(run_beside(&line, &ProductCode::Retail, &game));
        wait_until("the line started", || read() == "up\n");
        // Longer than `tail --pid` takes to notice a game that is gone.
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(read(), "up\n", "nothing after %command% while the game runs");

        stand_in.kill().unwrap();
        // Reaped, as Wine reaps the real game: `tail` sees a zombie as alive.
        stand_in.wait().unwrap();
        wait_until("the cleanup ran after the game exited", || read() == "up\ndown\n");
    }

    // A program started in the line runs for the whole session, and may say
    // far more on stderr than the log keeps. The shell writes 220 KB itself,
    // past the pipe's 64 KiB buffer, and only reaches the file if no write
    // was cut off.
    #[test]
    fn a_line_that_says_more_than_the_log_keeps_is_not_cut_off() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("done");
        let line = format!(
            "i=0; while [ $i -lt 20000 ]; do printf 'xxxxxxxxxx\\n' >&2; i=$((i+1)); done; echo done > '{}'; %command%",
            out.display()
        );
        assert!(run_beside(&line, &ProductCode::Retail, &gone()));
        wait_until("the line was killed before it finished", || out.exists());
    }

    #[test]
    fn a_profile_without_launch_options_saves_as_it_was() {
        let old: crate::product::ProductProfile = toml::from_str("proton_ge_version = \"GE-Proton10-1\"").unwrap();
        assert!(old.launch_options.is_empty());
        let saved = toml::to_string(&old).unwrap();
        assert!(!saved.contains("launch_options"), "{saved}");
    }
}
