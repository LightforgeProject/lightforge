//! The canonical on-disk layout.
//!
//! Every path goes through [`Layout`]:
//!
//! ```text
//! ~/Games/Blizzard/
//! ├── prefix/                       Proton compat-data dir (disposable)
//! │   └── pfx/                      the Wine prefix itself
//! │       └── drive_c/
//! │           └── Program Files (x86)/
//! │               ├── Battle.net/
//! │               └── World of Warcraft/  -> symlink into games/
//! └── games/                        real assets (survive a prefix wipe)
//!     └── World of Warcraft/
//! ```
//!
//! Game data lives outside the prefix and is symlinked in: Wine and CEF see a
//! conventional Windows path (the storefront grey-screen fix), and a prefix
//! rebuild never touches a 100 GB install.

use std::path::{Path, PathBuf};

use crate::library::{Libraries, Library};
use crate::product::ProductCode;

/// Directory name used for the WoW install on both sides of the symlink.
pub const WOW_DIR: &str = "World of Warcraft";

/// The install root as Windows sees it: `.product.db` records this string,
/// and Battle.net matches the install by it.
pub const WINDOWS_WOW_PATH: &str = "C:/Program Files (x86)/World of Warcraft";
/// Windows-side parent the install is presented under, for Battle.net's sake.
const WINDOWS_PROGRAM_FILES: &str = "Program Files (x86)";

/// Resolved filesystem layout: one Wine prefix at `root`, and the storage
/// libraries games go into, each reaching the prefix through its own
/// `dosdevices` entries ([`crate::library`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    libraries: Libraries,
    /// Each game's folder and binary as its product config named them,
    /// where known ([`crate::config::Config::layout`]).
    flavors: std::collections::HashMap<ProductCode, Flavor>,
}

/// Where one game lives inside the WoW install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flavor {
    /// e.g. `_forever_`.
    pub dir: String,
    /// e.g. `WoW.exe`; `None` falls back to [`ProductCode::executable`].
    pub executable: Option<String>,
}

