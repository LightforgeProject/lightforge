//! Which *game family* a product belongs to, read from Blizzard's own data.
//!
//! The client works in game families, one per tile in its library — `WTCG`,
//! `Fen`, `Pro` — and `--exec="launch <uid>"` wants that, not the TACT product
//! code. Getting it wrong is not a no-op: the client falls back to the first
//! family it knows, which is Warcraft III, and the user lands on a game they
//! may not even own. That is why [`crate::battlenet::game_uid`] only ever
//! listed families somebody had seen in a log.
//!
//! The client caches Blizzard's published catalogue in its own
//! content-addressed store, as JSON fragments, and the chain is all there:
//!
//! ```text
//! products[].id                         WTCG      the game family
//!   └ base.types.<type>.uid             hs_beta   the install key
//!       └ installs[<uid>].tact_product  hsb       the TACT product code
//! ```
//!
//! So a product's family is a lookup rather than a table to maintain, and it
//! covers every title Blizzard publishes rather than the two that happened to
//! be observed. Nothing here is written, requested or decoded out of a running
//! process: these are files the client left on disk, parsed as the JSON they
//! are.
//!
//! The cache is Blizzard's data in Lightforge's hands, so it is read the way
//! any foreign input is — a fragment that will not parse, or that has a shape
//! this does not recognise, is skipped rather than trusted or fatal. An empty
//! result is the normal state of a prefix whose client has never run, and the
//! caller falls back to the families that are compiled in.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::paths::Layout;

/// Largest cache fragment worth parsing.
///
/// The catalogue fragments are kilobytes; the same directory also holds the
/// artwork they refer to, which is most of its 70-odd megabytes. The first
/// byte already rules those out, and this rules out anything pathological.
const MAX_FRAGMENT: u64 = 4 * 1024 * 1024;

/// Game families, by every product code that names one.
///
/// Keyed by both the install key (`hs_beta`) and the TACT product code
/// (`hsb`), because a product is known by the first in Battle.net's registry
/// and by the second in ours, and either may be what a caller holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Families {
    by_code: BTreeMap<String, String>,
    /// The client's own install key for a product, which is what its install
    /// flow is addressed by and is not always the TACT code: Hearthstone is
    /// `hsb` to the CDN and `hs_beta` to Battle.net.
    uids: BTreeMap<String, String>,
}

impl Families {
    /// Read every catalogue fragment the client has cached.
    ///
    /// Cheap despite the size of the directory: only files that begin with
    /// `{` are read past their first byte, which on a real prefix is about
    /// three megabytes out of seventy.
    pub fn read(layout: &Layout, user: &str) -> Self {
        Self::read_dir(&layout.battle_net_cache(user))
    }

    fn read_dir(cache: &Path) -> Self {
        let mut uid_to_family: BTreeMap<String, String> = BTreeMap::new();
        let mut uid_to_tact: BTreeMap<String, String> = BTreeMap::new();

        for path in fragments(cache) {
            let Some(fragment) = parse(&path) else { continue };

            for product in fragment.products {
                let Some(family) = product.id else { continue };
                for entry in product.base.types.into_values() {
                    if let Some(uid) = entry.uid {
                        uid_to_family.insert(uid, family.clone());
                    }
                }
            }
            for (uid, install) in fragment.installs {
                if let Some(tact) = install.tact_product {
                    uid_to_tact.insert(uid, tact);
                }
            }
        }

        // Both spellings point at the same family. The install key is what
        // Battle.net's registry calls a product; the TACT code is what the
        // CDN and Lightforge call it, and for most titles they differ.
        let mut by_code = BTreeMap::new();
        let mut uids = BTreeMap::new();
        for (uid, family) in uid_to_family {
            if let Some(tact) = uid_to_tact.get(&uid) {
                by_code.insert(tact.to_ascii_lowercase(), family.clone());
                uids.insert(tact.to_ascii_lowercase(), uid.clone());
            }
            uids.insert(uid.to_ascii_lowercase(), uid.clone());
            by_code.insert(uid.to_ascii_lowercase(), family);
        }
        Self { by_code, uids }
    }

    /// The family a product code belongs to, if the catalogue names one.
    pub fn family_of(&self, code: &str) -> Option<&str> {
        self.by_code.get(&code.to_ascii_lowercase()).map(String::as_str)
    }

    /// The client's install key for a product code — what its install flow
    /// is addressed by, as opposed to the family its tile belongs to.
    pub fn install_uid(&self, code: &str) -> Option<&str> {
        self.uids.get(&code.to_ascii_lowercase()).map(String::as_str)
    }

    /// How many product codes resolve. Zero means a cache that has never been
    /// filled, which is a reason to fall back rather than a failure.
    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }
}

