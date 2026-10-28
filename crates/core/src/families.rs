//! Which *game family* a product belongs to, read from Blizzard's own data.
//!
//! `--exec="launch <uid>"` wants the family (`WTCG`, `Fen`, `Pro`), one per
//! tile, not the TACT code; a wrong one lands on Warcraft III. The client
//! caches Blizzard's catalogue as JSON fragments, and the chain is there:
//!
//! ```text
//! products[].id                         WTCG      the game family
//!   └ base.types.<type>.uid             hs_beta   the install key
//!       └ installs[<uid>].tact_product  hsb       the TACT product code
//! ```
//!
//! Files the client left on disk, read as foreign input: an unparsable or
//! unfamiliar fragment is skipped. Empty means the client never ran, and the
//! caller falls back to compiled-in families.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::paths::Layout;
use crate::tact::version::Region;

/// Largest cache fragment worth parsing.
///
/// Fragments are kilobytes; the directory's ~70 MB is mostly artwork.
const MAX_FRAGMENT: u64 = 4 * 1024 * 1024;

/// Game families, by every product code that names one.
///
/// Keyed by both the install key (`hs_beta`) and the TACT code (`hsb`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Families {
    by_code: BTreeMap<String, String>,
    /// The client's install key per product, what its install flow takes.
    uids: BTreeMap<String, String>,
    /// Battle.net's selector name for a product, by install key, already
    /// resolved through [`Fragment::strings`]: locale to text.
    names: BTreeMap<String, Localized>,
    /// The release-date line of a product Battle.net offers as a pre-install,
    /// by install key. Only a preload fragment carries one.
    release_dates: BTreeMap<String, Localized>,
}

/// One catalogue string in every locale the client cached, `default` among
/// them.
type Localized = BTreeMap<String, String>;

impl Families {
    /// Read every catalogue fragment the client has cached.
    ///
    /// Only files starting with `{` are read past their first byte (about 3 of
    /// 70 MB). `region` is the login region some fragments depend on.
    pub fn read(layout: &Layout, user: &str, region: Region) -> Self {
        Self::read_dir(&layout.battle_net_cache(user), region)
    }

    fn read_dir(cache: &Path, region: Region) -> Self {
        let mut uid_to_family: BTreeMap<String, String> = BTreeMap::new();
        let mut uid_to_tact: BTreeMap<String, String> = BTreeMap::new();
        // String ids, which can live in another fragment.
        let mut name_ids: BTreeMap<String, String> = BTreeMap::new();
        let mut release_ids: BTreeMap<String, String> = BTreeMap::new();
        let mut strings: BTreeMap<String, Localized> = BTreeMap::new();

        let fragments: Vec<(std::path::PathBuf, Fragment)> = fragments(cache)
            .into_iter()
            .filter_map(|path| Some((path.clone(), parse(&path)?)))
            .collect();
        let current = current(&fragments, region);

        for (path, fragment) in fragments {
            // The family is the same in every revision ever cached, so any
            // will do. Names and dates are not: see [`current`].
            let in_effect = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|hash| current.contains(hash));

            for product in fragment.products {
                let Some(family) = product.id else { continue };
                for (key, entry) in product.base.types {
                    // A preload fragment has no uid; its type key is the
                    // install key, but `retail` is one too, so no family.
                    let named = entry.uid.clone().unwrap_or(key);
                    if let Some(name) = entry.selector_display_name.or(entry.name).filter(|_| in_effect) {
                        name_ids.insert(named.clone(), name);
                    }
                    if let Some(release) = entry.release_date_message.filter(|_| in_effect) {
                        release_ids.insert(named, release);
                    }
                    if let Some(uid) = entry.uid {
                        uid_to_family.insert(uid, family.clone());
                    }
                }
            }
            for (locale, table) in fragment.strings.into_iter().filter(|_| in_effect) {
                for (id, text) in table {
                    strings.entry(id).or_default().insert(locale.clone(), text);
                }
            }
            for (uid, install) in fragment.installs {
                if let Some(tact) = install.tact_product {
                    uid_to_tact.insert(uid, tact);
                }
            }
        }