impl Layout {
    /// Layout rooted at an explicit directory, with one library under it.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let games = root.join("games");
        Self {
            libraries: Libraries::new(vec![Library::main_at(games, "home")]),
            root,
            flavors: Default::default(),
        }
    }

    /// One library, at `games` or else `root/games`. See [`Layout::with_libraries`].
    pub fn with_games(self, games: Option<PathBuf>) -> Self {
        let games = games.unwrap_or_else(|| self.root.join("games"));
        self.with_libraries(Libraries::new(vec![Library::main_at(games, "home")]))
    }

    /// The same layout over a given set of libraries.
    pub fn with_libraries(mut self, libraries: Libraries) -> Self {
        self.libraries = libraries;
        self
    }

    /// Where `code` lives in its WoW install, overriding the derived folder.
    pub fn with_flavor(mut self, code: ProductCode, flavor: Flavor) -> Self {
        self.flavors.insert(code, flavor);
        self
    }

    /// The folder `code` is known to live in, recorded or found on disk;
    /// `None` before it is installed.
    pub fn known_flavor(&self, code: &ProductCode) -> Option<&str> {
        self.flavors.get(code).map(|flavor| flavor.dir.as_str())
    }

    /// The folder `code` lives in: recorded, else derived from the code.
    pub fn flavor<'a>(&'a self, code: &'a ProductCode) -> std::borrow::Cow<'a, str> {
        match self.flavors.get(code) {
            Some(flavor) => std::borrow::Cow::Borrowed(flavor.dir.as_str()),
            None => code.install_dir(),
        }
    }

    /// Every library this layout knows about, and which product is in which.
    pub fn libraries(&self) -> &Libraries {
        &self.libraries
    }

    /// The library a product is installed in, or would be. `None` only when
    /// no library is configured at all.
    pub fn library(&self, code: &ProductCode) -> Option<&Library> {
        self.libraries.of(code)
    }

    /// The default install target.
    pub fn default_library(&self) -> Option<&Library> {
        self.libraries.default()
    }

    /// The default layout at `~/Games/Blizzard`.
    pub fn default_for_user() -> Result<Self, PathError> {
        let home = directories::UserDirs::new().ok_or(PathError::NoHomeDirectory)?;
        Ok(Self::with_root(
            home.home_dir().join("Games").join("Blizzard"),
        ))
    }

    /// `~/Games/Blizzard`
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The Proton compat-data directory, `STEAM_COMPAT_DATA_PATH`.
    pub fn compat_data(&self) -> PathBuf {
        self.root.join("prefix")
    }

    /// The Wine prefix itself, `prefix/pfx` (Proton's naming). Disposable:
    /// nothing irreplaceable goes here.
    pub fn prefix(&self) -> PathBuf {
        self.compat_data().join("pfx")
    }

    /// `prefix/drive_c`
    pub fn drive_c(&self) -> PathBuf {
        self.prefix().join("drive_c")
    }

    /// The WoW install's own Windows drive letter, as a `dosdevices` entry.
    /// The in-game shop's browser only starts its GPU child on a drive of its
    /// own; through `C:` or `Z:` it is a grey box (measured 2026-10-02,
    /// `Logs/BlizzardBrowser/CEF-*.log`). One per library, never moved.
    pub fn wow_drive(&self, library: &Library) -> PathBuf {
        self.dosdevices().join(&library.wow_letter)
    }

    /// A library's `games/` directory as a Windows drive, for Battle.net to
    /// install other titles into. Wine resolves to the most specific drive, so
    /// the install still gets [`Layout::wow_drive`].
    pub fn games_drive(&self, library: &Library) -> PathBuf {
        self.dosdevices().join(&library.games_letter)
    }

    /// `prefix/dosdevices`
    pub fn dosdevices(&self) -> PathBuf {
        self.prefix().join("dosdevices")
    }

    /// Resolve a Windows path the way Wine would, through `dosdevices`, for
    /// the install paths in Battle.net's `product.db`. `None` for an unmapped letter.
    pub fn resolve_windows(&self, windows: &str) -> Option<PathBuf> {
        resolve_windows_in(&self.dosdevices(), windows)
    }

    /// `drive_c/Program Files (x86)`
    pub fn program_files_x86(&self) -> PathBuf {
        self.drive_c().join(WINDOWS_PROGRAM_FILES)
    }

    /// Unattended Battle.net install inside the prefix.
    pub fn battle_net_dir(&self) -> PathBuf {
        battle_net_dir_in(&self.prefix())
    }

    /// `%APPDATA%/Battle.net/Battle.net.config`. `user` is the prefix's
    /// Windows user, `steamuser` under Proton.
    pub fn battle_net_config(&self, user: &str) -> PathBuf {
        self.drive_c()
            .join("users")
            .join(user)
            .join("AppData")
            .join("Roaming")
            .join("Battle.net")
            .join("Battle.net.config")
    }

    /// `%LOCALAPPDATA%/Battle.net/Logs`, one file per start: the only place
    /// the client says it has signed in. Read, never written.
    pub fn battle_net_logs(&self, user: &str) -> PathBuf {
        self.drive_c()
            .join("users")
            .join(user)
            .join("AppData")
            .join("Local")
            .join("Battle.net")
            .join("Logs")
    }

    /// `%LOCALAPPDATA%/Battle.net/Cache`, holding Blizzard's product catalogue
    /// with each game's family ([`crate::families`]). Read, never written.
    pub fn battle_net_cache(&self, user: &str) -> PathBuf {
        self.drive_c()
            .join("users")
            .join(user)
            .join("AppData")
            .join("Local")
            .join("Battle.net")
            .join("Cache")
    }

    /// The Battle.net mark, as Wine left it in the prefix.
    ///
    /// `winemenubuilder` rasterises it into `drive_c/proton_shortcuts/icons/`
    /// under a hash name, so the directory is searched. `None` until then.
    pub fn battle_net_icon(&self) -> Option<PathBuf> {
        let icons = self.drive_c().join("proton_shortcuts").join("icons");
        // Largest first: downscaling looks better.
        ["256x256", "128x128", "64x64", "48x48"].into_iter().find_map(|size| {
            let apps = icons.join(size).join("apps");
            let mut found: Vec<PathBuf> = std::fs::read_dir(apps)
                .ok()?
                .filter_map(|entry| Some(entry.ok()?.path()))
                .filter(|path| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    name.contains("Battle.net") && name.ends_with(".png")
                })
                .collect();
            // One file per Battle.net build, the same mark; pick one stably.
            found.sort();
            found.into_iter().next()
        })
    }

    /// The bootstrapper's staging directory. A setup torn down mid-promotion
    /// leaves a ~750 MB duplicate here, which pre-flight removes.
    pub fn battle_net_staging(&self) -> PathBuf {
        self.battle_net_dir().join(".battle.net").join(".extract")
    }

    /// Agent's directory cache, deleted by pre-flight cleanup.
    pub fn agent_cache_dir(&self) -> PathBuf {
        self.agent_dir().join("data").join("cache")
    }

    /// Agent's own directory. **Never delete this**: its `product.db` is
    /// every install Battle.net knows.
    pub fn agent_dir(&self) -> PathBuf {
        agent_dir_in(&self.prefix())
    }

    /// The default library's games root. A particular product's files go
    /// through [`Layout::wow_install`]: libraries each hold an install.
    pub fn games_root(&self) -> PathBuf {
        self.libraries
            .default()
            .map(|l| l.games().to_path_buf())
            .unwrap_or_else(|| self.root.join("games"))
    }

    /// Where a product's WoW install lives, per library.
    pub fn wow_install(&self, code: &ProductCode) -> PathBuf {
        self.library(code)
            .map(|l| l.games().join(WOW_DIR))
            .unwrap_or_else(|| self.games_root().join(WOW_DIR))
    }

    /// The WoW install in a given library, whatever is in it.
    pub fn wow_install_in(&self, library: &Library) -> PathBuf {
        library.games().join(WOW_DIR)
    }

    /// The `C:` symlink, for the **main** library only: adopted installs have
    /// that path recorded. Others use [`Library::windows_wow_path`].
    pub fn wow_symlink(&self) -> PathBuf {
        self.program_files_x86().join(WOW_DIR)
    }

    /// The `.build.info` at a product's install root, a row per product.
    pub fn build_info(&self, code: &ProductCode) -> PathBuf {
        self.wow_install(code).join(".build.info")
    }

    /// A product's install directory, e.g. `.../World of Warcraft/_retail_`.
    pub fn product_dir(&self, code: &ProductCode) -> PathBuf {
        self.wow_install(code).join(&*self.flavor(code))
    }

    /// A product's addon directory.
    pub fn addons_dir(&self, code: &ProductCode) -> PathBuf {
        self.product_dir(code).join("Interface").join("AddOns")
    }

    /// A product's `WTF` settings directory.
    pub fn wtf_dir(&self, code: &ProductCode) -> PathBuf {
        self.product_dir(code).join("WTF")
    }

    /// The product's game binary, for `[ PLAY ]`; see [`game_executable`].
    pub fn wow_exe(&self, code: &ProductCode) -> PathBuf {
        let name = self.flavors.get(code).and_then(|f| f.executable.as_deref()).unwrap_or(code.executable());
        game_executable(&self.product_dir(code), name)
    }
}

