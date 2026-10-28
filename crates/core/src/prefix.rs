//! Prefix lifecycle: creation via Proton, and the POSIX symlink that puts the
//! game inside a Windows-shaped path.
//!
//! Everything here runs Proton the way Steam does, the `proton` entry-point
//! script with `STEAM_COMPAT_DATA_PATH` and `STEAM_COMPAT_CLIENT_INSTALL_PATH`
//! in the environment, rather than reaching for a system `wine` binary. That
//! keeps the prefix byte-identical to one Steam would have produced, which is
//! what makes an imported install and a Lightforge-created one interchangeable.
//!
//! Proton owns the directory naming: it always creates the Wine prefix at
//! `$STEAM_COMPAT_DATA_PATH/pfx`. [`crate::paths::Layout::compat_data`] is
//! therefore the value handed to Proton, and [`crate::paths::Layout::prefix`]
//! is the `pfx` directory inside it.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::paths::Layout;
use crate::proton::ProtonRelease;

/// Files Proton unpacks its runtime into, newest layout first. GE ships
/// `files/`; older builds and some forks ship `dist/`.
const DIST_DIRS: &[&str] = &["files", "dist"];

/// Assembles and runs Proton against Lightforge's shared prefix.
///
/// Cheap to construct and free of side effects until [`ProtonRunner::run`] is
/// called, so [`ProtonRunner::command`] can be asserted against in tests
/// without Wine anywhere in the picture.
#[derive(Debug, Clone)]
pub struct ProtonRunner {
    entry_point: PathBuf,
    dist: PathBuf,
    compat_data: PathBuf,
    steam_client: PathBuf,
    /// Made visible to the prefix, since the game lives outside it.
    compat_mounts: Vec<PathBuf>,
    shader_cache: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    /// A Steam Linux Runtime entry point to run Proton inside, as Steam
    /// does. Only for Battle.net, see [`crate::runtime::tls_container`].
    container: Option<PathBuf>,
    /// The prefix already has a session this command joins, see
    /// [`ProtonRunner::joining_session`].
    joins_session: bool,
}

impl ProtonRunner {
    /// Run `release` against the prefix described by `layout`.
    ///
    /// `steam_client` is what Proton is told to treat as the Steam client
    /// install, the user's real Steam root when they have one, otherwise a
    /// Lightforge-owned stub directory (see
    /// [`ProtonRunner::steam_client_path`]). Proton only consults it for
    /// `steamclient.dll` plumbing that Lightforge never exercises, but the
    /// variable being unset makes some builds bail out early.
    pub fn new(release: &ProtonRelease, layout: &Layout, steam_client: impl Into<PathBuf>) -> Self {
        Self {
            entry_point: release.entry_point(),
            dist: dist_dir(&release.path),
            compat_data: layout.compat_data(),
            steam_client: steam_client.into(),
            compat_mounts: games_mounts(layout),
            shader_cache: None,
            env: Vec::new(),
            container: None,
            joins_session: false,
        }
    }

    /// The Steam client path to hand Proton: the real Steam root if Steam is
    /// installed, else a Lightforge-owned stub under its data directory.
    ///
    /// Proton is not a Steam dependency for Lightforge's purposes, so a missing
    /// Steam install must not be fatal, but the variable still has to point
    /// somewhere that exists.
    pub fn steam_client_path(
        steam: Option<&crate::steam::SteamRoot>,
        dirs: &crate::paths::AppDirs,
    ) -> PathBuf {
        match steam {
            Some(s) => s.path().to_path_buf(),
            None => dirs.data_dir().join("steam-compat"),
        }
    }

    /// Keep one product's shader caches in `dir`: DXVK's and VKD3D's in it,
    /// the NVIDIA and Mesa drivers' in `nvidia` and `mesa` under it.
    ///
    /// Left alone, DXVK writes to `%LOCALAPPDATA%\dxvk` inside the prefix and
    /// the drivers into `~/.cache`, pooled with every other program. Here all
    /// of them sit beside Lightforge's other caches, where Storage can size
    /// and clear them per product. The directories are made here rather than
    /// trusting each of those to create its own; failing only costs the
    /// cache, so the launch goes on either way.
    pub fn with_shader_cache(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        for driver in [NVIDIA_CACHE, MESA_CACHE] {
            let _ = std::fs::create_dir_all(dir.join(driver));
        }
        self.shader_cache = Some(dir);
        self
    }

    /// Add a path Proton should keep visible to the prefix.
    pub fn with_compat_mount(mut self, path: impl Into<PathBuf>) -> Self {
        self.compat_mounts.push(path.into());
        self
    }

    /// Set an extra environment variable, overriding any assembled default.
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Run Proton inside the Steam Linux Runtime whose `_v2-entry-point` this
    /// is. Its libraries replace the host's, and it sees only what it is
    /// told to: the prefix, the mounts, the Proton build, and the program's
    /// own directory.
    pub fn in_container(mut self, entry_point: impl Into<PathBuf>) -> Self {
        self.container = Some(entry_point.into());
        self
    }

    /// Whether commands run inside a container.
    pub fn has_container(&self) -> bool {
        self.container.is_some()
    }

