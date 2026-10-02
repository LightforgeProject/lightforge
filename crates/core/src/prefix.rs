//! Prefix lifecycle: creation via Proton, and the POSIX symlink that puts the
//! game inside a Windows-shaped path.
//!
//! Everything here runs Proton the way Steam does — the `proton` entry-point
//! script with `STEAM_COMPAT_DATA_PATH` and `STEAM_COMPAT_CLIENT_INSTALL_PATH`
//! in the environment — rather than reaching for a system `wine` binary. That
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
}

impl ProtonRunner {
    /// Run `release` against the prefix described by `layout`.
    ///
    /// `steam_client` is what Proton is told to treat as the Steam client
    /// install — the user's real Steam root when they have one, otherwise a
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
            compat_mounts: vec![layout.games_root()],
            shader_cache: None,
            env: Vec::new(),
        }
    }

    /// The Steam client path to hand Proton: the real Steam root if Steam is
    /// installed, else a Lightforge-owned stub under its data directory.
    ///
    /// Proton is not a Steam dependency for Lightforge's purposes, so a missing
    /// Steam install must not be fatal — but the variable still has to point
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

    /// Isolate DXVK's shader cache for one product, per the shared-prefix model.
    pub fn with_shader_cache(mut self, dir: impl Into<PathBuf>) -> Self {
        self.shader_cache = Some(dir.into());
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
        let mut cmd = Command::new(&self.entry_point);
        cmd.arg(verb).arg(program).args(args);

        cmd.env("STEAM_COMPAT_DATA_PATH", &self.compat_data);
        cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &self.steam_client);
        if !self.compat_mounts.is_empty() {
            cmd.env("STEAM_COMPAT_MOUNTS", join_paths(&self.compat_mounts));
        }
        // Proton keys parts of its runtime state off the app id. Lightforge
        // isn't a Steam app, so pin a stable placeholder instead of letting
        // each build invent its own.
        cmd.env("SteamAppId", "0");
        cmd.env("SteamGameId", "0");
        if let Some(cache) = &self.shader_cache {
            cmd.env("DXVK_STATE_CACHE_PATH", cache);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }

    /// Run a program in the prefix and wait for it, failing on a non-zero exit.
    pub fn run<I, S>(&self, program: impl AsRef<OsStr>, args: I) -> Result<(), PrefixError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.prepare()?;
        let program = program.as_ref().to_os_string();
        let output = self
            .command("run", &program, args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| PrefixError::Spawn { program: self.entry_point.clone(), source: e })?;

        if output.status.success() {
            return Ok(());
        }
        Err(PrefixError::Command {
            program: program.to_string_lossy().into_owned(),
            code: output.status.code(),
            stderr: tail(&output.stderr),
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

        // A prefix with no running server exits non-zero; that is the desired
        // end state, not a failure, so the status is deliberately dropped.
        drop(output);
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
    /// A symlink whose target doesn't exist — what a wiped `games/` leaves.
    Dangling(PathBuf),
    /// A symlink pointing somewhere else entirely.
    Mislinked(PathBuf),
    /// A real, empty directory — typically created by an installer that ran
    /// before the link existed. Safe to replace.
    OccupiedEmpty,
    /// A real directory with contents, or a file. Never touched automatically:
    /// this is someone's game install sitting in the disposable prefix.
    Occupied,
}

impl LinkState {
    /// Whether the prefix can be launched from as-is.
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Correct)
    }

    /// Whether [`ensure_link`] can fix this without the user deciding anything.
    pub fn is_repairable(&self) -> bool {
        !matches!(self, Self::Correct | Self::Occupied)
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
        return LinkState::Dangling(PathBuf::new());
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
        (true, false) => LinkState::Dangling(dest),
        (false, _) => LinkState::Mislinked(dest),
    }
}

/// Create or repair the symlink, returning the state found beforehand.
///
/// The target directory is created if absent — a symlink into a not-yet-
/// downloaded install is the normal pre-install state, and Wine resolves it
/// the moment the directory appears.
pub fn ensure_link(link: &Path, target: &Path) -> Result<LinkState, PrefixError> {
    let before = inspect_link(link, target);

    std::fs::create_dir_all(target)
        .map_err(|e| PrefixError::Io { path: target.to_path_buf(), source: e })?;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PrefixError::Io { path: parent.to_path_buf(), source: e })?;
    }

    match &before {
        LinkState::Correct => return Ok(before),
        LinkState::Occupied => return Err(PrefixError::LinkOccupied(link.to_path_buf())),
        LinkState::Dangling(_) | LinkState::Mislinked(_) => std::fs::remove_file(link)
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
    /// Wine has written its registry hives — the prefix is usable.
    pub initialized: bool,
    pub link: LinkState,
}