/// Resolve a Windows path against any prefix's `dosdevices`, including
/// another launcher's ([`crate::discover`]).
pub fn resolve_windows_in(dosdevices: &Path, windows: &str) -> Option<PathBuf> {
    let windows = windows.replace('\\', "/");
    let (letter, rest) = windows.split_once(':')?;
    let letter = letter.trim();
    if letter.len() != 1 {
        return None;
    }
    let link = dosdevices.join(format!("{}:", letter.to_ascii_lowercase()));
    // Wine writes `c:` as a relative link (`../drive_c`).
    let target = std::fs::read_link(&link).ok()?;
    let target = match target.is_absolute() {
        true => target,
        false => dosdevices.join(target),
    };
    let relative = rest.trim_start_matches('/');
    let resolved = match relative.is_empty() {
        true => target,
        false => target.join(relative),
    };
    // Drop the relative link's `..`.
    Some(normalize(&resolved))
}

/// Agent's own directory inside any Wine prefix, ours or another launcher's,
/// where `product.db` lives. See [`Layout::agent_dir`].
pub fn agent_dir_in(prefix: &Path) -> PathBuf {
    prefix.join("drive_c").join("ProgramData").join("Battle.net").join("Agent")
}

/// Battle.net's install directory inside any Wine prefix.
pub fn battle_net_dir_in(prefix: &Path) -> PathBuf {
    prefix.join("drive_c").join(WINDOWS_PROGRAM_FILES).join("Battle.net")
}

/// Whether a directory is a Wine prefix at all.
pub fn is_prefix(dir: &Path) -> bool {
    dir.join("drive_c").is_dir()
}

