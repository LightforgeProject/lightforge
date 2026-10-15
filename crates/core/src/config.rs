//! Persisted application configuration.
//!
//! TOML at `~/.config/lightforge/config.toml`, deliberately outside
//! `~/Games/Blizzard` so wiping the game root never destroys the user's
//! per-product pins and overrides. The format is hand-editable on purpose,
//! realmlist and CDN overrides are exactly the fields people poke manually.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::product::{Product, ProductCode, ProductError};
use crate::proton::ProtonPin;

/// Windows user name inside a Proton prefix.
const DEFAULT_PREFIX_USER: &str = "steamuser";

/// How Lightforge is used on a SteamOS machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteamOsUse {
    /// A Steam Deck or a Steam Machine: controller input.
    Console,
    /// SteamOS on a desktop PC: mouse and keyboard.
    Desktop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Root of the dual-directory layout. Defaults to `~/Games/Blizzard`.
    pub blizzard_root: Option<PathBuf>,
    /// Where games are installed, when not under `blizzard_root`.
    ///
    /// Superseded by `libraries`, and kept only so a config written before
    /// them still points at the right drive: [`Config::ensure_libraries`]
    /// turns it into the first library once, and nothing reads it afterwards.
    pub games_dir: Option<PathBuf>,
    /// Every drive Lightforge may install games to, the default one first.
    ///
    /// Empty in a config written before multi-drive storage, and seeded from
    /// `blizzard_root`/`games_dir` by [`Config::ensure_libraries`].
    #[serde(default)]
    pub libraries: Vec<crate::library::Library>,
    /// Windows user name within the prefix.
    pub prefix_user: String,
    /// Proton build used for the Battle.net/Agent prefix specifically.
    pub battle_net_proton: ProtonPin,
    /// Fallback Proton build for products that don't pin their own.
    pub default_proton: ProtonPin,
    /// An override for where themed branding is read from: a base URL, or a
    /// local directory for trying out new art. Deliberately absent from the
    /// UI, `None` means [`DEFAULT_ASSETS_SOURCE`], which is what everyone
    /// but whoever is making the art should be on.
    pub assets_source: Option<String>,
    /// Base URL of the addon catalog proxy, or a local fixture directory.
    /// `None` disables addon management rather than failing at use.
    pub addon_catalog: Option<String>,
    /// Which addon builds to accept for an addon that has no channel of its
    /// own. Stable by default; a per-addon override lives in the ledger.
    #[serde(default)]
    pub addon_channel: crate::addons::Channel,
    /// Whether addons update themselves when Lightforge makes its own pass,
    /// for an addon that has no opinion of its own.
    ///
    /// On by default: keeping addons current is what an addon manager is for,
    /// and an addon that should not move has two ways to say so, its own
    /// auto-update choice, or being ignored outright.
    #[serde(default = "yes")]
    pub addon_auto_update: bool,
    /// Where the person can be reached about a bug report, an address, a
    /// Discord name, whatever they chose to type. Optional, kept only so the
    /// report dialog can offer it back rather than asking twice, and never
    /// sent anywhere except with a report they pressed Send on.
    #[serde(default)]
    pub contact: Option<String>,
    /// Cap on game download speed, in megabytes per second. `None` is
    /// unlimited.
    ///
    /// MB rather than MiB because it is the unit the progress readout already
    /// uses, and a limit that does not match the number beside it is a support
    /// question waiting to happen.
    pub download_limit_mbps: Option<u64>,
    /// Whether launching a game also pauses downloads for *other* products.
    ///
    /// The launched product's own download is always paused. That one is a
    /// correctness guard, not a preference, since the client and the
    /// downloader would be reading and writing one CASC store. This governs
    /// the rest of the queue, which shares that same store and the same
    /// connection.
    pub pause_downloads_while_playing: bool,
    /// Whether the launcher gets out of the way once a game is up.
    ///
    /// Off by default: a window that disappears without being asked is
    /// startling, and the launcher is where the download queue and the status
    /// line live. Nothing is restored when the game exits, the window is in
    /// the taskbar, and a launcher that raises itself over whatever came next
    /// is the behaviour people disable this kind of setting to avoid.
    #[serde(default)]
    pub minimize_on_launch: bool,
    /// Whether Battle.net comes up minimized when it is only being used to
    /// hand a game over.
    ///
    /// On by default, and still only honoured where the client can sign itself
    /// in; see [`crate::battlenet::signs_in_by_itself`]. A sign-in form
    /// nobody can see is a launch that quietly never happens.
    #[serde(default = "yes")]
    pub battle_net_minimized: bool,
    /// How Battle.net and every game it launches are started.
    ///
    /// **Shared, not per-game, and that is the mechanism rather than a
    /// simplification.** Lightforge only ever sets the environment of a
    /// Battle.net client it starts *cold*; a client that is already up takes
    /// the launch over its own IPC queue, where there is no new process to
    /// give an environment to. Two games started from one client session
    /// therefore cannot have different environments, so offering a per-game
    /// control would be a lie; see `SPEC.md`, "Per-product pinning".
    ///
    /// Only the host and runtime half of [`crate::tweaks::Tweaks`] is read
    /// here. A hand-off game has no `Config.wtf`, so
    /// [`crate::tweaks::Tweaks::cvars`] is never called on this value.
    #[serde(default)]
    pub battle_net_tweaks: crate::tweaks::Tweaks,
    /// Which of Blizzard's regions products are downloaded from.
    ///
    /// Asked once, during the first run. It decides which version service is
    /// read, and it is written into `.build.info` and `product.db`: the client
    /// and Battle.net both expect the install to say where it came from.
    #[serde(default)]
    pub region: crate::tact::version::Region,
    /// The language the game is installed in, e.g. `deDE`.
    ///
    /// A download decision, not a display one: root lists most files once per
    /// locale, and the locale is what selects the right ones. Changing it after
    /// an install means downloading the new language's files, which is why it is
    /// asked at setup rather than guessed from `LANG`.
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Whether the first-run wizard has been seen through to the end.
    ///
    /// Not the same question as "is the machine provisioned": someone who
    /// dismissed the wizard with "set up later" has seen it, and showing it
    /// again on every start would be nagging. [`crate::setup::Status`] still
    /// knows what is missing, and Settings still says so.
    #[serde(default)]
    pub setup_complete: bool,
    /// Battle.net was declined during setup.
    ///
    /// Only WoW works without it, so this is also what hides the Battle.net
    /// button and the hand-off titles: a button that opens a client that is not
    /// installed is worse than no button. Reversible from Settings, and the
    /// wizard says so when it is set.
    #[serde(default)]
    pub battle_net_declined: bool,
    /// What a SteamOS machine is used as, asked during setup and only there.
    ///
    /// `None` everywhere else, and on SteamOS until the question is answered.
    /// The same OS runs a handheld, a box under a television, and an ordinary
    /// desktop, and nothing on the machine says which one this is.
    #[serde(default)]
    pub steamos_use: Option<SteamOsUse>,
    /// Whether the News panel is shown beside the game's key art.
    ///
    /// Off by default: a first run should show the game's key art and nothing
    /// else, and the panel also costs a network fetch on every visit to the
    /// Play page. Settings › General turns it on, and that is where someone
    /// who wants news about the game they are looking at will find it.
    #[serde(default)]
    pub news_on_play: bool,
    /// Whether the release-notes popup has been switched off for good.
    ///
    /// The checkbox on it says "never show this again", and that is what it
    /// means: no future release opens it either. Settings › General turns it
    /// back on, and Lightforge Updates shows the notes whatever this says.
    #[serde(default)]
    pub patch_notes_muted: bool,
    /// The version whose notes have already been shown.
    ///
    /// Without this the popup would open on every start rather than once per
    /// update. `None` on a machine that has never seen it, which includes
    /// every install from before the popup existed, and those *should* see the
    /// notes for what they just updated to.
    #[serde(default)]
    pub patch_notes_seen: Option<String>,
    /// Whether to ask GitHub, once per start, if a newer Lightforge exists.
    ///
    /// On by default and listed in `DISCLAIMER.md`, which is the file that
    /// says exactly what this program contacts. Nothing is ever installed:
    /// the answer is a sentence and a link to the releases page.
    #[serde(default = "yes")]
    pub check_for_updates: bool,
    /// The newest version already mentioned, so the notice opens once per
    /// release rather than on every start.
    #[serde(default)]
    pub update_seen: Option<String>,
    /// How many `WTF/Account` backups each game keeps.
    #[serde(default = "default_backup_keep")]
    pub backup_keep: usize,
    /// Every product Lightforge manages, official and custom alike.
    pub products: Vec<Product>,
}