    /// Run alongside a session the prefix already has, rather than after it.
    ///
    /// In a container a plain run becomes `waitforexitandrun`, which waits
    /// for the prefix's wineserver to exit first. Right for the call that
    /// becomes the client, and a hang for one that only hands a running
    /// client its arguments: measured, it waited until the client was closed.
    pub fn joining_session(mut self) -> Self {
        self.joins_session = true;
        self
    }

    /// The Proton build this runs, the directory holding `proton` and its
    /// `toolmanifest.vdf`.
    pub fn build(&self) -> &Path {
        self.entry_point.parent().unwrap_or(&self.entry_point)
    }

    /// The Wine prefix Proton will create or reuse: `<compat data>/pfx`.
    pub fn prefix(&self) -> PathBuf {
        self.compat_data.join("pfx")
    }

    /// Build the [`Command`] for `proton <verb> <program> [args…]` without
    /// running it.
    pub fn command<I, S>(&self, verb: &str, program: impl AsRef<OsStr>, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let program = program.as_ref();
        let mut mounts = self.compat_mounts.clone();
        let verb = match self.container {
            Some(_) if verb == "run" && !self.joins_session => "waitforexitandrun",
            _ => verb,
        };
        let mut cmd = match &self.container {
            None => Command::new(&self.entry_point),
            Some(container) => {
                // Steam's own pairing: the container waits for the session,
                // and Proton inside it for the prefix, both under one verb.
                let mut cmd = Command::new(container);
                cmd.arg(format!("--verb={verb}")).arg("--").arg(&self.entry_point);
                cmd.env("STEAM_COMPAT_TOOL_PATHS", self.build());
                mounts.extend(Path::new(program).parent().map(Path::to_path_buf));
                cmd
            }
        };
        cmd.arg(verb).arg(program).args(args);

        cmd.env("STEAM_COMPAT_DATA_PATH", &self.compat_data);
        cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &self.steam_client);
        if !mounts.is_empty() {
            cmd.env("STEAM_COMPAT_MOUNTS", join_paths(&mounts));
        }
        // Proton keys parts of its runtime state off the app id. Lightforge
        // isn't a Steam app, so pin a stable placeholder instead of letting
        // each build invent its own.
        cmd.env("SteamAppId", "0");
        // Never a game id, not even an empty one: under GE-Proton11-9 the
        // variable being set at all hangs `wineboot -u` for good (measured,
        // 11-7 is fine). Removed rather than left unset, because Steam puts
        // its own into this process when it starts the launcher.
        cmd.env_remove("SteamGameId");
        // Pointers at somebody else's Wine. Proton appends to these rather
        // than replacing them, so a stray `WINEDLLPATH` in the user's shell
        // makes the build load another Wine's DLLs against its own unix side
        // (`unimplemented function win32u...`, aborting). Nothing a user sets
        // for their own wine means anything to a Proton build. The tweaks in
        // `self.env` below are applied after this and still win.
        for var in FOREIGN_WINE_VARS.iter().chain(&FOREIGN_PYTHON_VARS) {
            cmd.env_remove(var);
        }
        // The names the DXVK (3.1) and VKD3D-Proton in GE-Proton11-7 read.
        // The older `DXVK_STATE_CACHE_PATH` went with DXVK's state cache and
        // is not read by that build at all.
        if let Some(cache) = &self.shader_cache {
            cmd.env("DXVK_SHADER_CACHE_PATH", cache);
            cmd.env("VKD3D_SHADER_CACHE_PATH", cache);
            cmd.env("__GL_SHADER_DISK_CACHE_PATH", cache.join(NVIDIA_CACHE));
            // The NVIDIA driver otherwise trims its cache to a size cap, and
            // a trimmed pipeline is a stutter the next time it is drawn. The
            // per-product Clear button is the limit instead.
            cmd.env("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1");
            cmd.env("MESA_SHADER_CACHE_DIR", cache.join(MESA_CACHE));
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }

    /// Proton's runtime directory, `files` or `dist`, whichever this build
    /// ships. The wine binaries and `wineserver` live under its `bin`.
    pub fn dist(&self) -> &Path {
        &self.dist
    }

    /// Run a program in the prefix and wait for it, failing on a non-zero exit.
    pub fn run<I, S>(&self, program: impl AsRef<OsStr>, args: I) -> Result<(), PrefixError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.prepare()?;
        let program = program.as_ref().to_os_string();
        let mut command = self.command("run", &program, args);
        command.stdin(Stdio::null());
        tracing::info!("running in the prefix: {command:?}");
        let started = std::time::Instant::now();
        let output = command
            .output()
            .map_err(|e| PrefixError::Spawn { program: self.entry_point.clone(), source: e })?;

