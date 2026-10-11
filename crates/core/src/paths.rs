//! The canonical on-disk layout.
//!
//! Everything in Lightforge resolves paths through [`Layout`] so the dual
//! directory structure lives in exactly one place:
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
//! Game data deliberately lives outside the prefix and is symlinked in, so
//! Wine and CEF see a conventional Windows storage path (the storefront
//! gray-screen fix) while a prefix rebuild never endangers a 100 GB install.

use std::path::{Path, PathBuf};

use crate::library::{Libraries, Library};
use crate::product::ProductCode;

/// Directory name used for the WoW install on both sides of the symlink.
pub const WOW_DIR: &str = "World of Warcraft";

/// The install root as Windows sees it inside the prefix.
///
/// Not cosmetic: `.product.db` records this string, and Battle.net matches the
/// install by it. It must be the path the symlink presents, not the Linux one.
pub const WINDOWS_WOW_PATH: &str = "C:/Program Files (x86)/World of Warcraft";
/// Windows-side parent the install is presented under, for Battle.net's sake.
const WINDOWS_PROGRAM_FILES: &str = "Program Files (x86)";

/// Resolved filesystem layout: one Wine prefix, and the storage libraries
/// games are installed into.
///
/// The prefix is always at `root`: on the user's home drive, where a Wine
/// prefix belongs, whatever drives the games are spread over. Each library
/// reaches into the prefix through its own `dosdevices` entries rather than
/// moving anything about; see [`crate::library`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    libraries: Libraries,
}

