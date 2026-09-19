//! Persisted application configuration.
//!
//! TOML at `~/.config/lightforge/config.toml`, deliberately outside
//! `~/Games/Blizzard` so wiping the game root never destroys the user's
//! per-product pins and overrides. The format is hand-editable on purpose —
//! realmlist and CDN overrides are exactly the fields people poke manually.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::product::{Product, ProductCode, ProductError};
use crate::proton::ProtonPin;

/// Windows user name inside a Proton prefix.
const DEFAULT_PREFIX_USER: &str = "steamuser";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Root of the dual-directory layout. Defaults to `~/Games/Blizzard`.
    pub blizzard_root: Option<PathBuf>,
    /// Windows user name within the prefix.
    pub prefix_user: String,
    /// Proton build used for the Battle.net/Agent prefix specifically.
    pub battle_net_proton: ProtonPin,
    /// Fallback Proton build for products that don't pin their own.
    pub default_proton: ProtonPin,
    /// Where themed branding is read from: a base URL, or a local fixture
    /// directory while the Tuxcore endpoint is not live yet. `None` means the
    /// UI uses its built-in art and never touches the network for theming.
    pub assets_source: Option<String>,
    /// Base URL of the addon catalog proxy, or a local fixture directory.
    /// `None` disables addon management rather than failing at use.
    pub addon_catalog: Option<String>,
    /// Every product Lightforge manages, official and custom alike.
    pub products: Vec<Product>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            blizzard_root: None,
            prefix_user: DEFAULT_PREFIX_USER.to_owned(),
            battle_net_proton: ProtonPin::default(),
            default_proton: ProtonPin::default(),
            assets_source: None,
            addon_catalog: None,
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
                Product::new(ProductCode::Ptr, "WoW PTR"),
            ],
            ..Self::default()
        }
    }

    /// Read `config.toml`, or fall back to the seeded defaults when it does
    /// not exist yet. A file that exists but does not parse is an error, not a
    /// silent reset — overwriting someone's hand-edited pins is worse than
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
    /// must not be able to leave a half-written file behind — losing the user's
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
        match &self.blizzard_root {
            Some(root) => Ok(crate::paths::Layout::with_root(root)),
            None => crate::paths::Layout::default_for_user(),
        }
    }

    /// Where the asset pipeline should read from, if configured.
    ///
    /// Anything carrying a scheme is a base URL; anything else is a path to a
    /// fixture directory, so switching to the live endpoint is a one-line
    /// config edit and no code change.
    pub fn asset_origin(&self) -> Option<crate::assets::Origin> {
        let source = self.assets_source.as_deref()?.trim();
        if source.is_empty() {
            return None;
        }
        Some(match source.contains("://") {
            true => crate::assets::Origin::Http(source.to_owned()),
            false => crate::assets::Origin::Dir(PathBuf::from(source)),
        })
    }

    /// The Proton build a product should launch under: its own pin, else the
    /// app-wide default, else `None` (meaning "auto-provision on first run").
    pub fn effective_proton(&self, code: &ProductCode) -> Option<&str> {
        self.product(code)
            .and_then(|p| p.profile.proton_ge_version.as_deref())
            .or_else(|| self.default_proton.version())
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

    #[test]
    fn an_asset_source_is_a_url_or_a_fixture_directory() {
        use crate::assets::Origin;

        let mut config = Config::default();
        assert!(config.asset_origin().is_none());

        config.assets_source = Some("https://assets.tuxcore.pro/wow/".into());
        assert!(matches!(config.asset_origin(), Some(Origin::Http(_))));

        config.assets_source = Some("/home/gabriel/lightforge-assets".into());
        assert!(matches!(config.asset_origin(), Some(Origin::Dir(_))));

        // An empty string is a blank field, not a fixture at the current dir.
        config.assets_source = Some("  ".into());
        assert!(config.asset_origin().is_none());
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