/// Where themed branding comes from unless `assets_source` says otherwise.
pub const DEFAULT_ASSETS_SOURCE: &str = "https://lightforge.tuxcore.pro/assets/";

/// Where the addon catalog is read from unless `addon_catalog` says otherwise.
///
/// The CurseForge API needs a key, and a key cannot live in a GPL binary, so
/// CurseForge is reachable only through a proxy that holds one. Defaulting to
/// ours means addon search works out of the box; pointing `addon_catalog` at
/// your own proxy, or at a fixture directory, replaces it.
///
/// The keyless providers (WoWInterface, Tukui, GitHub) never come through
/// here, so addon management keeps working whatever this says.
pub const DEFAULT_ADDON_CATALOG: &str = "https://lightforge.tuxcore.pro/";

/// Bytes in the megabyte this config counts in.
const BYTES_PER_MB: u64 = 1_000_000;

/// `true`, for a `serde(default)` that is on rather than off.
fn yes() -> bool {
    true
}

/// The language a WoW install is in unless the user picked another.
pub const DEFAULT_LOCALE: &str = "enUS";

fn default_backup_keep() -> usize {
    crate::backup::DEFAULT_KEEP
}

fn default_locale() -> String {
    DEFAULT_LOCALE.to_owned()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            blizzard_root: None,
            games_dir: None,
            libraries: Vec::new(),
            prefix_user: DEFAULT_PREFIX_USER.to_owned(),
            battle_net_proton: ProtonPin::default(),
            default_proton: ProtonPin::default(),
            assets_source: None,
            addon_catalog: None,
            addon_channel: crate::addons::Channel::default(),
            addon_auto_update: true,
            contact: None,
            download_limit_mbps: None,
            pause_downloads_while_playing: true,
            minimize_on_launch: false,
            battle_net_minimized: true,
            battle_net_tweaks: crate::tweaks::Tweaks::default(),
            region: crate::tact::version::Region::default(),
            locale: default_locale(),
            setup_complete: false,
            battle_net_declined: false,
            steamos_use: None,
            news_on_play: false,
            patch_notes_muted: false,
            patch_notes_seen: None,
            check_for_updates: true,
            update_seen: None,
            backup_keep: default_backup_keep(),
            products: Vec::new(),
        }
    }
}