impl Layout {
    /// Layout rooted at an explicit directory, with one library under it.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let games = root.join("games");
        Self {
            libraries: Libraries::new(vec![Library::main_at(games, "home")]),
            root,
        }
    }

    /// The same layout with its games somewhere else; `None` keeps them at
    /// `root/games`.
    ///
    /// Shorthand for a single-library layout, which is what the CLI and the
    /// tests mostly want. [`Layout::with_libraries`] is the general form.
    pub fn with_games(self, games: Option<PathBuf>) -> Self {
        let games = games.unwrap_or_else(|| self.root.join("games"));
        self.with_libraries(Libraries::new(vec![Library::main_at(games, "home")]))
    }

    /// The same layout over a given set of libraries.
    pub fn with_libraries(mut self, libraries: Libraries) -> Self {
        self.libraries = libraries;
        self
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

    /// The Proton compat-data directory, the value of
    /// `STEAM_COMPAT_DATA_PATH`. Proton creates the Wine prefix one level
    /// inside it, at `pfx/`.
    pub fn compat_data(&self) -> PathBuf {
        self.root.join("prefix")
    }

    /// The Wine prefix itself, `prefix/pfx`. Treated as disposable, never
    /// store game or user data here that isn't reproducible.
    ///
    /// The `pfx` component is Proton's, not Lightforge's: Proton always builds
    /// the prefix at `$STEAM_COMPAT_DATA_PATH/pfx`, so a layout that skipped it
    /// would describe a directory Proton never writes to.
    pub fn prefix(&self) -> PathBuf {
        self.compat_data().join("pfx")
    }

    /// `prefix/drive_c`
    pub fn drive_c(&self) -> PathBuf {
        self.prefix().join("drive_c")
    }

    /// The WoW install's own Windows drive letter, as a `dosdevices` entry.
    ///
    /// Not cosmetic, and not interchangeable with [`Layout::wow_symlink`]: the
    /// game's embedded browser, `Utils/BlizzardBrowser.exe`, the CEF that
    /// draws the in-game shop, fails to start its GPU child unless the
    /// install is a drive of its own. Reached through `C:` or through the
    /// catch-all `Z:`, that child dies three times and Chromium gives up,
    /// which is the grey box the shop had instead of a page. Measured
    /// 2026-10-02 against `Logs/BlizzardBrowser/CEF-*.log`.
    ///
    /// One per library, which is what keeps the shop working for a game on a
    /// second drive. The letter is recorded in the library and never moves: a
    /// letter that moves is a recorded install path that stops resolving.
    pub fn wow_drive(&self, library: &Library) -> PathBuf {
        self.dosdevices().join(&library.wow_letter)
    }

    /// A library's `games/` directory as a Windows drive, the directory
    /// Battle.net is told to install other titles into.
    ///
    /// Separate from [`Layout::wow_drive`] because the two want opposite
    /// things: the shop needs the WoW install to *be* a drive root, while
    /// Battle.net needs a drive it can create a game folder *inside*. Wine
    /// resolves a Unix path to the most specific drive mapped over it, so both
    /// hold at once, the install is on its own letter even though its parent
    /// is on another.
    pub fn games_drive(&self, library: &Library) -> PathBuf {
        self.dosdevices().join(&library.games_letter)
    }

    /// `prefix/dosdevices`
    pub fn dosdevices(&self) -> PathBuf {
        self.prefix().join("dosdevices")
    }

    /// Resolve a Windows path the way Wine would, into a real one.
    ///
    /// Reads `dosdevices` rather than assuming anything, so it answers for
    /// every letter the prefix has, `C:` for the prefix's own `drive_c`, and
    /// each library's two letters, without this needing to know which is
    /// which. `None` for a letter the prefix does not map.
    ///
    /// Used to turn the install paths in Battle.net's `product.db` back into
    /// directories that can be measured. Those are the client's own strings,
    /// in its own spelling, and this is the only place that has to care.
    pub fn resolve_windows(&self, windows: &str) -> Option<PathBuf> {
        resolve_windows_in(&self.dosdevices(), windows)
    }

    /// `drive_c/Program Files (x86)`
    pub fn program_files_x86(&self) -> PathBuf {
        self.drive_c().join(WINDOWS_PROGRAM_FILES)
    }

    /// Headless Battle.net install inside the prefix.
    pub fn battle_net_dir(&self) -> PathBuf {
        battle_net_dir_in(&self.prefix())
    }

    /// `%APPDATA%/Battle.net/Battle.net.config`: the file automated setup
    /// injects `GameLaunchWindowBehavior` and `DefaultInstallPath` into.
    ///
    /// `user` is the prefix's Windows user name; Proton prefixes conventionally
    /// use `steamuser`.
    pub fn battle_net_config(&self, user: &str) -> PathBuf {
        self.drive_c()
            .join("users")
            .join(user)
            .join("AppData")
            .join("Roaming")
            .join("Battle.net")
            .join("Battle.net.config")
    }

    /// `%LOCALAPPDATA%/Battle.net/Logs`: where the client writes its own
    /// running commentary, one timestamped file per start.
    ///
    /// Read, never written. It is the only place the client says out loud
    /// that it has finished starting and signed in, which is what a hand-off
    /// has to wait for.
    pub fn battle_net_logs(&self, user: &str) -> PathBuf {
        self.drive_c()
            .join("users")
            .join(user)
            .join("AppData")
            .join("Local")
            .join("Battle.net")
            .join("Logs")
    }

    /// `%LOCALAPPDATA%/Battle.net/Cache`: the client's content-addressed
    /// cache, where it keeps Blizzard's published product catalogue.
    ///
    /// Read, never written. It is the only place in the prefix that says
    /// which game family a product belongs to, which is what a hand-off has
    /// to name; see [`crate::families`].
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
    /// When `winemenubuilder` writes a program's `.desktop` file it also
    /// rasterises that program's icon into `drive_c/proton_shortcuts/icons/`,
    /// one directory per size. The client's own mark is therefore already on
    /// disk in a format we can draw, which is the only reason Lightforge shows
    /// it at all: nothing of Blizzard's is bundled or redistributed, and the
    /// mark always matches the client that is actually installed.
    ///
    /// Wine names the file after a hash of its own, so the directory is
    /// searched rather than the name built. `None` before Battle.net is
    /// installed, which is also the answer on a prefix Wine has not yet built
    /// a menu for.
    pub fn battle_net_icon(&self) -> Option<PathBuf> {
        let icons = self.drive_c().join("proton_shortcuts").join("icons");
        // Largest first: the button draws small, and downscaling a 256 is
        // kinder than upscaling a 32.
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
            // A prefix that has seen more than one Battle.net build keeps a
            // file per build; any of them is the same mark, so the answer is
            // pinned rather than left to the order the directory came back in.
            found.sort();
            found.into_iter().next()
        })
    }

    /// The bootstrapper's staging directory inside the Battle.net install.
    ///
    /// `Battle.net-Setup.exe` unpacks the client here and then promotes it to
    /// `Battle.net.<build>/` alongside. A setup that was torn down at the
    /// moment of promotion leaves a full duplicate behind, roughly 750 MB,
    /// which the client only clears on its next successful launch. Pre-flight
    /// removes it once nothing in the prefix is alive.
    pub fn battle_net_staging(&self) -> PathBuf {
        self.battle_net_dir().join(".battle.net").join(".extract")
    }

    /// Agent's directory cache, deleted by pre-flight cleanup.
    pub fn agent_cache_dir(&self) -> PathBuf {
        self.agent_dir().join("data").join("cache")
    }

    /// Agent's own directory. **Never delete this**: it holds `product.db`,
    /// the registry of every game Battle.net knows is installed. Wiping it
    /// makes the client forget every located install, so the user has to point
    /// it at their games again after every launch, which is exactly what
    /// Lightforge did until this was split out.
    pub fn agent_dir(&self) -> PathBuf {
        agent_dir_in(&self.prefix())
    }

    /// The default library's game assets root: `~/Games/Blizzard/games` unless
    /// the games were put somewhere else.
    ///
    /// The default install target, and what a product with no library of its
    /// own resolves to. Anything addressing a *particular* product's files
    /// goes through [`Layout::wow_install`] instead, because two libraries can
    /// each hold a WoW install.
    pub fn games_root(&self) -> PathBuf {
        self.libraries
            .default()
            .map(|l| l.games().to_path_buf())
            .unwrap_or_else(|| self.root.join("games"))
    }

    /// Where a product's WoW install lives on the native filesystem.
    ///
    /// Per product rather than per layout: each library holds its own
    /// `World of Warcraft/`, with its own `.build.info` and its own CASC
    /// store, so Retail can be on one drive and Classic Era on another.
    pub fn wow_install(&self, code: &ProductCode) -> PathBuf {
        self.library(code)
            .map(|l| l.games().join(WOW_DIR))
            .unwrap_or_else(|| self.games_root().join(WOW_DIR))
    }

    /// The WoW install in a given library, whatever is in it.
    pub fn wow_install_in(&self, library: &Library) -> PathBuf {
        library.games().join(WOW_DIR)
    }

    /// Symlink *source*: the Windows-visible `C:` path inside the prefix.
    ///
    /// There is exactly one of these and it points at the **main** library,
    /// because `C:\Program Files (x86)\World of Warcraft` is a single path
    /// and an install Battle.net has already adopted has that string recorded
    /// on disk. Every other library presents its install under its own
    /// `games/` drive instead; see [`Library::windows_wow_path`].
    pub fn wow_symlink(&self) -> PathBuf {
        self.program_files_x86().join(WOW_DIR)
    }

    /// The `.build.info` at a product's install root. One file holds a row
    /// per product sharing that install, which is why mutation must isolate a
    /// single row.
    pub fn build_info(&self, code: &ProductCode) -> PathBuf {
        self.wow_install(code).join(".build.info")
    }

    /// A product's install directory, e.g. `.../World of Warcraft/_retail_`.
    pub fn product_dir(&self, code: &ProductCode) -> PathBuf {
        self.wow_install(code).join(&*code.install_dir())
    }

    /// A product's addon directory.
    pub fn addons_dir(&self, code: &ProductCode) -> PathBuf {
        self.product_dir(code).join("Interface").join("AddOns")
    }

    /// A product's `WTF` settings directory.
    pub fn wtf_dir(&self, code: &ProductCode) -> PathBuf {
        self.product_dir(code).join("WTF")
    }

    /// The product's game binary, launched directly under Proton for `[ PLAY ]`.
    ///
    /// Resolved against the directory rather than assumed, because the name
    /// varies per flavor and not in a pattern worth guessing: one real install
    /// holds `_retail_/Wow.exe`, `_classic_era_/WowClassic.exe` and
    /// `_classic_beta_/WowB.exe`. The expected name wins when it is there; a
    /// probe covers the rest, including flavors that do not exist yet.
    ///
    /// Falls back to the expected name when nothing is installed, so the error
    /// a caller reports names a plausible file instead of nothing.
    pub fn wow_exe(&self, code: &ProductCode) -> PathBuf {
        game_executable(&self.product_dir(code), code)
    }
}