        if output.status.success() {
            tracing::info!(
                "{} finished in {:.1}s",
                program.to_string_lossy(),
                started.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        let stderr = tail(&output.stderr);
        tracing::error!(
            "{} exited with {:?} after {:.1}s; stderr tail: {}",
            program.to_string_lossy(),
            output.status.code(),
            started.elapsed().as_secs_f64(),
            stderr.trim(),
        );
        Err(PrefixError::Command {
            program: program.to_string_lossy().into_owned(),
            code: output.status.code(),
            stderr,
        })
    }

    /// Create the prefix if it doesn't exist, or update an existing one.
    ///
    /// `wineboot -u` is idempotent: on a fresh compat-data directory Proton
    /// builds the prefix from scratch, and on an existing one it refreshes the
    /// managed DLLs a Proton version change may have invalidated. Returns
    /// whether the prefix had to be created.
    pub fn wineboot_update(&self) -> Result<bool, PrefixError> {
        let created = !is_initialized(&self.prefix());
        self.run("wineboot", ["-u"])?;

        if !is_initialized(&self.prefix()) {
            return Err(PrefixError::NotInitialized(self.prefix()));
        }
        Ok(created)
    }

    /// Shut the prefix's `wineserver` down and wait for it to exit.
    ///
    /// Invoked directly out of Proton's runtime rather than through the
    /// `proton` script, which has no verb for it. A prefix with a live
    /// wineserver holds locks on the registry and on `.build.info`'s directory,
    /// so this is the gate every mutation runs behind.
    pub fn wineserver_kill(&self) -> Result<(), PrefixError> {
        let wineserver = self.dist.join("bin").join("wineserver");
        if !wineserver.is_file() {
            return Err(PrefixError::MissingRuntime(wineserver));
        }

        let output = Command::new(&wineserver)
            .arg("-k")
            // -w waits for the server to actually go away; without it the next
            // command can race a prefix that is still tearing down.
            .arg("-w")
            .env("WINEPREFIX", self.prefix())
            .stdin(Stdio::null())
            .output()
            .map_err(|e| PrefixError::Spawn { program: wineserver.clone(), source: e })?;

        // Still `Ok`: nothing waits on this to proceed safely, the next
        // pre-flight scans the prefix itself. But a refusal is worth a line in
        // a bug report, and is easy to miss as the "no server" exit.
        if !server_gone(output.status, &output.stderr) {
            tracing::warn!(
                "wineserver -k -w exited with {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Create the directories Proton expects to already exist.
    fn prepare(&self) -> Result<(), PrefixError> {
        if !self.entry_point.is_file() {
            return Err(PrefixError::MissingRuntime(self.entry_point.clone()));
        }
        for dir in [&self.compat_data, &self.steam_client] {
            std::fs::create_dir_all(dir)
                .map_err(|e| PrefixError::Io { path: dir.clone(), source: e })?;
        }
        Ok(())
    }
}

/// Variables that point a Wine at another Wine's files, dropped from every
/// Proton run. `WINEPREFIX` and the `WINEDLLOVERRIDES`/`WINEESYNC` family are
/// deliberately not here: Proton sets the first itself and users tune the rest.
const FOREIGN_WINE_VARS: [&str; 3] = ["WINEDLLPATH", "WINELOADER", "WINESERVER"];

/// `proton` is a Python script: a conda or venv shell's interpreter settings
/// make it import another Python's standard library and die before Wine
/// starts. Only these three; the `LD_*` variables stay, overlays and
/// capture tools are injected through them.
const FOREIGN_PYTHON_VARS: [&str; 3] = ["PYTHONHOME", "PYTHONPATH", "PYTHONSTARTUP"];

/// The GL/Vulkan drivers' own caches, inside a product's shader cache.
const NVIDIA_CACHE: &str = "nvidia";
const MESA_CACHE: &str = "mesa";

/// Every library's games folder, the default one first.
///
/// A container sees only what it is shown, and its wineserver serves the
/// whole prefix: a game on another drive, started on the host while the
/// client runs in the container, is still opened through it.
fn games_mounts(layout: &Layout) -> Vec<PathBuf> {
    let mut mounts = vec![layout.games_root()];
    for library in layout.libraries().all() {
        if !mounts.iter().any(|m| m == library.games()) {
            mounts.push(library.games().to_path_buf());
        }
    }
    mounts
}

/// Proton's unpacked runtime directory, which holds `bin/wineserver`.
fn dist_dir(proton_root: &Path) -> PathBuf {
    DIST_DIRS
        .iter()
        .map(|d| proton_root.join(d))
        .find(|p| p.is_dir())
        .unwrap_or_else(|| proton_root.join(DIST_DIRS[0]))
}

/// A prefix is only usable once Wine has written its registry hives.
fn is_initialized(prefix: &Path) -> bool {
    prefix.join("system.reg").is_file() && prefix.join("drive_c").is_dir()
}

fn join_paths(paths: &[PathBuf]) -> OsString {
    let mut out = OsString::new();
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            out.push(":");
        }
        out.push(path);
    }
    out
}

/// Last few lines of a child's stderr, for an error message that fits on screen.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// What the Windows-visible game path currently is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// A symlink pointing at the expected target.
    Correct,
    /// Nothing at the link path.
    Missing,
    /// A symlink pointing at the right place, where the right place is not
    /// there yet: a library with no game downloaded into it, or a `games/`
    /// that was wiped. The link itself is correct and Wine resolves it the
    /// moment the directory appears, so there is nothing to repair.
    Pending(PathBuf),
    /// A symlink pointing somewhere else entirely.
    Mislinked(PathBuf),
    /// A real, empty directory, typically created by an installer that ran
    /// before the link existed. Safe to replace.
    OccupiedEmpty,
    /// A real directory with contents, or a file. Never touched automatically:
    /// this is someone's game install sitting in the disposable prefix.
    Occupied,
}

impl LinkState {
    /// Whether the prefix can be launched from as-is.
    ///
    /// [`Self::Pending`] counts: the link is right, and the directory it names
    /// appears with the first download. Setup is finished either way, and
    /// holding it open until a game is installed would make a first run loop.
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Correct | Self::Pending(_))
    }

}