/// Cache files that could be a catalogue fragment.
///
/// The store is two levels of hex directories; everything in it is named
/// after its own content hash, so the only way to tell JSON from a PNG is to
/// look at the first byte.
fn fragments(cache: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(outer) = std::fs::read_dir(cache) else {
        return found;
    };
    for first in outer.flatten() {
        let Ok(inner) = std::fs::read_dir(first.path()) else { continue };
        for second in inner.flatten() {
            let Ok(files) = std::fs::read_dir(second.path()) else { continue };
            for file in files.flatten() {
                let Ok(meta) = file.metadata() else { continue };
                if !meta.is_file() || meta.len() > MAX_FRAGMENT {
                    continue;
                }
                if starts_with_brace(&file.path()) {
                    found.push(file.path());
                }
            }
        }
    }
    found
}

fn starts_with_brace(path: &Path) -> bool {
    use std::io::Read;

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut first = [0u8; 1];
    matches!(file.read_exact(&mut first), Ok(())) && first[0] == b'{'
}

fn parse(path: &Path) -> Option<Fragment> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// One catalogue fragment, reduced to the two parts that answer the question.
///
/// Everything else the client caches — artwork ids, storefront copy, regional
/// permissions — is left unparsed on purpose: a shape we do not read cannot
/// break when Blizzard changes it.
#[derive(Debug, Deserialize)]
struct Fragment {
    #[serde(default)]
    products: Vec<Product>,
    #[serde(default)]
    installs: BTreeMap<String, Install>,
}

#[derive(Debug, Deserialize)]
struct Product {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    base: Base,
}

#[derive(Debug, Default, Deserialize)]
struct Base {
    /// `retail`, `ptr`, `beta`, and whatever else a title ships. All of them
    /// belong to the same family, which is the only thing being asked.
    #[serde(default)]
    types: BTreeMap<String, TypeEntry>,
}

#[derive(Debug, Deserialize)]
struct TypeEntry {
    #[serde(default)]
    uid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Install {
    #[serde(default)]
    tact_product: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(cache: &Path, name: &str, body: &str) {
        let dir = cache.join(&name[0..2]).join(&name[2..4]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-families-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_product_resolves_by_both_the_names_it_is_known_by() {
        let cache = scratch("both-names");
        // The shape the client really writes, split across fragments the way
        // it really splits it: the family and the install config do not have
        // to arrive in the same file.
        write(
            &cache,
            "aa11000000000000000000000000000a",
            r#"{"products":[{"id":"WTCG","base":{"types":{"retail":{"uid":"hs_beta"}}}}]}"#,
        );
        write(
            &cache,
            "bb22000000000000000000000000000b",
            r#"{"installs":{"hs_beta":{"tact_product":"hsb","requires_sso_token":true}}}"#,
        );

        let families = Families::read_dir(&cache);
        // Battle.net's registry calls it hs_beta, the CDN calls it hsb, and
        // a caller may hold either.
        assert_eq!(families.family_of("hs_beta"), Some("WTCG"));
        assert_eq!(families.family_of("hsb"), Some("WTCG"));
        assert_eq!(families.family_of("HSB"), Some("WTCG"), "codes are not case");
        assert_eq!(families.family_of("nonesuch"), None);

        // And the install key, which the install flow is addressed by. The
        // CDN's name for the product is not the client's.
        assert_eq!(families.install_uid("hsb"), Some("hs_beta"));
        assert_eq!(families.install_uid("hs_beta"), Some("hs_beta"));
        assert_eq!(families.install_uid("nonesuch"), None);

        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn every_type_of_a_title_belongs_to_the_same_family() {
        let cache = scratch("types");
        write(
            &cache,
            "cc33000000000000000000000000000c",
            r#"{"products":[{"id":"Fen","base":{"types":{
                 "retail":{"uid":"fenris"},
                 "ptr":{"uid":"fenris_ptr"}}}}],
               "installs":{
                 "fenris":{"tact_product":"fenris"},
                 "fenris_ptr":{"tact_product":"fenrisb"}}}"#,
        );

        let families = Families::read_dir(&cache);
        assert_eq!(families.family_of("fenris"), Some("Fen"));
        assert_eq!(families.family_of("fenrisb"), Some("Fen"), "a PTR is the same tile");

        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn a_cache_of_junk_answers_nothing_rather_than_failing() {
        let cache = scratch("junk");
        // Artwork, which is most of what the store holds.
        write(&cache, "dd44000000000000000000000000000d", "\u{89}PNG\r\n\u{1a}\n");
        // JSON that is not a catalogue fragment.
        write(&cache, "ee55000000000000000000000000000e", r#"{"keys":[{"kty":"RSA"}]}"#);
        // A fragment that was being written when we read it.
        write(&cache, "ff66000000000000000000000000000f", r#"{"products":[{"id":"WoW","#);

        let families = Families::read_dir(&cache);
        assert!(families.is_empty());

        // And a prefix whose client has never run.
        assert!(Families::read_dir(&cache.join("nothing-here")).is_empty());

        let _ = std::fs::remove_dir_all(&cache);
    }
}




