use std::path::Path;

use super::wire::{Fields, Wire};
use super::{io, ProductDbError};

/// One product Agent reports as installed: enough to name it, find its files
/// and say what build it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The client's install key, field 1, e.g. `hs_beta`: Battle.net's name,
    /// **not always the CDN's** ([`Installed::product_code`]).
    pub product: String,
    /// The TACT product code, field 2, e.g. `hsb`. Equal to
    /// [`Installed::product`] except for Hearthstone, so a match against the
    /// catalog (keyed by CDN name) must accept either. Empty when omitted.
    pub product_code: String,
    /// Windows-visible install root, e.g. `C:/Program Files (x86)/Hearthstone`.
    pub install_path: String,
    /// Installed version, when the record carries one.
    pub version: Option<String>,
    /// Build config hash of what is on disk: the installed key (field 14),
    /// else the active one (12). The only record of a pre-installed build,
    /// which Agent keeps here after taking its `.build.info` row away.
    pub build_key: Option<String>,
    /// Product family, e.g. `wow` for every WoW flavour. Often empty
    /// (Hearthstone).
    pub family: String,
}

impl Installed {
    /// Battle.net's own plumbing, not a game someone plays.
    pub fn is_infrastructure(&self) -> bool {
        matches!(self.product.as_str(), "agent" | "bna" | "battle.net")
    }
}

/// Read Agent's `product.db`: what Battle.net has installed, and where. The
/// counterpart to [`encode`](super::encode), held to it by a round-trip test.
///
/// Tolerant: unknown fields are skipped, missing ones default, and malformed
/// bytes end the scan. Agent updates on its own schedule.
pub fn decode(bytes: &[u8]) -> Vec<Installed> {
    let mut found = Vec::new();
    for (field, value) in Fields::new(bytes) {
        if field == 1 {
            if let Wire::Bytes(record) = value {
                if let Some(install) = decode_install(record) {
                    found.push(install);
                }
            }
        }
    }
    found
}

/// Read `product.db` out of Agent's data directory.
pub fn read(path: &Path) -> Result<Vec<Installed>, ProductDbError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    Ok(decode(&bytes))
}

pub(super) fn decode_install(bytes: &[u8]) -> Option<Installed> {
    let mut product = String::new();
    let mut product_code = String::new();
    let mut install_path = String::new();
    let mut family = String::new();
    let mut version = None;
    let (mut active, mut installed) = (None, None);

    for (field, value) in Fields::new(bytes) {
        match (field, value) {
            (1, Wire::Bytes(b)) => product = string(b),
            (2, Wire::Bytes(b)) => product_code = string(b),
            (3, Wire::Bytes(settings)) => {
                for (field, value) in Fields::new(settings) {
                    if let (1, Wire::Bytes(b)) = (field, value) {
                        install_path = string(b);
                    }
                }
            }
            (4, Wire::Bytes(cached)) => {
                for (field, value) in Fields::new(cached) {
                    if let (1, Wire::Bytes(base)) = (field, value) {
                        for (field, value) in Fields::new(base) {
                            let text = |b| Some(string(b)).filter(|v: &String| !v.is_empty());
                            match (field, value) {
                                (7, Wire::Bytes(b)) => version = text(b),
                                (12, Wire::Bytes(b)) => active = text(b),
                                (14, Wire::Bytes(b)) => installed = text(b),
                                _ => {}
                            }
                        }
                    }
                }
            }
            (6, Wire::Bytes(b)) => family = string(b),
            _ => {}
        }
    }

    // No code or no path: nothing to draw.
    (!product.is_empty() && !install_path.is_empty()).then_some(Installed {
        product,
        product_code,
        install_path,
        version,
        build_key: installed.or(active),
        family,
    })
}

fn string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