/// Classify what sits at `link` relative to the intended `target`.
pub fn inspect_link(link: &Path, target: &Path) -> LinkState {
    let Ok(meta) = std::fs::symlink_metadata(link) else {
        return LinkState::Missing;
    };

    if !meta.file_type().is_symlink() {
        let empty = meta.is_dir()
            && std::fs::read_dir(link).map(|mut d| d.next().is_none()).unwrap_or(false);
        return if empty { LinkState::OccupiedEmpty } else { LinkState::Occupied };
    }

    let Ok(dest) = std::fs::read_link(link) else {
        return LinkState::Pending(PathBuf::new());
    };
    // Compare resolved paths where possible so `games/` reached through a
    // symlinked home still counts as correct.
    let same = dest == target
        || matches!(
            (dest.canonicalize(), target.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        );

    match (same, link.exists()) {
        (true, true) => LinkState::Correct,
        (true, false) => LinkState::Pending(dest),
        (false, _) => LinkState::Mislinked(dest),
    }
}

/// Create or repair the symlink, returning the state found beforehand.
///
/// The **target is not created**. A symlink into a not-yet-downloaded install
/// is the normal pre-install state and Wine resolves it the moment the
/// directory appears, so conjuring the directory here buys nothing and costs
/// a real one: an empty `World of Warcraft/` in every library the moment a
/// prefix is built. That folder is indistinguishable from a broken install, it
/// occupies the name an install found elsewhere would be linked or adopted
/// into, and a product whose library holds one reads as `Install` rather than
/// `Play`. The downloader creates the directory it writes into.
pub fn ensure_link(link: &Path, target: &Path) -> Result<LinkState, PrefixError> {
    let before = inspect_link(link, target);

    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PrefixError::Io { path: parent.to_path_buf(), source: e })?;
    }

    match &before {
        LinkState::Correct | LinkState::Pending(_) => return Ok(before),
        LinkState::Occupied => return Err(PrefixError::LinkOccupied(link.to_path_buf())),
        LinkState::Mislinked(_) => std::fs::remove_file(link)
            .map_err(|e| PrefixError::Io { path: link.to_path_buf(), source: e })?,
        LinkState::OccupiedEmpty => std::fs::remove_dir(link)
            .map_err(|e| PrefixError::Io { path: link.to_path_buf(), source: e })?,
        LinkState::Missing => {}
    }

    std::os::unix::fs::symlink(target, link)
        .map_err(|e| PrefixError::Io { path: link.to_path_buf(), source: e })?;
    Ok(before)
}

/// A read-only snapshot of the prefix, for the UI and `lightforge prefix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixStatus {
    pub compat_data: PathBuf,
    pub prefix: PathBuf,
    /// Wine has written its registry hives, the prefix is usable.
    pub initialized: bool,
    /// The Proton build that last created or upgraded this prefix, as Proton
    /// itself recorded it. `None` before the prefix exists.
    ///
    /// Worth carrying next to `initialized`: the prefix is shared by every
    /// product, and which build made it is what says whether a pin is a
    /// routine upgrade or a downgrade of everything, see
    /// [`crate::proton::prefix_fit`].
    pub built_by: Option<String>,
    pub link: LinkState,
}

impl PrefixStatus {
    /// Inspect the prefix described by `layout`. Never mutates anything.
    pub fn inspect(layout: &Layout) -> Self {
        let prefix = layout.prefix();
        Self {
            initialized: is_initialized(&prefix),
            built_by: crate::proton::prefix_build(&layout.compat_data()),
            compat_data: layout.compat_data(),
            prefix,
            link: match layout.default_library() {
                Some(main) => {
                    inspect_link(&layout.wow_symlink(), &layout.wow_install_in(main))
                }
                None => LinkState::Missing,
            },
        }
    }

    /// Ready to launch: prefix built and the game visible at its Windows path.
    pub fn is_ready(&self) -> bool {
        self.initialized && self.link.is_usable()
    }
}

/// Whether `wineserver -k -w` left the prefix without a server.
///
/// Measured on GE-Proton11-7: a server it stopped exits 0, no server at all
/// exits 1 and says nothing, and a refusal also exits 1 but says why on
/// stderr (`wineserver: / is not owned by you`).
fn server_gone(status: std::process::ExitStatus, stderr: &[u8]) -> bool {
    status.success() || (status.code() == Some(1) && stderr.iter().all(u8::is_ascii_whitespace))
}

