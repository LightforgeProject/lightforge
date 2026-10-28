//! Persisted application configuration.
//!
//! TOML at `~/.config/lightforge/config.toml`, outside `~/Games/Blizzard` so
//! wiping the game root keeps the pins. Meant to be hand-editable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::product::{Product, ProductCode};
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
    /// Where games were installed before `libraries`; only
    /// [`Config::ensure_libraries`] reads it.
    pub games_dir: Option<PathBuf>,
    /// Every drive Lightforge may install games to, the default one first.
    /// Seeded by [`Config::ensure_libraries`] for older configs.
    #[serde(default)]
    pub libraries: Vec<crate::library::Library>,
    /// Windows user name within the prefix.
    pub prefix_user: String,
    /// Proton build used for the Battle.net/Agent prefix specifically.
    pub battle_net_proton: ProtonPin,
    /// Fallback Proton build for products that don't pin their own.
    pub default_proton: ProtonPin,
    /// Where themed branding is read from: a base URL, or a local directory
    /// for trying out art. Not in the UI; `None` is [`DEFAULT_ASSETS_SOURCE`].
    pub assets_source: Option<String>,
    /// Base URL of the addon catalog proxy, or a local fixture directory.
    /// `None` disables addon management instead of failing at use.
    pub addon_catalog: Option<String>,
    /// Which addon builds to accept for an addon that has no channel of its
    /// own. Stable by default; a per-addon override lives in the ledger.
    #[serde(default)]
    pub addon_channel: crate::addons::Channel,
    /// Whether addons with no choice of their own update in Lightforge's own
    /// pass. On by default.
    #[serde(default = "yes")]
    pub addon_auto_update: bool,
    /// Contact for bug reports, as typed. Kept to prefill the dialog; only
    /// ever sent with a report.
    #[serde(default)]
    pub contact: Option<String>,
    /// Cap on game download speed in MB/s (the progress readout's unit).
    /// `None` is unlimited.
    pub download_limit_mbps: Option<u64>,
    /// Whether launching a game also pauses downloads for *other* products.
    /// The launched product's own is always paused.
    pub pause_downloads_while_playing: bool,
    /// Whether the launcher minimizes once a game is up. Off by default;
    /// nothing is restored when the game exits.
    #[serde(default)]
    pub minimize_on_launch: bool,
    /// Whether Battle.net comes up minimized for a hand-off. On by default,
    /// honoured only when [`crate::battlenet::signs_in_by_itself`].
    #[serde(default = "yes")]
    pub battle_net_minimized: bool,
    /// Whether PLAY closes a running Battle.net first. Off by default, so
    /// friends and chat stay up.
    #[serde(default)]
    pub close_battle_net_on_play: bool,
    /// How Battle.net and every game it launches are started. Shared: a
    /// running client takes launches over IPC, so games cannot differ. Only
    /// the host and runtime half of [`crate::tweaks::Tweaks`] applies.
    #[serde(default)]
    pub battle_net_tweaks: crate::tweaks::Tweaks,
    /// Which of Blizzard's regions products are downloaded from; also written
    /// into `.build.info` and `product.db`.
    #[serde(default)]
    pub region: crate::tact::version::Region,
    /// The language the game is installed in, e.g. `deDE`: it selects which
    /// files are downloaded, so it is asked at setup.
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Whether the first-run wizard has been seen through, "set up later"
    /// included. [`crate::setup::Status`] knows what is missing.
    #[serde(default)]
    pub setup_complete: bool,
    /// Battle.net was declined during setup, which hides its button and the
    /// hand-off titles. Reversible from Settings.
    #[serde(default)]
    pub battle_net_declined: bool,
    /// What a SteamOS machine is used as, asked during setup: nothing on the
    /// machine tells a Deck from a desktop. `None` elsewhere.
    #[serde(default)]
    pub steamos_use: Option<SteamOsUse>,
    /// Whether the release-notes popup is off for good ("never show this
    /// again"). Settings › General turns it back on.
    #[serde(default)]
    pub patch_notes_muted: bool,
    /// The version whose notes have already been shown. `None` shows them.
    #[serde(default)]
    pub patch_notes_seen: Option<String>,
    /// Whether to ask GitHub once per start if a newer Lightforge exists. On
    /// by default, listed in `DISCLAIMER.md`; nothing installs by itself.
    #[serde(default = "yes")]
    pub check_for_updates: bool,
    /// The newest version already mentioned, so the notice opens once per
    /// release, not on every start.
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
/// CurseForge needs a key, so it goes through a proxy; ours by default. The
/// keyless providers never come through here.
pub const DEFAULT_ADDON_CATALOG: &str = "https://lightforge.tuxcore.pro/";