impl Config {
    /// Config seeded with the official products, for a first run.
    pub fn with_default_products() -> Self {
        Self {
            // The live WoW products, named the way Battle.net names them.
            // Labels stay generic where the content rotates: "Classic" is
            // whatever expansion the progression realms are on.
            products: vec![
                Product::new(ProductCode::Retail, "World of Warcraft"),
                Product::new(ProductCode::ClassicEra, "World of Warcraft Classic"),
                Product::new(ProductCode::Classic, "WoW Classic (progression)"),
                Product::new(ProductCode::ClassicBeta, "World of Warcraft: Forever — Beta"),
                Product::new(ProductCode::Forever, "World of Warcraft: Forever"),
                Product::new(ProductCode::Ptr, "WoW PTR"),
            ],
            ..Self::default()
        }
    }

    /// Read `config.toml`, or fall back to the seeded defaults when it does
    /// not exist yet. A file that exists but does not parse is an error, not a
    /// silent reset, overwriting someone's hand-edited pins is worse than
    /// refusing to start.
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::with_default_products())
            }
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        toml::from_str(s).map_err(ConfigError::Deserialize)
    }

    /// Write `config.toml` atomically: temp file in the same directory, then
    /// rename. A settings screen saves on every change, so a crash mid-write
    /// must not be able to leave a half-written file behind, losing the user's
    /// Proton pins to a truncated table would be worse than losing the edit.
    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(dir).map_err(|source| ConfigError::Io {
            path: dir.to_path_buf(),
            source,
        })?;

        let temp = dir.join(format!(
            ".{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("config.toml")
        ));
        std::fs::write(&temp, text.as_bytes()).map_err(|source| ConfigError::Io {
            path: temp.clone(),
            source,
        })?;
        std::fs::rename(&temp, path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::Serialize)
    }

    pub fn product(&self, code: &ProductCode) -> Option<&Product> {
        self.products.iter().find(|p| &p.code == code)
    }

    pub fn product_mut(&mut self, code: &ProductCode) -> Option<&mut Product> {
        self.products.iter_mut().find(|p| &p.code == code)
    }

    /// Add a product, rejecting a duplicate code.
    pub fn add_product(&mut self, product: Product) -> Result<(), ProductError> {
        if self.product(&product.code).is_some() {
            return Err(ProductError::Duplicate(product.code));
        }
        self.products.push(product);
        Ok(())
    }

    /// The layout this config describes: `blizzard_root` when set, else the
    /// default under the user's home.
    ///
    /// Both front-ends resolve it through here. A `blizzard_root` that only the
    /// CLI honoured would mean the GUI quietly managing a different install
    /// than the one the user configured.
    pub fn layout(&self) -> Result<crate::paths::Layout, crate::paths::PathError> {
        let layout = match &self.blizzard_root {
            Some(root) => crate::paths::Layout::with_root(root),
            None => crate::paths::Layout::default_for_user()?,
        };
        if self.libraries.is_empty() {
            return Ok(layout.with_games(self.games_dir.clone()));
        }
        let mut libraries = crate::library::Libraries::new(self.libraries.clone());
        for product in &self.products {
            if let Some(id) = &product.profile.library {
                libraries.assign(product.code.clone(), id);
            }
        }
        Ok(layout.with_libraries(libraries))
    }

    /// Seed the library list from a config written before multi-drive storage,
    /// returning whether anything changed and the config needs saving.
    ///
    /// The first library keeps the games directory the install already used and
    /// the drive letters it already had, so a 150 GB install Battle.net has
    /// adopted stays recognised. It is *not* claimed here, writing a marker
    /// file is a side effect on the user's disk, and this runs on every start;
    /// [`crate::library::Library::claim`] happens where a library is set up.
    ///
    /// Nor does it need to be saved to be correct: the seeded library's id is
    /// [`crate::library::DEFAULT_ID`], a constant, so the same seed comes back on
    /// the next start whether or not it was written down.
    pub fn ensure_libraries(&mut self) -> Result<bool, crate::paths::PathError> {
        if !self.libraries.is_empty() {
            return Ok(false);
        }
        let games = self.layout()?.games_root();
        // "Home" where it is, which is where the prefix lives and where a
        // first run puts the games; otherwise the directory it is under, which
        // is the only name we have for a hand-pointed `games_dir`.
        let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
        let label = match home.filter(|home| games.starts_with(home)) {
            Some(_) => "Home".to_owned(),
            None => games
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Games".to_owned()),
        };
        let mut main = crate::library::Library::main_at(games, label);
        main.probe();
        self.libraries = vec![main];
        Ok(true)
    }

    /// The library a product is in, or would be installed into.
    pub fn library_of(&self, code: &ProductCode) -> Option<&crate::library::Library> {
        let assigned = self
            .product(code)
            .and_then(|p| p.profile.library.as_deref())
            .and_then(|id| self.libraries.iter().find(|l| l.id == id));
        assigned.or_else(|| self.libraries.iter().find(|l| l.is_default).or(self.libraries.first()))
    }

    /// Where the asset pipeline should read from, if configured.
    ///
    /// Anything carrying a scheme is a base URL; anything else is a path to a
    /// fixture directory. Unset or blank is the built-in source.
    pub fn asset_origin(&self) -> crate::assets::Origin {
        let source = match self.assets_source.as_deref().map(str::trim) {
            Some(source) if !source.is_empty() => source,
            _ => DEFAULT_ASSETS_SOURCE,
        };
        match source.contains("://") {
            true => crate::assets::Origin::Http(source.to_owned()),
            false => crate::assets::Origin::Dir(PathBuf::from(source)),
        }
    }

    /// Where to read the addon catalog from.
    ///
    /// Same shape as [`Config::asset_origin`]: a URL is a proxy, anything else
    /// is a fixture directory, and absent means [`DEFAULT_ADDON_CATALOG`].
    ///
    /// This used to be `Option`, with `None` disabling CurseForge entirely,
    /// written before the proxy existed, and the reason a stock install could
    /// not find a single CurseForge addon.
    pub fn catalog_origin(&self) -> crate::addons::catalog::Origin {
        let source = match self.addon_catalog.as_deref().map(str::trim) {
            Some(source) if !source.is_empty() => source,
            _ => DEFAULT_ADDON_CATALOG,
        };
        match source.contains("://") {
            true => crate::addons::catalog::Origin::Http(source.to_owned()),
            false => crate::addons::catalog::Origin::Dir(PathBuf::from(source)),
        }
    }

    /// Where the News page reads its headlines from.
    ///
    /// The same base as [`Config::catalog_origin`], with no knob of its own:
    /// one proxy serves both namespaces, and a second URL to keep in step
    /// would only be a second thing to point somewhere wrong. A fixture
    /// directory works here too, it reads `news.json` beside the catalog's
    /// fixtures.
    pub fn news_origin(&self) -> crate::news::Origin {
        match self.catalog_origin() {
            crate::addons::catalog::Origin::Http(base) => crate::news::Origin::Http(base),
            crate::addons::catalog::Origin::Dir(dir) => crate::news::Origin::Dir(dir),
        }
    }

    /// The Proton build a product should launch under: its own pin, else the
    /// app-wide default, else `None` (meaning "auto-provision on first run").
    pub fn effective_proton(&self, code: &ProductCode) -> Option<&str> {
        self.product(code)
            .and_then(|p| p.profile.proton_ge_version.as_deref())
            .or_else(|| self.default_proton.version())
    }

    /// The download cap in bytes per second, `0` meaning unlimited.
    ///
    /// A configured `0` is unlimited too rather than a cap of nothing, which
    /// would be a download that never moves.
    pub fn download_limit_bytes(&self) -> u64 {
        self.download_limit_mbps.unwrap_or(0).saturating_mul(BYTES_PER_MB)
    }

    /// Every configured product Lightforge downloads itself.
    ///
    /// The set whose Battle.net auto-update has to stay off: two programs
    /// patching one CASC store is the corruption pre-flight exists to
    /// prevent. A hand-off title is Battle.net's to update and is never here.
    pub fn managed_products(&self) -> Vec<ProductCode> {
        self.products
            .iter()
            .map(|product| product.code.clone())
            .filter(|code| crate::product::Kind::of(code) == crate::product::Kind::Managed)
            .collect()
    }

    /// The Proton build Battle.net and Agent launch under.
    ///
    /// Its own pin rather than the selected product's: the client and Agent
    /// are a different workload from the game, CEF, an updater, a storefront,
    /// and `SPEC.md` notes that "latest" is not always best for them. Falls
    /// back to the app default, so the pin only has to be set when it differs.
    pub fn battle_net_proton_version(&self) -> Option<&str> {
        self.battle_net_proton
            .version()
            .or_else(|| self.default_proton.version())
    }

    /// The Proton build a Battle.net *hand-off* for `code` runs under.
    ///
    /// The product's own pin first, because the thing being started is that
    /// game: a title Battle.net launches is still that title, and a user who
    /// pinned Diablo IV to a build meant it for Diablo IV. Falls through to
    /// Battle.net's pin, which is where a choice about the client itself
    /// belongs, and then to the app default.
    ///
    /// The prefix is shared, so a pin that is *older* than the build which
    /// last ran in it is a downgrade of everything, see
    /// [`crate::proton::prefix_fit`]. Resolved here all the same; refusing a
    /// pin silently would be the worse answer.
    pub fn handoff_proton(&self, code: &ProductCode) -> Option<&str> {
        self.product(code)
            .and_then(|p| p.profile.proton_ge_version.as_deref())
            .or_else(|| self.battle_net_proton_version())
    }

    /// Everything pinned to `version`, named the way the user named it.
    ///
    /// Only *explicit* pins count. A product following the app default is not
    /// listed even though it currently launches under this build: unpinning
    /// the default is one edit that moves all of them, and naming every
    /// follower would make an ordinary default look like a thicket of pins.
    pub fn pinned_to(&self, version: &str) -> Vec<String> {
        let mut users = Vec::new();
        if self.default_proton.version() == Some(version) {
            users.push("the app default".to_owned());
        }
        if self.battle_net_proton.version() == Some(version) {
            users.push("Battle.net".to_owned());
        }
        users.extend(
            self.products
                .iter()
                .filter(|p| p.profile.proton_ge_version.as_deref() == Some(version))
                .map(|p| p.display_name.clone()),
        );
        users
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not parse config.toml")]
    Deserialize(#[source] toml::de::Error),

    #[error("could not serialize config")]
    Serialize(#[source] toml::ser::Error),

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

    // The shared profile is new, so every config on disk predates it. A
    // missing key must default rather than fail the load, or an upgrade
    // bricks the launcher for everyone who already had one.
    #[test]
    fn a_config_without_a_battle_net_profile_still_loads() {
        let config: Config = toml::from_str("install_root = \"/games/Blizzard\"").unwrap();
        assert_eq!(config.battle_net_tweaks, crate::tweaks::Tweaks::default());
        assert!(config.battle_net_tweaks.is_default());
    }

    // Written as a word a person could type, and absent until answered.
    #[test]
    fn the_steamos_answer_is_a_plain_word_and_optional() {
        let config: Config = toml::from_str("install_root = \"/games/Blizzard\"").unwrap();
        assert_eq!(config.steamos_use, None);

        let config = Config { steamos_use: Some(SteamOsUse::Console), ..Config::default() };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("steamos_use = \"console\""), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.steamos_use, Some(SteamOsUse::Console));
    }

    #[test]
    fn the_battle_net_profile_round_trips() {
        let mut config = Config::default();
        config.battle_net_tweaks.gamemode = true;
        config.battle_net_tweaks.wayland = true;
        config.battle_net_tweaks.env.insert("DXVK_HUD".to_owned(), "fps".to_owned());

        let text = toml::to_string(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.battle_net_tweaks, config.battle_net_tweaks);
    }

    #[test]
    fn battle_net_has_its_own_pin_but_falls_back_to_the_default() {
        let mut config = Config::with_default_products();
        config.default_proton = ProtonPin::Pinned("GE-Proton11-7".into());

        // Unset, so it follows the default like everything else.
        assert_eq!(config.battle_net_proton_version(), Some("GE-Proton11-7"));

        // Set, and it wins. That is the entire point of the field.
        config.battle_net_proton = ProtonPin::Pinned("GE-Proton10-34".into());
        assert_eq!(config.battle_net_proton_version(), Some("GE-Proton10-34"));
        // And it does not disturb what the games launch under.
        assert_eq!(config.effective_proton(&ProductCode::Retail), Some("GE-Proton11-7"));
    }

    #[test]
    fn a_hand_off_follows_the_game_it_is_starting() {
        let mut config = Config::with_default_products();
        config.default_proton = ProtonPin::Pinned("GE-Proton11-7".into());
        config.battle_net_proton = ProtonPin::Pinned("GE-Proton11-8".into());

        let diablo = ProductCode::Custom("d4".to_owned());
        config.products.push(Product::new(diablo.clone(), "Diablo IV"));

        // No pin of its own: the client's build, which is the old behaviour.
        assert_eq!(config.handoff_proton(&diablo), Some("GE-Proton11-8"));

        // Pinned, and the hand-off follows the game rather than the client.
        config.product_mut(&diablo).unwrap().profile.proton_ge_version =
            Some("GE-Proton10-34".to_owned());
        assert_eq!(config.handoff_proton(&diablo), Some("GE-Proton10-34"));
        // Another product is untouched by it.
        assert_eq!(config.handoff_proton(&ProductCode::Retail), Some("GE-Proton11-8"));
    }

    #[test]
    fn a_download_cap_is_read_in_megabytes_and_spent_in_bytes() {
        let mut config = Config::default();
        assert_eq!(config.download_limit_bytes(), 0, "no cap by default");

        config.download_limit_mbps = Some(10);
        assert_eq!(config.download_limit_bytes(), 10_000_000);

        // A configured zero is unlimited, not a download that never moves.
        config.download_limit_mbps = Some(0);
        assert_eq!(config.download_limit_bytes(), 0);
    }

    #[test]
    fn pausing_while_playing_is_on_unless_turned_off() {
        // The launched product's own download is paused regardless; this
        // covers the rest of the queue, which shares the same CASC store.
        assert!(Config::default().pause_downloads_while_playing);

        let toml = toml::to_string(&Config::with_default_products()).unwrap();
        let read: Config = toml::from_str(&toml).unwrap();
        assert!(read.pause_downloads_while_playing);
    }

    #[test]
    fn the_launch_window_settings_default_the_way_a_fresh_install_behaves() {
        // A config written before these existed has to keep working, and has
        // to keep behaving the way it did: the launcher stayed put, and a
        // hand-off already put Battle.net out of the way where it could.
        let old: Config = toml::from_str("prefix_user = \"steamuser\"\n").unwrap();
        assert!(!old.minimize_on_launch);
        assert!(old.battle_net_minimized);
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let config = Config::with_default_products();
        let parsed = Config::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn product_pin_wins_over_the_app_default() {
        let mut config = Config::with_default_products();
        config.default_proton = ProtonPin::Pinned("GE-Proton9-20".into());
        config.product_mut(&ProductCode::Classic).unwrap().profile.proton_ge_version =
            Some("GE-Proton8-32".into());

        assert_eq!(config.effective_proton(&ProductCode::Classic), Some("GE-Proton8-32"));
        assert_eq!(config.effective_proton(&ProductCode::Retail), Some("GE-Proton9-20"));
    }

    #[test]
    fn overrides_do_not_leak_between_products() {
        let mut config = Config::with_default_products();
        config.product_mut(&ProductCode::Ptr).unwrap().profile.realmlist_override =
            Some("set realmlist test.example".into());

        assert!(config.product(&ProductCode::Retail).unwrap().profile.realmlist_override.is_none());
    }

    #[test]
    fn a_configured_root_wins_over_the_default() {
        let mut config = Config::default();
        assert!(config.blizzard_root.is_none());

        config.blizzard_root = Some(PathBuf::from("/mnt/games/blizzard"));
        assert_eq!(config.layout().unwrap().root(), PathBuf::from("/mnt/games/blizzard"));
        assert_eq!(
            config.layout().unwrap().games_root(),
            PathBuf::from("/mnt/games/blizzard/games")
        );

        // Games elsewhere; the prefix stays put and links across.
        config.games_dir = Some(PathBuf::from("/mnt/ssd/games"));
        let layout = config.layout().unwrap();
        assert_eq!(
            layout.wow_install(&ProductCode::Retail),
            PathBuf::from("/mnt/ssd/games/World of Warcraft")
        );
        assert!(layout.wow_symlink().starts_with("/mnt/games/blizzard/prefix"));
    }

    /// A config written before multi-drive storage keeps pointing at the same
    /// games folder, on the same drive letters. Anything else would make an
    /// install Battle.net has already adopted stop being recognised.
    #[test]
    fn migrating_a_config_keeps_the_games_folder_and_the_letters() {
        let mut config = Config::with_default_products();
        config.blizzard_root = Some(PathBuf::from("/mnt/games/blizzard"));
        config.games_dir = Some(PathBuf::from("/mnt/ssd/games"));
        let before = config.layout().unwrap().games_root();

        assert!(config.ensure_libraries().unwrap(), "a migration happened");
        assert_eq!(config.libraries.len(), 1);
        let main = &config.libraries[0];
        assert!(main.is_default);
        assert_eq!(main.games(), before);
        assert_eq!(main.wow_letter, crate::library::DEFAULT_WOW_LETTER);
        assert_eq!(main.games_letter, crate::library::DEFAULT_GAMES_LETTER);
        assert_eq!(config.layout().unwrap().games_root(), before);

        // Idempotent: it runs on every start.
        assert!(!config.ensure_libraries().unwrap());
    }

    /// Two libraries, and a product assigned to the second one: every path for
    /// that product resolves there, and every other product stays on main.
    #[test]
    fn a_product_resolves_to_the_library_it_was_installed_in() {
        let mut config = Config::with_default_products();
        config.blizzard_root = Some(PathBuf::from("/home/u/Games/Blizzard"));
        config.ensure_libraries().unwrap();
        let mut second =
            crate::library::Library::on_partition(std::path::Path::new("/mnt/games"), "nvme2", None);
        second.games_letter = "f:".to_owned();
        second.wow_letter = "g:".to_owned();
        let id = second.id.clone();
        config.libraries.push(second);
        config.product_mut(&ProductCode::ClassicEra).unwrap().profile.library = Some(id.clone());

        let layout = config.layout().unwrap();
        assert_eq!(
            layout.wow_install(&ProductCode::ClassicEra),
            PathBuf::from("/mnt/games/Lightforge/games/World of Warcraft"),
        );
        assert_eq!(
            layout.wow_install(&ProductCode::Retail),
            PathBuf::from("/home/u/Games/Blizzard/games/World of Warcraft"),
        );
        // Two installs means two `.build.info` files, each holding only the
        // rows of the products that share it.
        assert_ne!(
            layout.build_info(&ProductCode::ClassicEra),
            layout.build_info(&ProductCode::Retail),
        );
        assert_eq!(config.library_of(&ProductCode::ClassicEra).unwrap().id, id);
        // The prefix does not move with the games.
        assert_eq!(layout.prefix(), PathBuf::from("/home/u/Games/Blizzard/prefix/pfx"));
    }

    /// The default drive decides where *games* go. Battle.net, the Wine
    /// prefix and everything inside it stay at the Blizzard root chosen during
    /// setup, however many drives are added and whichever one is default.
    #[test]
    fn making_another_drive_the_default_does_not_move_battle_net() {
        let mut config = Config::with_default_products();
        config.blizzard_root = Some(PathBuf::from("/home/u/Games/Blizzard"));
        config.ensure_libraries().unwrap();
        let before = config.layout().unwrap();

        let mut second =
            crate::library::Library::on_partition(std::path::Path::new("/mnt/games"), "nvme2", None);
        second.games_letter = "f:".to_owned();
        second.wow_letter = "g:".to_owned();
        let id = second.id.clone();
        config.libraries.push(second);
        for library in &mut config.libraries {
            library.is_default = library.id == id;
        }
        let after = config.layout().unwrap();

        assert_eq!(after.prefix(), before.prefix());
        assert_eq!(after.battle_net_dir(), before.battle_net_dir());
        assert_eq!(after.agent_dir(), before.agent_dir());
        assert_eq!(after.battle_net_config("steamuser"), before.battle_net_config("steamuser"));
        assert!(after.battle_net_dir().starts_with("/home/u/Games/Blizzard"));

        // What *does* move: where a new game lands, and the folder Battle.net
        // proposes for its own titles.
        assert_eq!(after.games_root(), PathBuf::from("/mnt/games/Lightforge/games"));
        let install_path = crate::battlenet::default_tweaks(&after)
            .into_iter()
            .find(|t| t.key() == "Client.Install.DefaultInstallPath")
            .map(|t| t.value);
        assert_eq!(install_path, Some(serde_json::json!(r"F:\")));
    }

    #[test]
    fn saving_and_loading_round_trips_through_a_real_file() {
        let dir = std::env::temp_dir().join("lightforge-config-save");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");

        let mut config = Config::with_default_products();
        config.assets_source = Some("https://lightforge.tuxcore.pro/assets/".into());
        config.default_proton = ProtonPin::Pinned("GE-Proton11-7".into());
        config.save(&path).unwrap();

        assert_eq!(Config::load_or_default(&path).unwrap(), config);
        // No temp file survives a successful save.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
    }

    #[test]
    fn a_missing_config_file_reads_as_the_seeded_defaults() {
        let missing = std::env::temp_dir().join("lightforge-no-such-config.toml");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(Config::load_or_default(&missing).unwrap(), Config::with_default_products());
    }

    /// The regression this default exists for: a stock install, with no
    /// `addon_catalog` line, could not find a single CurseForge addon.
    /// CurseForge needs a key, the key needs a proxy, and `None` meant "no
    /// proxy" rather than "the usual one".
    #[test]
    fn an_unconfigured_install_still_reaches_curseforge() {
        use crate::addons::catalog::Origin as CatalogOrigin;
        let config = Config::default();
        assert!(config.addon_catalog.is_none());
        assert_eq!(
            config.catalog_origin(),
            CatalogOrigin::Http(DEFAULT_ADDON_CATALOG.to_owned())
        );
    }

    #[test]
    fn a_configured_catalog_wins_and_a_path_is_a_fixture_dir() {
        use crate::addons::catalog::Origin as CatalogOrigin;
        let mut config = Config {
            addon_catalog: Some("https://proxy.example/".into()),
            ..Default::default()
        };
        assert_eq!(
            config.catalog_origin(),
            CatalogOrigin::Http("https://proxy.example/".into())
        );

        config.addon_catalog = Some("/tmp/fixtures".into());
        assert_eq!(
            config.catalog_origin(),
            CatalogOrigin::Dir(PathBuf::from("/tmp/fixtures"))
        );

        // Blank is "unset", not "disabled", same rule as assets_source.
        config.addon_catalog = Some("   ".into());
        assert_eq!(
            config.catalog_origin(),
            CatalogOrigin::Http(DEFAULT_ADDON_CATALOG.to_owned())
        );
    }

    #[test]
    fn an_asset_source_is_a_url_or_a_fixture_directory() {
        use crate::assets::Origin;

        let default = Origin::Http(DEFAULT_ASSETS_SOURCE.to_owned());
        let mut config = Config::default();
        assert_eq!(config.asset_origin(), default);

        config.assets_source = Some("https://assets.tuxcore.pro/wow/".into());
        assert!(matches!(config.asset_origin(), Origin::Http(_)));

        config.assets_source = Some("/home/gabriel/lightforge-assets".into());
        assert!(matches!(config.asset_origin(), Origin::Dir(_)));

        // An empty string is a blank field, not a fixture at the current dir.
        config.assets_source = Some("  ".into());
        assert_eq!(config.asset_origin(), default);
    }

    #[test]
    fn duplicate_product_codes_are_rejected() {
        let mut config = Config::with_default_products();
        let dup = Product::new(ProductCode::Retail, "Another Retail");
        assert!(matches!(config.add_product(dup), Err(ProductError::Duplicate(_))));
    }

    #[test]
    fn custom_products_survive_a_round_trip() {
        let mut config = Config::default();
        let code = ProductCode::Custom("custom_wow".into());
        config.add_product(Product::new(code.clone(), "Custom")).unwrap();

        let parsed = Config::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(parsed.product(&code).unwrap().display_name, "Custom");
    }
}
