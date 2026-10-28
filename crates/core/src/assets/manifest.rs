use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::pipeline::join;
use super::{AssetError, SCHEMA_VERSION};
use crate::product::{Branch, Catalog, CatalogEntry, ProductCode};

/// The themed branding document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// Product code whose theme covers anything `products` doesn't name,
    /// PTR, beta, and user-added custom products.
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub products: BTreeMap<String, Theme>,
}

/// One product's branding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    /// Stable display name, `Retail`, never `Midnight`.
    pub label: String,
    /// Which game this product is a build of, `retail`, `classic`,
    /// `classic_era`, `forever`, or a name of your own. Optional: lets a
    /// product newer than the binary land in the right tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Which build of that line, `live`, `ptr`, `ptr2` or `beta`. Optional;
    /// anything else is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Optional since schema 2; without it the label is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<AssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<AssetRef>,
    /// Small square mark for the product bar. Optional: the bar falls back to
    /// [`ProductCode::short_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<AssetRef>,
}

/// One image: where it lives, optionally what it must hash to, and optionally
/// how tall to draw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRef {
    /// Absolute, or relative to the manifest's own location.
    pub url: String,
    /// Lowercase hex SHA-256 of the image bytes. Optional, but when present a
    /// mismatch discards rather than caches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Logical height to draw a [`Slot::Logo`] at, in pixels: a wide wordmark
    /// at a square one's height looks twice the size. Absent: the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

/// Which of a theme's two images, for error messages and cache naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Logo,
    Background,
    Icon,
}

impl Slot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Background => "background",
            Self::Icon => "icon",
        }
    }
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Manifest {
    /// Parse and check the schema version in one step.
    pub fn parse(bytes: &[u8]) -> Result<Self, AssetError> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(AssetError::Parse)?;
        if manifest.schema > SCHEMA_VERSION {
            return Err(AssetError::UnsupportedSchema {
                found: manifest.schema,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(manifest)
    }

    /// This product's own entry, with no fallback: right for a *mark*, which
    /// is identity (WoW's icon must not appear beside "Hearthstone").
    pub fn own_theme(&self, code: &ProductCode) -> Option<&Theme> {
        self.products.get(code.as_str())
    }

    /// The theme `default` names, when it names one that exists.
    fn default_theme(&self) -> Option<&Theme> {
        self.default.as_deref().and_then(|code| self.products.get(code))
    }

    /// The theme of this product's *line*: Classic's for Classic PTR.
    ///
    /// Between a product's own entry and `default`. For a head the manifest
    /// does not name, the first other branch with a theme: Forever's art stays
    /// on `wow_classic_beta`, since naming `wowf` breaks 1.0.0-beta.3.
    fn head_theme(&self, code: &ProductCode) -> Option<&Theme> {
        let line = code.line();
        let head = line.head();
        let head_theme = match head == *code {
            true => None,
            false => self.products.get(head.as_str()),
        };
        head_theme.or_else(|| {
            line.branches()
                .iter()
                .filter(|branch| *branch != code)
                .find_map(|branch| self.products.get(branch.as_str()))
        })
    }

    /// One art slot for a product: its own, else its line's, else the default
    /// theme's.
    ///
    /// Per slot, not per theme: an entry with only a mark still inherits the
    /// logo and background.
    pub fn slot(
        &self,
        code: &ProductCode,
        pick: fn(&Theme) -> Option<&AssetRef>,
    ) -> Option<&AssetRef> {
        self.own_theme(code)
            .and_then(pick)
            .or_else(|| self.head_theme(code).and_then(pick))
            .or_else(|| self.default_theme().and_then(pick))
    }

    /// A product's theme, down the same chain [`Manifest::slot`] walks.
    pub fn theme(&self, code: &ProductCode) -> Option<&Theme> {
        self.own_theme(code)
            .or_else(|| self.head_theme(code))
            .or_else(|| self.default_theme())
    }
}

impl Manifest {
    /// Every asset URL this manifest names, as the client would resolve them.
    ///
    /// `base` is the manifest's own location.
    pub fn urls(&self, base: &str) -> Vec<String> {
        let mut out = Vec::with_capacity(self.products.len() * 3);
        for theme in self.products.values() {
            for asset in [theme.logo.as_ref(), theme.background.as_ref(), theme.icon.as_ref()]
                .into_iter()
                .flatten()
            {
                out.push(join(base, &asset.url));
            }
        }
        out
    }

    /// The product catalog this manifest states, over the built-in one.
    ///
    /// Only identity, grouping and label; where files go stays derived from
    /// the code, so a bad publish can mislabel but never misinstall.
    pub fn catalog(&self) -> Catalog {
        // A published code is a folder name on every machine that reads it.
        let published = self.products.iter().filter(|(code, _)| {
            let keep = crate::product::is_well_formed(code);
            if !keep {
                tracing::warn!("ignoring product `{code}` in the manifest: not a usable product code");
            }
            keep
        });
        Catalog::with(published.map(|(code, theme)| {
            let code = ProductCode::from(code.clone());
            CatalogEntry {
                line: match theme.line.as_deref() {
                    Some(line) => line.parse().unwrap_or_else(|never| match never {}),
                    None => code.line(),
                },
                // An unparseable branch falls back; the entry stays.
                branch: theme
                    .branch
                    .as_deref()
                    .and_then(|b| b.parse::<Branch>().ok())
                    .unwrap_or_else(|| code.branch()),
                kind: crate::product::Kind::of(&code),
                label: theme.label.clone(),
                code,
            }
        }))
    }
}