        // Install key and TACT code both point at the family.
        let mut by_code = BTreeMap::new();
        let mut uids = BTreeMap::new();
        // Only referenced ids. Cloned: two types can share one.
        let resolve = |ids: BTreeMap<String, String>| -> BTreeMap<String, Localized> {
            ids.into_iter()
                .filter_map(|(uid, id)| Some((uid.to_ascii_lowercase(), strings.get(&id)?.clone())))
                .collect()
        };
        let names = resolve(name_ids);
        let release_dates = resolve(release_ids);
        for (uid, family) in uid_to_family {
            if let Some(tact) = uid_to_tact.get(&uid) {
                by_code.insert(tact.to_ascii_lowercase(), family.clone());
                uids.insert(tact.to_ascii_lowercase(), uid.clone());
            }
            uids.insert(uid.to_ascii_lowercase(), uid.clone());
            by_code.insert(uid.to_ascii_lowercase(), family);
        }
        Self { by_code, uids, names, release_dates }
    }

    /// The family a product code belongs to, if the catalogue names one.
    pub fn family_of(&self, code: &str) -> Option<&str> {
        self.by_code.get(&code.to_ascii_lowercase()).map(String::as_str)
    }

    /// The client's install key for a product code, what its install flow
    /// is addressed by, as opposed to the family its tile belongs to.
    pub fn install_uid(&self, code: &str) -> Option<&str> {
        self.uids.get(&code.to_ascii_lowercase()).map(String::as_str)
    }

    /// The version part of Battle.net's name for a product: `Stress Test` out
    /// of `WoW: Forever - Stress Test`.
    ///
    /// Only for *game - version* names: no expansion names in the picker.
    pub fn version_name(&self, code: &str, locale: &str) -> Option<&str> {
        let name = localized(self.names.get(&self.uid_key(code))?, locale)?;
        // `deDE` writes an en dash where `default` has a hyphen.
        [" - ", " \u{2013} "]
            .iter()
            .find_map(|dash| name.rsplit_once(dash))
            .map(|(_, version)| version.trim())
            .filter(|version| !version.is_empty())
    }

    /// When a pre-installed product becomes playable, as Battle.net words it:
    /// `Available October 19, 2026`.
    ///
    /// A caption, never a gate: the cache can be stale on release day.
    pub fn release_date(&self, code: &str, locale: &str) -> Option<&str> {
        localized(self.release_dates.get(&self.uid_key(code))?, locale)
    }

    fn uid_key(&self, code: &str) -> String {
        let code = code.to_ascii_lowercase();
        self.uids.get(&code).map(|uid| uid.to_ascii_lowercase()).unwrap_or(code)
    }

    /// How many product codes resolve; zero means fall back.
    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }
}

/// Cache files that could be a catalogue fragment.
///
/// Content-hash names, so the first byte tells JSON from a PNG.
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

/// The fragments Battle.net would show right now, by hash.
///
/// The newest root fragment says which revision is current. A conditional
/// fragment counts only when decidable here and true; a guessed account
/// condition once showed "Available November 4" for an open beta.
fn current(fragments: &[(std::path::PathBuf, Fragment)], region: Region) -> std::collections::HashSet<String> {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let Some((root_path, root)) = fragments
        .iter()
        .filter(|(_, f)| !f.fragments.is_empty())
        .max_by_key(|(path, _)| modified(path))
    else {
        return Default::default();
    };
    let mut current: std::collections::HashSet<String> = root
        .fragments
        .iter()
        .filter(|f| f.requires.as_ref().is_none_or(|r| holds(r, region) == Some(true)))
        .map(|f| f.hash.to_ascii_lowercase())
        .collect();
    // The root carries strings of its own, `default#PLAY_DISABLED_MESSAGE`.
    if let Some(name) = root_path.file_name().and_then(|n| n.to_str()) {
        current.insert(name.to_owned());
    }
    current
}