impl PrefixStatus {
    /// Inspect the prefix described by `layout`. Never mutates anything.
    pub fn inspect(layout: &Layout) -> Self {
        let prefix = layout.prefix();
        Self {
            initialized: is_initialized(&prefix),
            compat_data: layout.compat_data(),
            prefix,
            link: inspect_link(&layout.wow_symlink(), &layout.wow_install()),
        }
    }

    /// Ready to launch: prefix built and the game visible at its Windows path.
    pub fn is_ready(&self) -> bool {
        self.initialized && self.link.is_usable()
    }
}

/// Create the prefix if needed and put the symlink in place.
///
/// Ordering matters: the prefix has to exist before anything can be linked into
/// `drive_c`, and Wine must be finished writing before the link is created, or
/// `wineboot` can replace the link with a real directory of its own.
pub fn provision(runner: &ProtonRunner, layout: &Layout) -> Result<PrefixStatus, PrefixError> {
    runner.wineboot_update()?;
    runner.wineserver_kill()?;
    ensure_link(&layout.wow_symlink(), &layout.wow_install())?;
    // Both, and for different reasons: Battle.net finds the install by the
    // `C:` path it recorded, while the game's in-game shop only renders when
    // the install is a drive of its own — see [`Layout::wow_drive`].
    ensure_link(&layout.wow_drive(), &layout.wow_install())?;
    Ok(PrefixStatus::inspect(layout))
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
    fn the_wine_prefix_is_proton_owned_pfx_inside_compat_data() {
        // Proton hard-codes this; the layout has to agree with it.
        let layout = Layout::with_root("/scratch/Blizzard");
        assert_eq!(runner(Path::new("/scratch")).prefix(), layout.prefix());
        assert!(layout.prefix().ends_with("pfx"));
    }

    #[test]
    fn shader_cache_is_only_set_when_a_product_asks_for_one() {
        let root = Path::new("/scratch");
        assert!(env_of(&runner(root).command("run", "wineboot", ["-u"]), "DXVK_STATE_CACHE_PATH")
            .is_none());

        let pinned = runner(root).with_shader_cache("/cache/dxvk/wow");
        assert_eq!(
            env_of(&pinned.command("run", "wineboot", ["-u"]), "DXVK_STATE_CACHE_PATH").as_deref(),
            Some("/cache/dxvk/wow")
        );
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
    fn missing_link_is_created_and_reports_as_correct() {
        let dir = scratch("create");
        let (link, target) = (dir.join("link"), dir.join("target"));

        assert_eq!(inspect_link(&link, &target), LinkState::Missing);
        assert_eq!(ensure_link(&link, &target).unwrap(), LinkState::Missing);
        assert_eq!(inspect_link(&link, &target), LinkState::Correct);
        // Idempotent: a second pass is a no-op.
        assert_eq!(ensure_link(&link, &target).unwrap(), LinkState::Correct);
    }

    #[test]
    fn a_dangling_link_is_repaired_rather_than_reported_missing() {
        let dir = scratch("dangling");
        let (link, target) = (dir.join("link"), dir.join("target"));
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(inspect_link(&link, &target), LinkState::Dangling(target.clone()));
        assert!(ensure_link(&link, &target).unwrap().is_repairable());
        assert_eq!(inspect_link(&link, &target), LinkState::Correct);
    }

    #[test]
    fn a_link_to_the_wrong_place_is_relinked() {
        let dir = scratch("mislinked");
        let (link, target, other) = (dir.join("link"), dir.join("target"), dir.join("other"));
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();

        assert_eq!(inspect_link(&link, &target), LinkState::Mislinked(other));
        ensure_link(&link, &target).unwrap();
        assert_eq!(inspect_link(&link, &target), LinkState::Correct);
    }

    #[test]
    fn an_empty_directory_is_replaced_but_a_populated_one_is_not() {
        let dir = scratch("occupied");
        let (target, empty, full) = (dir.join("target"), dir.join("empty"), dir.join("full"));
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
    /// of its own rather than being assumed from `provision` — which cannot be
    /// tested here, since it runs Proton.
    #[test]
    fn the_wow_drive_is_a_dosdevices_link_to_the_install() {
        let dir = scratch("wow-drive");
        let layout = Layout::with_root(&dir);

        ensure_link(&layout.wow_drive(), &layout.wow_install()).unwrap();

        let link = layout.wow_drive();
        assert_eq!(link.file_name().unwrap(), "d:", "Wine reads the letter in lowercase");
        assert_eq!(link.parent().unwrap().file_name().unwrap(), "dosdevices");
        assert_eq!(std::fs::read_link(&link).unwrap(), layout.wow_install());

        // Idempotent: provisioning runs on every launch.
        assert_eq!(
            ensure_link(&layout.wow_drive(), &layout.wow_install()).unwrap(),
            LinkState::Correct,
        );
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
        ensure_link(&layout.wow_symlink(), &layout.wow_install()).unwrap();

        assert!(PrefixStatus::inspect(&layout).is_ready());
    }
}