/// Flatten `.` and `..` without touching the filesystem. Not `canonicalize`:
/// one of these paths is the game symlink itself.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The folder `code` lives in under the WoW install `install`, for code
/// without a [`Layout`]: the one whose `.flavor.info` names it (every install
/// writes one, Battle.net's too), else the folder derived from the code.
pub fn flavor_dir_name(install: &Path, code: &ProductCode) -> String {
    let names = |dir: &Path| {
        std::fs::read_to_string(dir.join(".flavor.info"))
            .is_ok_and(|text| text.lines().nth(1).is_some_and(|line| line.trim() == code.as_str()))
    };
    std::fs::read_dir(install)
        .into_iter()
        .flatten()
        .flatten()
        .find(|entry| entry.file_type().is_ok_and(|t| t.is_dir()) && names(&entry.path()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .unwrap_or_else(|| code.install_dir().into_owned())
}

/// The game binary inside a flavor directory, resolved by probing it.
///
/// One real install holds `_retail_/Wow.exe`, `_classic_era_/WowClassic.exe`
/// and `_classic_beta_/WowB.exe`. The expected name wins, matched without
/// case (retail's config says `WoW.exe`, the file is `Wow.exe`), a probe
/// covers the rest, and with nothing found the expected name comes back for
/// the error.
pub fn game_executable(flavor_dir: &Path, name: &str) -> PathBuf {
    let expected = flavor_dir.join(name);
    if expected.is_file() {
        return expected;
    }
    let same_name = std::fs::read_dir(flavor_dir).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
    });
    if let Some(found) = same_name {
        return found;
    }

    std::fs::read_dir(flavor_dir)
        .ok()
        .and_then(|entries| {
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    // `BlizzardError.exe` ships beside the game in every
                    // flavor and is a crash reporter, not the client.
                    p.is_file()
                        && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                        && p.file_name()
                            .is_some_and(|n| !n.eq_ignore_ascii_case("BlizzardError.exe"))
                })
                .collect();
            found.sort();
            found.into_iter().next()
        })
        .unwrap_or(expected)
}

/// XDG locations for Lightforge's own data, apart from the game root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    config: PathBuf,
    cache: PathBuf,
    data: PathBuf,
}

impl AppDirs {
    pub fn discover() -> Result<Self, PathError> {
        let dirs = directories::ProjectDirs::from("io", "Tuxcore", "Lightforge")
            .ok_or(PathError::NoHomeDirectory)?;
        Ok(Self {
            config: dirs.config_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
            data: dirs.data_dir().to_path_buf(),
        })
    }

    pub fn with_dirs(
        config: impl Into<PathBuf>,
        cache: impl Into<PathBuf>,
        data: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config: config.into(),
            cache: cache.into(),
            data: data.into(),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// `~/.config/lightforge/config.toml`
    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    /// The launcher's own log files, see [`crate::logging`].
    pub fn log_dir(&self) -> PathBuf {
        self.data.join("logs")
    }

    /// Cache for ETag'd theming assets (logos, backgrounds) and their manifest.
    pub fn asset_cache_dir(&self) -> PathBuf {
        self.cache.join("assets")
    }

    /// The download queue as it stood, restored on the next start.
    pub fn queue_file(&self) -> PathBuf {
        self.data.join("queue.toml")
    }

    /// Where Lightforge-managed Proton-GE builds are unpacked.
    pub fn proton_dir(&self) -> PathBuf {
        self.data.join("proton")
    }

    /// Shader caches for one product: DXVK's and VKD3D's, and the NVIDIA and
    /// Mesa drivers' below it, see [`crate::prefix::ProtonRunner::with_shader_cache`].
    ///
    /// Per product, so a Proton swap on one does not clear another's.
    pub fn shader_cache_dir(&self, code: &ProductCode) -> PathBuf {
        self.cache.join("dxvk").join(code.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("could not determine the user's home directory")]
    NoHomeDirectory,
}


/// Prove `dir` will take a new file, repairing the one case we own.
///
/// A root from another tool can refuse a file of its own, which otherwise
/// shows only at the end of a download. A missing owner-write bit on our own
/// directory is added; anything else is reported with its cause.
pub fn ensure_writable(dir: &Path) -> Result<Repaired, std::io::Error> {
    std::fs::create_dir_all(dir)?;
    match probe(dir) {
        Ok(()) => return Ok(Repaired::No),
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => return Err(e),
        Err(e) => {
            if !owned_here(dir) {
                return Err(e);
            }
            let Ok(meta) = std::fs::metadata(dir) else { return Err(e) };
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions());
            // Only the owner-write bit, and only when it is the bit missing.
            if mode & 0o200 != 0 {
                return Err(e);
            }
            let fixed = std::os::unix::fs::PermissionsExt::from_mode(mode | 0o200);
            std::fs::set_permissions(dir, fixed).map_err(|_| e)?;
        }
    }
    probe(dir).map(|()| Repaired::Yes)
}

/// Whether the directory belongs to the user running this process.
fn owned_here(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir).is_ok_and(|meta| meta.uid() == unsafe { libc::geteuid() })
}