/// Whether a fragment's condition holds, `None` when it cannot be decided
/// here. Only the login region is decided; it is the configured region.
fn holds(requires: &serde_json::Value, region: Region) -> Option<bool> {
    let all = |key: &str| -> Option<Vec<bool>> {
        requires.get(key)?.as_array()?.iter().map(|r| holds(r, region)).collect()
    };
    match requires.get("type").and_then(|t| t.as_str()) {
        Some("login_region") => {
            let ours = login_region(region)?;
            let names = |key: &str| -> Option<bool> {
                let value = requires.get(key)?;
                Some(match value.as_array() {
                    Some(list) => list.iter().any(|v| v.as_str() == Some(ours)),
                    None => value.as_str() == Some(ours),
                })
            };
            match (names("include"), names("exclude")) {
                (Some(included), None) => Some(included),
                (None, Some(excluded)) => Some(!excluded),
                _ => None,
            }
        }
        Some(_) => None,
        None => {
            if let Some(any) = all("any_of") {
                Some(any.contains(&true))
            } else if let Some(each) = all("all_of") {
                Some(!each.contains(&false))
            } else if let Some(none) = all("none_of") {
                Some(!none.contains(&true))
            } else {
                Some(!holds(requires.get("not")?, region)?)
            }
        }
    }
}

/// Battle.net's login region for a CDN region, where it is certain.
fn login_region(region: Region) -> Option<&'static str> {
    match region {
        Region::Us => Some("US"),
        Region::Eu => Some("EU"),
        Region::Kr => Some("KR"),
        Region::Cn => Some("CN"),
        Region::Tw | Region::Sg => None,
    }
}

/// One string in the asked locale, else the client's `default`.
fn localized<'a>(text: &'a Localized, locale: &str) -> Option<&'a str> {
    text.get(locale).or_else(|| text.get("default")).map(String::as_str)
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
/// Everything else stays unparsed, so it cannot break.
#[derive(Debug, Deserialize)]
struct Fragment {
    #[serde(default)]
    products: Vec<Product>,
    #[serde(default)]
    installs: BTreeMap<String, Install>,
    /// Locale (`default`, `deDE`, …) to string id to text.
    #[serde(default)]
    strings: BTreeMap<String, BTreeMap<String, String>>,
    /// Only in the root: the current hash of every fragment.
    #[serde(default)]
    fragments: Vec<FragmentRef>,
}

#[derive(Debug, Deserialize)]
struct FragmentRef {
    hash: String,
    #[serde(default)]
    requires: Option<serde_json::Value>,
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
    /// `retail`, `ptr`, `beta`, ..., all of one family.
    #[serde(default)]
    types: BTreeMap<String, TypeEntry>,
}

#[derive(Debug, Deserialize)]
struct TypeEntry {
    #[serde(default)]
    uid: Option<String>,
    /// String ids, resolved against [`Fragment::strings`].
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    selector_display_name: Option<String>,
    #[serde(default)]
    release_date_message: Option<String>,
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

        let families = Families::read_dir(&cache, Region::Eu);
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

        let families = Families::read_dir(&cache, Region::Eu);
        assert_eq!(families.family_of("fenris"), Some("Fen"));
        assert_eq!(families.family_of("fenrisb"), Some("Fen"), "a PTR is the same tile");