/// Create the prefix if needed and put the symlink in place.
///
/// Ordering matters: the prefix has to exist before anything can be linked into
/// `drive_c`, and Wine must be finished writing before the link is created, or
/// `wineboot` can replace the link with a real directory of its own.
pub fn provision(runner: &ProtonRunner, layout: &Layout) -> Result<PrefixStatus, PrefixError> {
    // A prefix about to be built from nothing can still have a server from the
    // one that stood there before, possibly another Proton build's: a deleted
    // directory does not stop its processes, and the new one can inherit the
    // old one's socket. The scan only matches processes whose environment
    // names this prefix, so nobody else's Wine is touched. Skipped on an
    // existing prefix, where a running client is the user's.
    if !is_initialized(&runner.prefix()) {
        if let Err(e) = crate::preflight::shutdown(runner, layout, crate::preflight::DEFAULT_GRACE) {
            tracing::warn!("could not stop what the old prefix left running: {}", crate::error::chain(&e));
        }
        // What matters is the outcome, not whether the shutdown said it
        // worked: wineboot beside a leftover server hangs or inherits it.
        let left = crate::preflight::scan(layout);
        if !left.is_empty() {
            let names = left.into_iter().map(|p| format!("{} (pid {})", p.name, p.pid));
            return Err(PrefixError::Busy(names.collect()));
        }
    }
    runner.wineboot_update()?;
    runner.wineserver_kill()?;
    link_libraries(layout)?;
    Ok(PrefixStatus::inspect(layout))
}

