//! WoW product identity and per-product profile.
//!
//! A "product" is one independently-managed WoW install: Retail, Classic, PTR,
//! or a user-added custom entry. Each carries its own Proton-GE pin, its own
//! realmlist/CDN overrides, and its own prefix state — switching one must never
//! disturb another.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Blizzard's TACT product code (the `--productcode=` argument, and the value
/// in a `.build.info` `Product!` column).
///
/// Official codes are modelled as variants so the common paths are
/// exhaustively matchable; anything else round-trips through
/// [`ProductCode::Custom`] so user-added entries are first-class.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ProductCode {
    /// Retail — `wow`
    Retail,
    /// Classic progression — `wow_classic`
    Classic,
    /// Classic Era — `wow_classic_era`
    ClassicEra,
    /// Retail PTR — `wowt`
    Ptr,
    /// Classic PTR — `wow_classic_ptr`
    ClassicPtr,
    /// Beta — `wow_beta`
    Beta,
    /// Any code Lightforge doesn't model explicitly, including custom entries.
    Custom(String),
}

impl ProductCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Retail => "wow",
            Self::Classic => "wow_classic",
            Self::ClassicEra => "wow_classic_era",
            Self::Ptr => "wowt",
            Self::ClassicPtr => "wow_classic_ptr",
            Self::Beta => "wow_beta",
            Self::Custom(code) => code,
        }
    }

    /// The install subdirectory this product uses beneath the WoW root, e.g.
    /// `_retail_`. Custom products get their code as the directory name.
    pub fn install_dir(&self) -> &str {
        match self {
            Self::Retail => "_retail_",
            Self::Classic => "_classic_",
            Self::ClassicEra => "_classic_era_",
            Self::Ptr => "_ptr_",
            Self::ClassicPtr => "_classic_ptr_",
            Self::Beta => "_beta_",
            Self::Custom(code) => code,
        }
    }

    /// Whether this is an official Blizzard product code.
    pub fn is_official(&self) -> bool {
        !matches!(self, Self::Custom(_))
    }
}

impl FromStr for ProductCode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "wow" => Self::Retail,
            "wow_classic" => Self::Classic,
            "wow_classic_era" => Self::ClassicEra,
            "wowt" => Self::Ptr,
            "wow_classic_ptr" => Self::ClassicPtr,
            "wow_beta" => Self::Beta,
            other => Self::Custom(other.to_owned()),
        })
    }
}

impl From<String> for ProductCode {
    fn from(s: String) -> Self {
        // Avoid re-allocating when the string is already a custom code.
        match s.parse::<Self>() {
            Ok(Self::Custom(_)) => Self::Custom(s),
            Ok(known) => known,
            Err(never) => match never {},
        }
    }
}

impl From<ProductCode> for String {
    fn from(code: ProductCode) -> Self {
        match code {
            ProductCode::Custom(s) => s,
            other => other.as_str().to_owned(),
        }
    }
}

impl fmt::Display for ProductCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A product as Lightforge presents it: identity plus its user-facing label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Product {
    pub code: ProductCode,
    /// Name shown in the UI, e.g. "World of Warcraft" or "Classic Era".
    pub display_name: String,
    pub profile: ProductProfile,
}

impl Product {
    pub fn new(code: ProductCode, display_name: impl Into<String>) -> Self {
        Self {
            code,
            display_name: display_name.into(),
            profile: ProductProfile::default(),
        }
    }
}

/// Everything the user can configure per product.
///
/// `None` on an override means "use the product's stock behavior"; this is the
/// distinction that keeps a custom realmlist from leaking into Retail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProductProfile {
    /// Which Proton-GE build this product launches under. `None` means the
    /// app-wide default applies.
    pub proton_ge_version: Option<String>,
    /// Contents written to the product's `realmlist`/portal config, if set.
    pub realmlist_override: Option<String>,
    /// CDN/patch endpoint override used by the downloader, if set.
    pub cdn_override: Option<String>,
    pub prefix_state: PrefixState,
}

/// Lifecycle of a product's Wine/Proton prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixState {
    /// No prefix has been created yet.
    #[default]
    Absent,
    /// `wineboot -u` ran but automated setup hasn't finished.
    Provisioning,
    /// Prefix is set up and usable.
    Ready,
    /// Setup failed or the prefix was detected as broken; needs recreation.
    Broken,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductError {
    #[error("no product configured with code `{0}`")]
    Unknown(ProductCode),

    #[error("a product with code `{0}` already exists")]
    Duplicate(ProductCode),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_codes_round_trip() {
        for code in [
            "wow",
            "wow_classic",
            "wow_classic_era",
            "wowt",
            "wow_classic_ptr",
            "wow_beta",
        ] {
            let parsed: ProductCode = code.parse().unwrap();
            assert!(parsed.is_official());
            assert_eq!(parsed.as_str(), code);
        }
    }

    #[test]
    fn unknown_code_becomes_custom() {
        let parsed: ProductCode = "turtle_wow".parse().unwrap();
        assert_eq!(parsed, ProductCode::Custom("turtle_wow".into()));
        assert!(!parsed.is_official());
        assert_eq!(parsed.install_dir(), "turtle_wow");
    }
}