        let _ = std::fs::remove_dir_all(&cache);
    }

    /// The shape Battle.net cached for the Forever stress test, 2026-10-14:
    /// the name in the title's own fragment, the date in a preload fragment
    /// that states messages and no uid, and a root naming which revision of
    /// each is current.
    #[test]
    fn a_preload_reads_its_version_name_and_release_date() {
        let cache = scratch("preload");
        let title = "9af8000000000000000000000000000a";
        let preload = "6385000000000000000000000000000b";
        write(
            &cache,
            title,
            r#"{"products":[{"id":"WoW","base":{"types":{
                 "wowf":{"uid":"wowf","selector_display_name":"world_of_warcraft#ST_NAME"},
                 "beta":{"uid":"wow_beta","name":"world_of_warcraft#MIDNIGHT_BETA"}}}}],
               "installs":{"wowf":{"tact_product":"wowf"}},
               "strings":{
                 "default":{"world_of_warcraft#ST_NAME":"WoW: Forever - Stress Test",
                            "world_of_warcraft#MIDNIGHT_BETA":"Midnight Beta"},
                 "deDE":{"world_of_warcraft#ST_NAME":"WoW: Forever \u2013 Belastungstest"}}}"#,
        );
        write(
            &cache,
            preload,
            r#"{"products":[{"id":"WoW","base":{"types":{
                 "wowf":{"release_date_message":"preload#DATE"}}}}],
               "strings":{"default":{"preload#DATE":"Available October 19, 2026"},
                          "deDE":{"preload#DATE":"Verfügbar ab 19. Oktober 2026"}}}"#,
        );
        // An older revision of the title fragment, still in the cache.
        write(
            &cache,
            "fc99000000000000000000000000000c",
            r#"{"products":[{"id":"WoW","base":{"types":{
                 "wowf":{"uid":"wowf","name":"old#NAME"}}}}],
               "strings":{"default":{"old#NAME":"WoW - Something Else"}}}"#,
        );
        // Current, but only for accounts without the licence: undecidable here.
        let gated = "c255000000000000000000000000000d";
        write(
            &cache,
            gated,
            r#"{"products":[{"id":"WoW","base":{"types":{
                 "wow_classic_beta":{"release_date_message":"gated#DATE"}}}}],
               "strings":{"default":{"gated#DATE":"Available November 4, 2026"}}}"#,
        );
        // Current, and decidable, but for China only.
        let cn = "3dc3000000000000000000000000000e";
        write(
            &cache,
            cn,
            r#"{"products":[{"id":"WoW","base":{"types":{
                 "wow":{"release_date_message":"cn#DATE"}}}}],
               "strings":{"default":{"cn#DATE":"CN only"}}}"#,
        );
        write(
            &cache,
            "ab86000000000000000000000000000f",
            &format!(
                r#"{{"fragments":[
                     {{"name":"world_of_warcraft","hash":"{title}"}},
                     {{"name":"preload","hash":"{preload}","requires":{{"type":"login_region","exclude":"CN"}}}},
                     {{"name":"gated","hash":"{gated}","requires":{{"type":"licenses","op":"none_of","licenses":[1]}}}},
                     {{"name":"cn","hash":"{cn}","requires":{{"any_of":[{{"type":"login_region","include":"CN"}}]}}}}]}}"#
            ),
        );

        let families = Families::read_dir(&cache, Region::Eu);
        assert_eq!(families.version_name("wowf", "enUS"), Some("Stress Test"));
        assert_eq!(families.version_name("wowf", "deDE"), Some("Belastungstest"));
        assert_eq!(families.release_date("wowf", "enUS"), Some("Available October 19, 2026"));
        assert_eq!(families.release_date("wowf", "deDE"), Some("Verfügbar ab 19. Oktober 2026"));
        // A name with no version part says nothing, rather than an expansion.
        assert_eq!(families.version_name("wow_beta", "enUS"), None);
        assert_eq!(families.release_date("wow_classic_beta", "enUS"), None, "licence-gated");
        assert_eq!(families.release_date("wow", "enUS"), None, "another region's");
        assert_eq!(families.family_of("wowf"), Some("WoW"));

        // The same cache read from China: the preload excludes it.
        let cn_view = Families::read_dir(&cache, Region::Cn);
        assert_eq!(cn_view.release_date("wowf", "enUS"), None);
        assert_eq!(cn_view.release_date("wow", "enUS"), Some("CN only"));

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

        let families = Families::read_dir(&cache, Region::Eu);
        assert!(families.is_empty());

        // And a prefix whose client has never run.
        assert!(Families::read_dir(&cache.join("nothing-here"), Region::Eu).is_empty());

        let _ = std::fs::remove_dir_all(&cache);
    }
}