/// Give every library that is here its place inside the prefix:
///
/// * the `games/` drive letter, so Battle.net can install a title into it;
/// * the WoW drive letter, which the in-game shop needs (see
///   [`Layout::wow_drive`]);
/// * for the default library, the `C:\Program Files (x86)` symlink, the path
///   already-adopted installs have recorded.
///
/// A [`library::State::Missing`] library is skipped entirely: `ensure_link`
/// creates its target, so relinking an unmounted drive would conjure an empty
/// games directory, and an empty install makes the client offer a repair.
pub fn link_libraries(layout: &Layout) -> Result<(), PrefixError> {
    for library in layout.libraries().all() {
        // A library that has never been set up is set up here, which is what
        // makes a first run work: the first library comes out of the config
        // migration unclaimed, and setup is the moment it becomes real. Only
        // ever from `New`, which means there is no marker to adopt, so the
        // id stays the one the config holds, and the `claimed` flag the clone
        // gains is redundant rather than lost: a marker that matches reads as
        // `Ready` whatever the flag says.
        let mut library = library.clone();
        if library.state() == crate::library::State::New {
            library.claim().map_err(|e| match e {
                crate::library::LibraryError::Io { path, source } => {
                    PrefixError::Io { path, source }
                }
                other => PrefixError::Io {
                    path: library.games().to_path_buf(),
                    source: std::io::Error::other(other.to_string()),
                },
            })?;
        }
        if !library.online() {
            continue;
        }
        let library = &library;
        let install = layout.wow_install_in(library);
        ensure_link(&layout.games_drive(library), library.games())?;
        ensure_link(&layout.wow_drive(library), &install)?;
        if library.is_default {
            ensure_link(&layout.wow_symlink(), &install)?;
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum PrefixError {
    #[error("Proton runtime file is missing: {0}")]
    MissingRuntime(PathBuf),

    #[error("could not execute {program}")]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("`{program}` failed{}{}", match code { Some(c) => format!(" (exit {c})"), None => " (killed by signal)".to_owned() }, if stderr.is_empty() { String::new() } else { format!(":\n{stderr}") })]
    Command { program: String, code: Option<i32>, stderr: String },

    #[error("Proton ran but left no usable prefix at {0}")]
    NotInitialized(PathBuf),

    #[error("the old prefix still has processes running, so a new one cannot be built: {}", .0.join(", "))]
    Busy(Vec<String>),

    #[error("{0} is a real directory with files in it, not a symlink — move or remove it first")]
    LinkOccupied(PathBuf),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proton::{ProtonFlavor, ProtonSource};

    #[test]
    fn only_a_silent_exit_1_from_wineserver_means_no_server() {
        use std::os::unix::process::ExitStatusExt;
        let exit = |code: i32| std::process::ExitStatus::from_raw(code << 8);

        assert!(server_gone(exit(0), b""));
        assert!(server_gone(exit(1), b""));
        assert!(server_gone(exit(1), b"\n"));
        assert!(!server_gone(exit(1), b"wineserver: / is not owned by you\n"));
        assert!(!server_gone(exit(2), b""));
        assert!(!server_gone(std::process::ExitStatus::from_raw(9), b""), "killed by a signal");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-prefix-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn runner(root: &Path) -> ProtonRunner {
        let release = ProtonRelease {
            version: "GE-Proton11-7".into(),
            flavor: ProtonFlavor::GloriousEggroll,
            source: ProtonSource::Managed,
            path: root.join("GE-Proton11-7"),
            built_at: None,
        };
        ProtonRunner::new(&release, &Layout::with_root(root.join("Blizzard")), root.join("Steam"))
    }

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }

    #[test]
    fn proton_is_told_where_the_prefix_and_steam_client_live() {
        let root = Path::new("/scratch");
        let cmd = runner(root).command("run", "wineboot", ["-u"]);

        assert_eq!(cmd.get_program(), OsStr::new("/scratch/GE-Proton11-7/proton"));
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["run", "wineboot", "-u"]);
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_DATA_PATH").as_deref(),
            Some("/scratch/Blizzard/prefix")
        );
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_CLIENT_INSTALL_PATH").as_deref(),
            Some("/scratch/Steam")
        );
    }

    #[test]
    fn in_a_container_proton_runs_the_way_steam_starts_it() {
        let cmd = runner(Path::new("/scratch"))
            .in_container("/steam/common/SteamLinuxRuntime_4/_v2-entry-point")
            .command("run", "/home/u/.cache/lightforge/Battle.net-Setup.exe", ["--lang=deDE"]);

        assert_eq!(
            cmd.get_program(),
            OsStr::new("/steam/common/SteamLinuxRuntime_4/_v2-entry-point")
        );
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                "--verb=waitforexitandrun",
                "--",
                "/scratch/GE-Proton11-7/proton",
                "waitforexitandrun",
                "/home/u/.cache/lightforge/Battle.net-Setup.exe",
                "--lang=deDE",
            ]
        );
        // The container sees only what it is shown: the build, the games,
        // and the installer's own directory.
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_TOOL_PATHS").as_deref(),
            Some("/scratch/GE-Proton11-7")
        );
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_MOUNTS").as_deref(),
            Some("/scratch/Blizzard/games:/home/u/.cache/lightforge")
        );
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_DATA_PATH").as_deref(),
            Some("/scratch/Blizzard/prefix")
        );
    }

    #[test]
    fn the_wine_prefix_is_proton_owned_pfx_inside_compat_data() {
        // Proton hard-codes this; the layout has to agree with it.
        let layout = Layout::with_root("/scratch/Blizzard");
        assert_eq!(runner(Path::new("/scratch")).prefix(), layout.prefix());
        assert!(layout.prefix().ends_with("pfx"));
    }

    #[test]
    fn shader_cache_is_only_set_when_a_product_asks_for_one() {
        let root = scratch("shader-cache");
        let cache = root.join("cache/dxvk/wow");
        let plain = runner(&root).command("run", "wineboot", ["-u"]);
        let pinned = runner(&root).with_shader_cache(&cache).command("run", "wineboot", ["-u"]);
        let expected = [
            ("DXVK_SHADER_CACHE_PATH", cache.clone()),
            ("VKD3D_SHADER_CACHE_PATH", cache.clone()),
            ("__GL_SHADER_DISK_CACHE_PATH", cache.join("nvidia")),
            ("MESA_SHADER_CACHE_DIR", cache.join("mesa")),
        ];
        for (key, dir) in expected {
            assert!(env_of(&plain, key).is_none(), "{key}");
            assert_eq!(env_of(&pinned, key).map(PathBuf::from), Some(dir), "{key}");
        }
        assert!(env_of(&pinned, "DXVK_STATE_CACHE_PATH").is_none());
    }

    #[test]
    fn driver_caches_exist_before_the_launch() {
        let root = scratch("driver-caches");
        let cache = root.join("cache/dxvk/wow");
        let _ = runner(&root).with_shader_cache(&cache);
        assert!(cache.join("nvidia").is_dir());
        assert!(cache.join("mesa").is_dir());
    }

    #[test]
    fn explicit_env_overrides_an_assembled_default() {
        let cmd = runner(Path::new("/scratch"))
            .with_env("SteamAppId", "1234")
            .command("run", "wineboot", ["-u"]);
        // Later entries win, so the override must be the last one seen.
        let ids: Vec<_> = cmd
            .get_envs()
            .filter(|(k, _)| *k == OsStr::new("SteamAppId"))
            .map(|(_, v)| v.unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(ids.last().map(String::as_str), Some("1234"));
    }

    #[test]
    fn games_root_is_mounted_into_the_prefix() {
        let cmd = runner(Path::new("/scratch")).command("run", "wineboot", ["-u"]);
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_MOUNTS").as_deref(),
            Some("/scratch/Blizzard/games")
        );
    }

    #[test]
    fn every_library_is_mounted_not_only_the_default() {
        use crate::library::{Libraries, Library};

        let main = Library::main_at(PathBuf::from("/scratch/Blizzard/games"), "home");
        let second = Library::on_partition(Path::new("/mnt/games"), "nvme2", None);
        let layout = Layout::with_root("/scratch/Blizzard")
            .with_libraries(Libraries::new(vec![main, second]));
        let release = ProtonRelease {
            version: "GE-Proton11-7".into(),
            flavor: ProtonFlavor::GloriousEggroll,
            source: ProtonSource::Managed,
            path: PathBuf::from("/scratch/GE-Proton11-7"),
            built_at: None,
        };
        let cmd = ProtonRunner::new(&release, &layout, "/scratch/Steam")
            .command("run", "wineboot", ["-u"]);
        assert_eq!(
            env_of(&cmd, "STEAM_COMPAT_MOUNTS").as_deref(),
            Some("/scratch/Blizzard/games:/mnt/games/Lightforge/games")
        );
    }

    #[test]
    fn a_call_into_a_running_session_does_not_wait_for_it() {
        // Measured: `waitforexitandrun` here waited until the client it was
        // handing a launch to was closed.
        let cmd = runner(Path::new("/scratch"))
            .in_container("/steam/common/SteamLinuxRuntime_4/_v2-entry-point")
            .joining_session()
            .command("run", "/games/Battle.net/Battle.net.exe", ["--exec=launch WoW"]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                "--verb=run",
                "--",
                "/scratch/GE-Proton11-7/proton",
                "run",
                "/games/Battle.net/Battle.net.exe",
                "--exec=launch WoW",
            ]
        );
    }

    #[test]
    fn proton_is_never_handed_a_game_id() {
        // Set at all, even empty, it hangs a GE-Proton11-9 prefix creation.
        let cmd = runner(Path::new("/scratch")).command("run", "wineboot", ["-u"]);
        let game_id: Vec<_> =
            cmd.get_envs().filter(|(k, _)| *k == OsStr::new("SteamGameId")).collect();
        assert_eq!(game_id, [(OsStr::new("SteamGameId"), None)], "removed, not just unset");
    }

    #[test]
    fn another_wines_files_are_never_handed_to_proton() {
        let cmd = runner(Path::new("/scratch")).command("run", "wineboot", ["-u"]);
        for var in ["WINEDLLPATH", "WINELOADER", "WINESERVER"] {
            let seen: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == OsStr::new(var)).collect();
            assert_eq!(seen, [(OsStr::new(var), None)], "{var} removed");
        }
        let overrides = cmd.get_envs().any(|(k, _)| k == OsStr::new("WINEDLLOVERRIDES"));
        assert!(!overrides, "user tuning is left alone, not listed");
    }

    #[test]
    fn a_shells_python_is_not_handed_to_the_proton_script() {
        let cmd = runner(Path::new("/scratch")).command("run", "wineboot", ["-u"]);
        for var in ["PYTHONHOME", "PYTHONPATH", "PYTHONSTARTUP"] {
            let seen: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == OsStr::new(var)).collect();
            assert_eq!(seen, [(OsStr::new(var), None)], "{var} removed");
        }
        let preload = cmd.get_envs().any(|(k, _)| k == OsStr::new("LD_PRELOAD") || k == OsStr::new("LD_LIBRARY_PATH"));
        assert!(!preload, "overlays and capture tools still reach the game");
    }

    #[test]
    fn joining_a_session_changes_nothing_on_the_host() {
        let cmd = runner(Path::new("/scratch")).joining_session().command("run", "wineboot", ["-u"]);
        assert_eq!(cmd.get_program(), OsStr::new("/scratch/GE-Proton11-7/proton"));
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["run", "wineboot", "-u"]);
    }

    #[test]
    fn missing_link_is_created_and_reports_as_correct() {
        let dir = scratch("create");
        let (link, target) = (dir.join("link"), dir.join("target"));
        std::fs::create_dir_all(&target).unwrap();

        assert_eq!(inspect_link(&link, &target), LinkState::Missing);
        assert_eq!(ensure_link(&link, &target).unwrap(), LinkState::Missing);
        assert_eq!(inspect_link(&link, &target), LinkState::Correct);
        // Idempotent: a second pass is a no-op.
        assert_eq!(ensure_link(&link, &target).unwrap(), LinkState::Correct);
    }

    #[test]
    fn a_link_whose_target_is_not_downloaded_yet_is_left_alone() {
        let dir = scratch("pending");
        let (link, target) = (dir.join("link"), dir.join("target"));
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let state = inspect_link(&link, &target);
        assert_eq!(state, LinkState::Pending(target.clone()));
        // Correct as it stands: nothing to repair, and setup is finished.
        assert!(state.is_usable());
        assert_eq!(ensure_link(&link, &target).unwrap(), state);
        assert!(!target.exists(), "linking must not conjure the install");
    }

    #[test]
    fn a_new_link_does_not_create_the_install_it_points_at() {
        let dir = scratch("no-pre-create");
        let (link, target) = (dir.join("link"), dir.join("World of Warcraft"));
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(ensure_link(&link, &target).unwrap(), LinkState::Missing);
        assert!(link.is_symlink());
        assert!(
            !target.exists(),
            "an empty install folder reads as a broken one and takes the name \
             an install found elsewhere would be adopted into"
        );
    }

    #[test]
    fn a_link_to_the_wrong_place_is_relinked() {
        let dir = scratch("mislinked");
        let (link, target, other) = (dir.join("link"), dir.join("target"), dir.join("other"));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();

        assert_eq!(inspect_link(&link, &target), LinkState::Mislinked(other));
        ensure_link(&link, &target).unwrap();
        assert_eq!(inspect_link(&link, &target), LinkState::Correct);
    }

    #[test]
    fn an_empty_directory_is_replaced_but_a_populated_one_is_not() {
        let dir = scratch("occupied");
        let (target, empty, full) = (dir.join("target"), dir.join("empty"), dir.join("full"));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(full.join("Data")).unwrap();

        assert_eq!(inspect_link(&empty, &target), LinkState::OccupiedEmpty);
        ensure_link(&empty, &target).unwrap();
        assert_eq!(inspect_link(&empty, &target), LinkState::Correct);

        assert_eq!(inspect_link(&full, &target), LinkState::Occupied);
        assert!(matches!(ensure_link(&full, &target), Err(PrefixError::LinkOccupied(_))));
        // The refusal must not have disturbed what was there.
        assert!(full.join("Data").is_dir());
    }

    /// The drive is what makes the in-game shop render, so it is worth a test
    /// of its own rather than being assumed from `provision`: which cannot be
    /// tested here, since it runs Proton.
    #[test]
    fn the_wow_drive_is_a_dosdevices_link_to_the_install() {
        let dir = scratch("wow-drive");
        let layout = Layout::with_root(&dir);
        let main = layout.default_library().unwrap().clone();
        let install = layout.wow_install_in(&main);

        ensure_link(&layout.wow_drive(&main), &install).unwrap();

        let link = layout.wow_drive(&main);
        assert_eq!(link.file_name().unwrap(), "d:", "Wine reads the letter in lowercase");
        assert_eq!(link.parent().unwrap().file_name().unwrap(), "dosdevices");
        assert_eq!(std::fs::read_link(&link).unwrap(), install);

        // Idempotent: provisioning runs on every launch. `Pending` rather than
        // `Correct` because nothing has been downloaded into the library yet,
        // and linking deliberately does not create the install directory.
        assert!(!install.exists());
        assert_eq!(
            ensure_link(&layout.wow_drive(&main), &install).unwrap(),
            LinkState::Pending(install.clone()),
        );

        // The `games/` letter is a second, separate mapping, Battle.net needs
        // a directory it can install *into*, the shop needs a drive root.
        ensure_link(&layout.games_drive(&main), main.games()).unwrap();
        assert_eq!(layout.games_drive(&main).file_name().unwrap(), "e:");
        assert_eq!(std::fs::read_link(layout.games_drive(&main)).unwrap(), main.games());
    }

    /// The reason [`link_libraries`] exists: a drive that is not mounted must
    /// not have anything created under it. `ensure_link` creates its target,
    /// so a relink of an absent library would leave an empty games directory
    /// on whatever filesystem holds the mount point, and an empty install is
    /// what makes the client offer a repair.
    #[test]
    fn a_missing_library_is_not_linked_or_created() {
        use crate::library::{Libraries, Library};

        let dir = scratch("missing-library");
        let away = dir.join("not-mounted").join("Lightforge").join("games");
        let mut missing = Library::on_partition(&dir.join("not-mounted"), "nvme2", None);
        missing.games_letter = "f:".to_owned();
        missing.wow_letter = "g:".to_owned();
        // Claimed once, and the marker is not there now.
        missing.claimed = true;
        let layout = Layout::with_root(&dir)
            .with_libraries(Libraries::new(vec![missing.clone()]));

        link_libraries(&layout).unwrap();

        assert!(!away.exists(), "a missing library's games directory was created");
        assert!(!layout.wow_drive(&missing).exists());
        assert!(!layout.games_drive(&missing).exists());
    }

    /// A first run: the first library comes out of the config migration
    /// unclaimed, and setup is the moment it becomes real.
    #[test]
    fn a_new_library_is_set_up_and_linked_in_one_pass() {
        let dir = scratch("new-library");
        let layout = Layout::with_root(&dir);
        let main = layout.default_library().unwrap().clone();
        assert_eq!(main.state(), crate::library::State::New);

        link_libraries(&layout).unwrap();

        assert!(main.online(), "the marker was not written");
        assert_eq!(
            std::fs::read_link(layout.wow_drive(&main)).unwrap(),
            layout.wow_install_in(&main),
        );
        assert_eq!(
            std::fs::read_link(layout.games_drive(&main)).unwrap(),
            main.games(),
        );
        assert_eq!(std::fs::read_link(layout.wow_symlink()).unwrap(), layout.wow_install_in(&main));

        // Idempotent: this runs on every provision.
        link_libraries(&layout).unwrap();
    }

    /// Two libraries, each with its own install, its own letters, and only one
    /// of them on `C:`.
    #[test]
    fn a_second_library_gets_its_own_letters_and_stays_off_the_c_symlink() {
        use crate::library::{Libraries, Library};

        let dir = scratch("two-libraries");
        let main = Library::main_at(dir.join("games"), "home");
        let mut second = Library::on_partition(&dir.join("ssd"), "nvme2", None);
        let (games_letter, wow_letter) = crate::library::allocate_letters(std::slice::from_ref(&main)).unwrap();
        second.games_letter = games_letter;
        second.wow_letter = wow_letter;
        let layout = Layout::with_root(&dir)
            .with_libraries(Libraries::new(vec![main.clone(), second.clone()]));

        link_libraries(&layout).unwrap();

        assert_eq!(
            std::fs::read_link(layout.wow_drive(&second)).unwrap(),
            layout.wow_install_in(&second),
        );
        // One `C:` path, and it is the default library's, the string an install
        // Battle.net has already adopted has recorded on disk.
        assert_eq!(std::fs::read_link(layout.wow_symlink()).unwrap(), layout.wow_install_in(&main));
        assert_ne!(second.wow_letter, main.wow_letter);
        assert_ne!(second.games_letter, main.games_letter);
        assert_eq!(second.windows_wow_path(), "F:/World of Warcraft");
    }

    #[test]
    fn status_is_not_ready_until_wine_has_written_its_hives() {
        let dir = scratch("status");
        let layout = Layout::with_root(&dir);

        let status = PrefixStatus::inspect(&layout);
        assert!(!status.initialized);
        assert!(!status.is_ready());

        std::fs::create_dir_all(layout.drive_c()).unwrap();
        std::fs::write(layout.prefix().join("system.reg"), "WINE REGISTRY Version 2").unwrap();
        ensure_link(&layout.wow_symlink(), &layout.wow_install(&crate::product::ProductCode::Retail)).unwrap();

        assert!(PrefixStatus::inspect(&layout).is_ready());
    }
}