fn probe(dir: &Path) -> Result<(), std::io::Error> {
    let probe = dir.join(".lightforge-write-probe");
    std::fs::File::create(&probe)?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Whether [`ensure_writable`] had to change the directory's mode to succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repaired {
    No,
    /// The owner-write bit was missing and was added; worth reporting.
    Yes,
}

#[cfg(test)]
mod tests {
    /// A directory writable through its subdirectories that refuses a file
    /// of its own: an install root from another tool.
    #[test]
    fn a_root_that_refuses_a_file_is_repaired_when_we_own_it() {
        use std::os::unix::fs::PermissionsExt;

        // root ignores the mode bits, so there is nothing to prove there.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = std::env::temp_dir()
            .join(format!("lightforge-writable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(ensure_writable(&dir).unwrap(), Repaired::No);

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(
            ensure_writable(&dir).unwrap(),
            Repaired::Yes,
            "a directory this user owns is made writable rather than reported"
        );
        // Only the owner-write bit, and nothing else about the directory.
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "only the owner-write bit was added, got {mode:o}");
        // And it stays fixed, so the next run reports nothing.
        assert_eq!(ensure_writable(&dir).unwrap(), Repaired::No);
        // The probe leaves nothing behind.
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A refusal that is not ours to fix keeps its cause.
    #[test]
    fn a_refusal_we_cannot_fix_comes_back_with_its_cause() {
        let root = std::env::temp_dir()
            .join(format!("lightforge-unwritable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A regular file where a parent directory should be: nothing about
        // permissions, and nothing a mode change could help.
        let file = root.join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();

        let error = ensure_writable(&file.join("World of Warcraft"))
            .expect_err("a file is not a directory to install into");
        let rendered = crate::error::chain(&error);
        assert!(!rendered.is_empty(), "the io error must survive");

        let _ = std::fs::remove_dir_all(&root);
    }

    use super::*;

    /// `_forever_/.flavor.info` as Battle.net wrote it, 2026-10-22.
    const FOREVER_FLAVOR_INFO: &str = "Product Flavor!STRING:0\nwowf\n";

    #[test]
    fn a_game_is_found_in_whatever_folder_its_flavor_info_is_in() {
        let install = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(install.path().join("_forever_")).unwrap();
        std::fs::write(install.path().join("_forever_/.flavor.info"), FOREVER_FLAVOR_INFO).unwrap();
        std::fs::create_dir_all(install.path().join("_retail_")).unwrap();

        assert_eq!(flavor_dir_name(install.path(), &ProductCode::Forever), "_forever_");
        // Nothing names retail: derived from its code.
        assert_eq!(flavor_dir_name(install.path(), &ProductCode::Retail), "_retail_");
    }

    #[test]
    fn the_binary_is_matched_without_case() {
        let flavor = tempfile::tempdir().unwrap();
        std::fs::write(flavor.path().join("Wow.exe"), b"MZ").unwrap();
        assert_eq!(game_executable(flavor.path(), "WoW.exe"), flavor.path().join("Wow.exe"));
    }

    fn layout() -> Layout {
        Layout::with_root("/games/Blizzard")
    }

    #[test]
    fn game_data_lives_outside_the_prefix() {
        let l = layout();
        assert!(!l.wow_install(&ProductCode::Retail).starts_with(l.prefix()));
        assert!(l.wow_symlink().starts_with(l.prefix()));
    }

    #[test]
    fn the_battle_net_mark_is_found_by_searching_wines_icon_directory() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path());
        assert_eq!(layout.battle_net_icon(), None, "nothing is installed yet");

        let apps = layout
            .drive_c()
            .join("proton_shortcuts")
            .join("icons")
            .join("256x256")
            .join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("other.png"), b"").unwrap();
        std::fs::write(apps.join("1AC2_Battle.net Launcher.0.png"), b"").unwrap();

        assert_eq!(
            layout.battle_net_icon(),
            Some(apps.join("1AC2_Battle.net Launcher.0.png")),
            "the mark is found under a name Wine chose, not one we built"
        );
    }

    #[test]
    fn the_largest_battle_net_mark_wins() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path());
        let icons = layout.drive_c().join("proton_shortcuts").join("icons");
        for size in ["48x48", "256x256"] {
            let apps = icons.join(size).join("apps");
            std::fs::create_dir_all(&apps).unwrap();
            std::fs::write(apps.join("1AC2_Battle.net Launcher.0.png"), b"").unwrap();
        }