/// Bytes in the megabyte this config counts in.
const BYTES_PER_MB: u64 = 1_000_000;

/// `true`, for a `serde(default)` that is on, not off.
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
            close_battle_net_on_play: false,
            battle_net_tweaks: crate::tweaks::Tweaks::default(),
            region: crate::tact::version::Region::default(),
            locale: default_locale(),
            setup_complete: false,
            battle_net_declined: false,
            steamos_use: None,
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
            // Labels stay generic: "Classic" is whatever expansion is current.
            products: vec![
                Product::new(ProductCode::Retail, "World of Warcraft"),
                Product::new(ProductCode::ClassicEra, "World of Warcraft Classic"),
                Product::new(ProductCode::Classic, "WoW Classic (progression)"),
                Product::new(ProductCode::Forever, "World of Warcraft: Forever"),
                Product::new(ProductCode::Ptr, "WoW PTR"),
            ],
            ..Self::default()
        }
    }

    /// Read `config.toml`, or the seeded defaults when it does not exist. One
    /// that does not parse is an error, not a reset.
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            // An empty file is a save that did not finish, not "all defaults".
            Ok(text) if text.trim().is_empty() => Err(ConfigError::Empty(path.to_path_buf())),
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
        let mut config: Self = toml::from_str(s).map_err(ConfigError::Deserialize)?;
        // Skipped: one bad hand-typed code must not cost every setting.
        config.products.retain(|p| {
            let keep = p.code.is_well_formed();
            if !keep {
                tracing::warn!(
                    "ignoring product `{}` in the config: a code is letters, digits and `_` only; \
                     it goes on the next save",
                    p.code.as_str()
                );
            }
            keep
        });
        Ok(config)
    }

    /// Write `config.toml` atomically: settings save on every change.
    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        crate::fsutil::write_atomic(path, text.as_bytes()).map_err(io(path))
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

    /// The language `code` is downloaded in: its own, else the setup choice.
    pub fn locale_for(&self, code: &ProductCode) -> &str {
        crate::tact::install::current_locale(
            self.product(code)
                .and_then(|p| p.profile.locale.as_deref())
                .unwrap_or(&self.locale),
        )
    }

    /// Add a product, rejecting a duplicate code.
    #[cfg(test)]
    pub fn add_product(&mut self, product: Product) -> Result<(), crate::product::ProductError> {
        if self.product(&product.code).is_some() {
            return Err(crate::product::ProductError::Duplicate(product.code));
        }
        self.products.push(product);
        Ok(())
    }

    /// The layout this config describes: `blizzard_root` when set, else the
    /// default. Both front ends resolve it here.
    pub fn layout(&self) -> Result<crate::paths::Layout, crate::paths::PathError> {
        let layout = match &self.blizzard_root {
            Some(root) => crate::paths::Layout::with_root(root),
            None => crate::paths::Layout::default_for_user()?,
        };
        let mut layout = match self.libraries.is_empty() {
            true => layout.with_games(self.games_dir.clone()),
            false => {
                let mut libraries = crate::library::Libraries::new(self.libraries.clone());
                for product in &self.products {
                    if let Some(id) = &product.profile.library {
                        libraries.assign(product.code.clone(), id);
                    }
                }
                layout.with_libraries(libraries)
            }
        };
        // Recorded at install; for an install from before that, or from
        // Battle.net, the folder whose `.flavor.info` names the game.
        for product in &self.products {
            let profile = &product.profile;
            let dir = profile.install_dir.clone().or_else(|| {
                let install = layout.wow_install(&product.code);
                let found = crate::paths::flavor_dir_name(&install, &product.code);
                install.join(&found).is_dir().then_some(found)
            });
            if let Some(dir) = dir {
                let flavor = crate::paths::Flavor { dir, executable: profile.executable.clone() };
                layout = layout.with_flavor(product.code.clone(), flavor);
            }
        }
        Ok(layout)
    }

    /// Seed the library list from a config written before multi-drive storage,
    /// returning whether anything changed and the config needs saving.
    ///
    /// The first library keeps the existing games directory and drive letters,
    /// so an adopted install stays recognised. Not claimed here (that writes a
    /// marker); its id is [`crate::library::DEFAULT_ID`] either way.
    pub fn ensure_libraries(&mut self) -> Result<bool, crate::paths::PathError> {
        if !self.libraries.is_empty() {
            return Ok(false);
        }
        let games = self.layout()?.games_root();
        // "Home" at the default place, else the parent directory's name.
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
    /// A scheme makes it a base URL, else a fixture directory; blank is the default.
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
    /// Same shape as [`Config::asset_origin`]; absent is [`DEFAULT_ADDON_CATALOG`].
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

    /// The Proton build a product should launch under: its own pin, else the
    /// app-wide default, else `None` (meaning "auto-provision on first run").
    pub fn effective_proton(&self, code: &ProductCode) -> Option<&str> {
        self.product(code)
            .and_then(|p| p.profile.proton_ge_version.as_deref())
            .or_else(|| self.default_proton.version())
    }

    /// The download cap in bytes per second, `0` meaning unlimited.
    ///
    /// A configured `0` is unlimited too.
    pub fn download_limit_bytes(&self) -> u64 {
        self.download_limit_mbps.unwrap_or(0).saturating_mul(BYTES_PER_MB)
    }

    /// Every configured product Lightforge downloads itself.
    ///
    /// Their Battle.net auto-update stays off: two programs must not patch one store.
    pub fn managed_products(&self) -> Vec<ProductCode> {
        self.products
            .iter()
            .map(|product| product.code.clone())
            .filter(|code| crate::product::Kind::of(code) == crate::product::Kind::Managed)
            .collect()
    }

    /// The Proton build Battle.net and Agent launch under.
    ///
    /// Its own pin ("latest" is not always best for CEF and Agent), else the
    /// app default.
    pub fn battle_net_proton_version(&self) -> Option<&str> {
        self.battle_net_proton
            .version()
            .or_else(|| self.default_proton.version())
    }

    /// The Proton build a Battle.net *hand-off* for `code` runs under.
    ///
    /// The product's own pin, then Battle.net's, then the app default. An
    /// older pin downgrades the shared prefix ([`crate::proton::prefix_fit`]);
    /// it is honoured all the same.
    pub fn handoff_proton(&self, code: &ProductCode) -> Option<&str> {
        self.product(code)
            .and_then(|p| p.profile.proton_ge_version.as_deref())
            .or_else(|| self.battle_net_proton_version())
    }

    /// Everything pinned to `version`, named the way the user named it.
    ///
    /// Only explicit pins; products following the default are not listed.
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

    #[error("{0} is empty; move it aside to start over with defaults")]
    Empty(PathBuf),

    #[error("could not serialize config")]
    Serialize(#[source] toml::ser::Error),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConfigError + '_ {
    move |source| ConfigError::Io { path: path.to_path_buf(), source }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_an_error_not_the_defaults() {
        // An empty file parses as every default, and the default library is
        // the real `~/Games/Blizzard`, whatever the file was meant to say.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "\n  \n").unwrap();
        assert!(matches!(Config::load_or_default(&path), Err(ConfigError::Empty(_))));
    }

    #[test]
    fn a_saved_config_loads_back_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config::with_default_products();
        config.save(&path).unwrap();
        config.save(&path).unwrap();

        assert_eq!(Config::load_or_default(&path).unwrap(), config);
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, ["config.toml"]);
    }

    // Older configs lack it; a missing key defaults.
    #[test]
    fn a_config_without_a_battle_net_profile_still_loads() {
        let config: Config = toml::from_str("blizzard_root = \"/games/Blizzard\"").unwrap();
        assert_eq!(config.battle_net_tweaks, crate::tweaks::Tweaks::default());
    }

    // Written as a word a person could type, and absent until answered.
    #[test]
    fn the_steamos_answer_is_a_plain_word_and_optional() {
        let config: Config = toml::from_str("blizzard_root = \"/games/Blizzard\"").unwrap();
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
        config.battle_net_tweaks.mangohud = true;
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

        // Pinned, and the hand-off follows the game, not the client.
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
        // A config from before these keeps its old behaviour.
        let old: Config = toml::from_str("prefix_user = \"steamuser\"\n").unwrap();
        assert!(!old.minimize_on_launch);
        assert!(old.battle_net_minimized);
        // And a config from before the switch leaves the client running.
        assert!(!old.close_battle_net_on_play);
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let config = Config::with_default_products();
        let parsed = Config::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn a_product_code_that_is_not_a_folder_name_is_skipped_not_fatal() {
        let mut config = Config::with_default_products();
        config.products.push(Product::new(ProductCode::Custom("../../outside".into()), "Escape"));
        let parsed = Config::from_toml(&config.to_toml().unwrap()).unwrap();

        assert!(parsed.product(&ProductCode::Custom("../../outside".into())).is_none());
        assert_eq!(parsed.products.len(), config.products.len() - 1, "every other product kept");
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

    /// A config from before multi-drive storage keeps its games folder and
    /// drive letters, so adopted installs stay recognised.
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

    /// The default drive decides where games go; the prefix stays at the
    /// Blizzard root.
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

    /// A stock install, with no `addon_catalog` line, once found no
    /// CurseForge addon at all.
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
        assert!(matches!(config.add_product(dup), Err(crate::product::ProductError::Duplicate(_))));
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
