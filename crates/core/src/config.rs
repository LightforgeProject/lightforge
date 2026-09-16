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
            products: Vec::new(),
        }
    }
}

impl Config {
    /// Config seeded with the official products, for a first run.
    pub fn with_default_products() -> Self {
        Self {
            products: vec![
                Product::new(ProductCode::Retail, "World of Warcraft"),
                Product::new(ProductCode::Classic, "WoW Classic"),
                Product::new(ProductCode::Ptr, "WoW PTR"),
            ],
            ..Self::default()
        }
    }

    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        toml::from_str(s).map_err(ConfigError::Deserialize)
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