        assert_eq!(
            layout.battle_net_icon(),
            Some(icons.join("256x256").join("apps").join("1AC2_Battle.net Launcher.0.png")),
            "downscaling a big mark beats upscaling a small one"
        );
    }

    #[test]
    fn symlink_uses_a_windows_program_files_path() {
        assert_eq!(
            layout().wow_symlink(),
            PathBuf::from(
                "/games/Blizzard/prefix/pfx/drive_c/Program Files (x86)/World of Warcraft"
            )
        );
    }

    #[test]
    fn products_in_one_library_share_its_build_info_but_keep_separate_dirs() {
        let l = layout();
        assert_eq!(
            l.build_info(&ProductCode::Retail),
            l.build_info(&ProductCode::Classic),
        );
        assert_eq!(
            l.build_info(&ProductCode::Retail),
            PathBuf::from("/games/Blizzard/games/World of Warcraft/.build.info")
        );
        assert_ne!(
            l.product_dir(&ProductCode::Retail),
            l.product_dir(&ProductCode::Classic)
        );
    }

    #[test]
    fn shader_caches_are_isolated_per_product() {
        let dirs = AppDirs::with_dirs("/c", "/cache", "/data");
        assert_ne!(
            dirs.shader_cache_dir(&ProductCode::Retail),
            dirs.shader_cache_dir(&ProductCode::Classic)
        );
        assert_eq!(
            dirs.shader_cache_dir(&ProductCode::Retail),
            PathBuf::from("/cache/dxvk/wow")
        );
    }

    #[test]
    fn addons_resolve_under_the_product_dir() {
        assert_eq!(
            layout().addons_dir(&ProductCode::Retail),
            PathBuf::from("/games/Blizzard/games/World of Warcraft/_retail_/Interface/AddOns")
        );
    }
}

#[cfg(test)]
mod exe_tests {
    use super::*;

    fn layout_with(flavor: &str, files: &[&str]) -> (tempfile::TempDir, Layout, ProductCode) {
        let temp = tempfile::TempDir::new().unwrap();
        let layout = Layout::with_root(temp.path());
        let code: ProductCode = flavor.parse().unwrap();
        let dir = layout.product_dir(&code);
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), b"MZ").unwrap();
        }
        (temp, layout, code)
    }

    #[test]
    fn the_expected_binary_wins_when_it_is_installed() {
        let (_t, layout, code) =
            layout_with("wow_classic_era", &["WowClassic.exe", "BlizzardError.exe"]);
        assert_eq!(layout.wow_exe(&code).file_name().unwrap(), "WowClassic.exe");
    }

    #[test]
    fn an_unexpected_name_is_found_by_probing() {
        // A real install's `_classic_beta_` ships WowB.exe, which no table
        // predicted; the directory is the authority.
        let (_t, layout, code) = layout_with("wow_classic_beta", &["WowB.exe", "BlizzardError.exe"]);
        assert_eq!(layout.wow_exe(&code).file_name().unwrap(), "WowB.exe");
    }

    #[test]
    fn the_crash_reporter_is_never_mistaken_for_the_game() {
        let (_t, layout, code) = layout_with("wow", &["BlizzardError.exe"]);
        // Nothing launchable is present, so the expected name comes back for
        // the error message, not the crash reporter.
        assert_eq!(layout.wow_exe(&code).file_name().unwrap(), "Wow.exe");
    }

    #[test]
    fn an_empty_install_still_names_a_plausible_binary() {
        let (_t, layout, code) = layout_with("wow_classic_era", &[]);
        assert_eq!(layout.wow_exe(&code).file_name().unwrap(), "WowClassic.exe");
    }
}