/// Flatten `.` and `..` out of a path without touching the filesystem.
///
/// Resolve a Windows path against any prefix's `dosdevices` directory.
///
/// Free of [`Layout`] on purpose: a first run reads the install paths out of
/// *another* launcher's Battle.net, whose prefix Lightforge does not own and
/// has no layout for. See [`crate::discover`].
pub fn resolve_windows_in(dosdevices: &Path, windows: &str) -> Option<PathBuf> {
    let windows = windows.replace('\\', "/");
    let (letter, rest) = windows.split_once(':')?;
    let letter = letter.trim();
    if letter.len() != 1 {
        return None;
    }
    let link = dosdevices.join(format!("{}:", letter.to_ascii_lowercase()));
    // The entry is a symlink, and Wine writes `c:` as a *relative* one
    // (`../drive_c`), so it is resolved against the directory it lives in.
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
    // `..` components from a relative `dosdevices` entry would otherwise
    // survive into every comparison made against this path.
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

/// Not `canonicalize`: that resolves symlinks, and the whole point of the
/// paths this is used on is that one of them *is* the symlink putting a game
/// where Windows expects it.
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

/// The game binary inside a flavor directory, resolved by probing it.
///
/// The name varies per flavor and not in a pattern worth guessing: one real
/// install holds `_retail_/Wow.exe`, `_classic_era_/WowClassic.exe` and
/// `_classic_beta_/WowB.exe`. The expected name wins when it is there, a probe
/// covers the rest, and the expected name is the fallback so a caller that
/// finds nothing can still name a plausible file in its error.
///
/// Takes the directory rather than a [`Layout`] because the launch path goes
/// through the prefix symlink, Wine has to see the conventional Windows path,
/// while everything else addresses the install directly.
pub fn game_executable(flavor_dir: &Path, code: &ProductCode) -> PathBuf {
    let expected = flavor_dir.join(code.executable());
    if expected.is_file() {
        return expected;
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

/// XDG locations for Lightforge's own data, kept separate from the game root
/// so wiping `~/Games/Blizzard` doesn't take the user's configuration with it.
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

    /// Cache for ETag'd theming assets (logos, backgrounds) and their manifest.
    pub fn asset_cache_dir(&self) -> PathBuf {
        self.cache.join("assets")
    }

    /// The merged news feed and its ETag, so the News page draws something
    /// on a cold start and keeps reading after the proxy goes away.
    pub fn news_cache_file(&self) -> PathBuf {
        self.cache.join("news.json")
    }

    /// Headline pictures, content-addressed by URL. Beside the feed rather
    /// than inside the asset cache: these are not themed art, nothing prunes
    /// them against a manifest, and mixing the two would put a `Store::prune`
    /// over files it knows nothing about.
    pub fn news_image_dir(&self) -> PathBuf {
        self.cache.join("news")
    }

    /// The download queue as it stood, restored on the next start.
    pub fn queue_file(&self) -> PathBuf {
        self.data.join("queue.toml")
    }

    /// Where Lightforge-managed Proton-GE builds are unpacked.
    pub fn proton_dir(&self) -> PathBuf {
        self.data.join("proton")
    }

    /// DXVK state cache for one product, passed as `DXVK_STATE_CACHE_PATH`.
    ///
    /// Keyed per product deliberately. All products share a single prefix, so
    /// letting DXVK pool its cache there would mean clearing Retail's stale
    /// shaders after a Proton swap also discards Classic's, a stutter penalty
    /// on a product the user never touched.
    pub fn shader_cache_dir(&self, code: &ProductCode) -> PathBuf {
        self.cache.join("dxvk").join(code.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("could not determine the user's home directory")]
    NoHomeDirectory,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // the error message rather than the crash reporter.
        assert_eq!(layout.wow_exe(&code).file_name().unwrap(), "Wow.exe");
    }

    #[test]
    fn an_empty_install_still_names_a_plausible_binary() {
        let (_t, layout, code) = layout_with("wow_classic_era", &[]);
        assert_eq!(layout.wow_exe(&code).file_name().unwrap(), "WowClassic.exe");
    }
}
