use std::path::Path;

use super::wire::{Fields, Wire};
use super::ProductDbError;

/// One product Agent reports as installed, as far as Lightforge cares.
///
/// A deliberately small slice of the record: enough to name a product, find
/// its files and say what it is on. Everything else in there describes an
/// install Lightforge does not manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The client's install key, field 1, e.g. `hs_beta`.
    ///
    /// This is what Battle.net calls the product, which is **not always what
    /// the CDN calls it**: see [`Installed::product_code`].
    pub product: String,
    /// The TACT product code, field 2, e.g. `hsb`.
    ///
    /// The two are the same string for every product but Hearthstone, which
    /// Battle.net records as `hs_beta` and Blizzard's CDN publishes as `hsb`.
    /// Lightforge's catalog is keyed by the CDN's name, so a caller matching
    /// a tile against this file has to accept either, matching only on
    /// [`Installed::product`] leaves Hearthstone reading as "not installed"
    /// however many times someone installs it. Empty on a record that omits
    /// the field rather than assumed equal to `product`.
    pub product_code: String,
    /// Windows-visible install root, e.g. `C:/Program Files (x86)/Hearthstone`.
    pub install_path: String,
    /// Installed version, when the record carries one.
    pub version: Option<String>,
    /// Build config hash of what is on disk: the installed key (field 14),
    /// else the active one (12). The only record of a pre-installed build,
    /// which Agent keeps here after taking its `.build.info` row away.
    pub build_key: Option<String>,
    /// Product family, e.g. `wow` for every WoW flavour. Empty when absent,
    /// which is the normal state for a single-title game like Hearthstone.
    pub family: String,
}

impl Installed {
    /// Battle.net's own plumbing rather than a game someone plays.
    pub fn is_infrastructure(&self) -> bool {
        matches!(self.product.as_str(), "agent" | "bna" | "battle.net")
    }
}

/// Read Agent's `product.db`: what Battle.net has installed, and where.
///
/// The counterpart to [`encode`], against the same layout documented at the
/// top of this module. Written separately rather than derived from a schema
/// because there is no published schema; the two directions read the same
/// field numbers and are kept honest by a round-trip test.
///
/// Tolerant by design. Unknown fields are skipped, a record missing a field
/// yields a default for it, and malformed bytes end the scan rather than
/// failing the file: this is someone else's file, written by a program that
/// updates on its own schedule, and a launcher that refuses to start because
/// Agent added a field is worse than one that shows fewer tiles.
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
    let bytes = std::fs::read(path).map_err(|source| ProductDbError::Io {
        path: path.to_path_buf(),
        source,
    })?;
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

    // A record with no code names nothing; a record with no path points
    // nowhere. Either way there is no tile to draw.
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
