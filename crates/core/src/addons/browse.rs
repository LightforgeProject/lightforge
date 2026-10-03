//! Searching for addons to install.
//!
//! The import-token flow answers "give me this exact list". This answers the
//! other question — "what is there?" — which is what makes Lightforge an addon
//! manager rather than an importer.
//!
//! Each provider is searched the cheapest way it allows:
//!
//! - **WoWInterface** publishes its entire catalogue as one 2.7 MB
//!   `filelist.json` (7,702 addons), so it is fetched once, cached, and
//!   searched locally. Every keystroke costs nothing after that.
//! - **Tukui** lists two addons in 4 KB. Same treatment, trivially.
//! - **CurseForge** has no public catalogue dump, so its search is a request to
//!   the proxy, and is simply absent when no proxy is configured.
//! - **GitHub** cannot be searched usefully without authentication, so an
//!   `owner/repo` typed exactly is offered as a direct result.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::catalog::{Catalog, CatalogError};
use super::Provider;
use crate::net::Http;

/// How long a cached catalogue is considered fresh.
///
/// Addon listings move slowly and the payload is megabytes; re-fetching on
/// every search would be rude to a service that gives its whole catalogue away
/// for free.
pub const CATALOGUE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const WOWI_FILELIST: &str = "https://api.mmoui.com/v4/game/WOW/filelist.json";
const TUKUI_ADDONS: &str = "https://api.tukui.org/v1/addons";

/// One search result, provider-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub provider: Provider,
    /// The id to install by — a numeric id, a slug, or `owner/repo`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Lifetime downloads, where the provider reports them. Used for ranking.
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub website_url: Option<String>,
}

/// A WoWInterface catalogue entry.
#[derive(Debug, Clone, Deserialize)]
struct WowiEntry {
    id: u64,
    title: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    downloads: u64,
    /// `gameVersions` is **null** for 3,510 of WoWInterface's 7,702 entries,
    /// and `#[serde(default)]` covers a *missing* key, not an explicit null —
    /// so this must be an `Option` or the whole catalogue fails to parse.
    #[serde(rename = "gameVersions", default)]
    game_versions: Option<Vec<String>>,
    #[serde(rename = "fileInfoUri", default)]
    file_info_uri: Option<String>,
}

// The API spells it camelCase; serde needs telling once.
impl WowiEntry {
    fn flavors(&self) -> &[String] {
        self.game_versions.as_deref().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TukuiEntry {
    slug: String,
    name: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
}

/// Search every provider that can answer, best matches first.
pub async fn search(
    http: &Http,
    catalog: Option<&Catalog>,
    cache_dir: &Path,
    query: &str,
    flavor: &str,
    limit: usize,
) -> Results {
    let query = query.trim();
    if query.is_empty() {
        return Results::default();
    }

    let mut results = Vec::new();
    let mut problems = Vec::new();

    // An `owner/repo` typed exactly is a GitHub addon; nothing to search for.
    if let Some(listing) = github_direct(query) {
        results.push(listing);
    }

    // A provider that cannot answer is reported, not silently dropped: an
    // empty result list and a broken catalogue look identical to a user, and
    // that is how a parse failure hid here once already.
    match wowi_catalogue(http, cache_dir).await {
        Ok(entries) => results.extend(search_wowi(&entries, query, flavor)),
        Err(e) => problems.push(format!("WoWInterface: {e}")),
    }
    match tukui_catalogue(http, cache_dir).await {
        Ok(entries) => results.extend(search_tukui(&entries, query)),
        Err(e) => problems.push(format!("Tukui: {e}")),
    }
    if let Some(catalog) = catalog {
        match catalog.search(http, flavor, query, limit).await {
            Ok(projects) => results.extend(projects.into_iter().map(|p| Listing {
                provider: Provider::Curse,
                id: p.id,
                name: p.name,
                author: p.authors.first().cloned(),
                version: None,
                downloads: p.download_count,
                website_url: p.website_url,
            })),
            Err(e) => problems.push(format!("CurseForge: {e}")),
        }
    }

    rank(&mut results, query);
    results.truncate(limit);
    Results { listings: results, problems }
}

/// What a search found, and which providers could not answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub listings: Vec<Listing>,
    /// One line per provider that failed, for the UI to show. A search that
    /// finds nothing because a provider is down must not look like a search
    /// that found nothing because nothing matched.
    pub problems: Vec<String>,
}

/// Order results by how well they match, then by popularity.
///
/// An exact title match must outrank a more-downloaded addon that merely
/// contains the word, or searching "BigWigs" surfaces eleven BigWigs plugins
/// before BigWigs itself.
fn rank(results: &mut [Listing], query: &str) {
    let needle = fold(query);
    results.sort_by(|a, b| {
        score(&a.name, &needle)
            .cmp(&score(&b.name, &needle))
            .then(b.downloads.cmp(&a.downloads))
            .then_with(|| a.name.len().cmp(&b.name.len()))
    });
}

/// Lower is better. Both sides are folded, so "big wigs" scores against
/// "BigWigs" as an exact match rather than as no match at all.
fn score(name: &str, needle: &str) -> u8 {
    let name = fold(name);
    match () {
        _ if name == needle => 0,
        _ if name.starts_with(needle) => 1,
        _ if name.contains(needle) => 2,
        _ => 3,
    }
}

/// Lowercase, keeping only letters and digits.
///
/// Addon names are typed from memory, not copied: "big wigs", "BigWigs" and
/// "Big-Wigs" are one query, and "ElvUI" is often typed "Elv UI". Folding
/// also strips the decoration real titles carry — "BigWigs (Boss Timers &
/// Tools)" still begins with the query once the punctuation is gone.
fn fold(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn search_wowi(entries: &[WowiEntry], query: &str, flavor: &str) -> Vec<Listing> {
    // Folded on both sides, so "big wigs" finds "BigWigs". A matcher that is
    // stricter than the ranker hides results the ranker was written to order.
    let needle = fold(query);
    entries
        .iter()
        .filter(|e| fold(&e.title).contains(&needle))
        .filter(|e| e.flavors().is_empty() || supports(e.flavors(), flavor))
        .map(|e| Listing {
            provider: Provider::WowInterface,
            id: e.id.to_string(),
            name: e.title.clone(),
            author: e.author.clone(),
            version: e.version.clone(),
            downloads: e.downloads,
            website_url: e.file_info_uri.clone(),
        })
        .collect()
}

fn search_tukui(entries: &[TukuiEntry], query: &str) -> Vec<Listing> {
    let needle = fold(query);
    entries
        .iter()
        .filter(|e| fold(&e.name).contains(&needle) || fold(&e.slug).contains(&needle))
        .map(|e| Listing {
            provider: Provider::TukUI,
            // Slug, not id: Tukui's ids can be negative.
            id: e.slug.clone(),
            name: e.name.clone(),
            author: e.author.clone(),
            version: e.version.clone(),
            downloads: 0,
            website_url: e.web_url.clone(),
        })
        .collect()
}

/// `owner/repo` typed exactly, offered as a direct GitHub result.
fn github_direct(query: &str) -> Option<Listing> {
    let (owner, repo) = query.split_once('/')?;
    let ok = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    (ok(owner) && ok(repo)).then(|| Listing {
        provider: Provider::GitHub,
        id: query.to_owned(),
        name: query.to_owned(),
        author: Some(owner.to_owned()),
        version: None,
        downloads: 0,
        website_url: Some(format!("https://github.com/{query}")),
    })
}

/// Whether a catalogue entry's game versions cover a flavor.
///
/// Version *lines* rather than exact patches: retail is the 10+ major series,
/// Classic Era is the 1.x line, and the progression Classic realms move through
/// 2.x-5.x. Coarse on purpose — a hard match on the live patch would hide every
/// addon whose author has not yet bumped their `.toc`.
fn supports(game_versions: &[String], flavor: &str) -> bool {
    game_versions.iter().any(|version| {
        let major: u32 = version
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
            .unwrap_or(0);
        match flavor {
            "retail" => major >= 9,
            "classic_era" => major == 1,
            "classic" => (2..=5).contains(&major),
            _ => true,
        }
    })
}

async fn wowi_catalogue(http: &Http, cache_dir: &Path) -> Result<Vec<WowiEntry>, CatalogError> {
    cached_json(http, cache_dir, "wowinterface-filelist.json", WOWI_FILELIST).await
}

async fn tukui_catalogue(http: &Http, cache_dir: &Path) -> Result<Vec<TukuiEntry>, CatalogError> {
    cached_json(http, cache_dir, "tukui-addons.json", TUKUI_ADDONS).await
}

/// Fetch and cache a whole catalogue, re-using it while it is fresh.
async fn cached_json<T: serde::de::DeserializeOwned>(
    http: &Http,
    cache_dir: &Path,
    name: &str,
    url: &str,
) -> Result<T, CatalogError> {
    let path = cache_dir.join("catalogues").join(name);
    if is_fresh(&path) {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(parsed) = serde_json::from_slice(&bytes) {
                return Ok(parsed);
            }
            // A corrupt cache is replaced rather than reported: it is a copy of
            // something freely refetchable.
        }
    }

    let body = http.get_bytes(url).await?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &body);

    serde_json::from_slice(&body).map_err(|source| CatalogError::Fixture { path, source })
}

fn is_fresh(path: &PathBuf) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < CATALOGUE_TTL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wowi(id: u64, title: &str, downloads: u64, versions: &[&str]) -> WowiEntry {
        WowiEntry {
            id,
            title: title.to_owned(),
            author: Some("someone".into()),
            version: Some("1.0".into()),
            downloads,
            game_versions: Some(versions.iter().map(|v| (*v).to_string()).collect()),
            file_info_uri: None,
        }
    }

    #[test]
    fn the_catalogue_json_keys_are_the_ones_the_api_actually_sends() {
        // Guards the camelCase renames: a missing one silently empties the
        // field and flavor filtering then hides nothing, or everything.
        let json = br#"[{"id":26065,"title":"BigWigs Countdown","author":"nebula",
            "version":"1.4.9","downloads":556,"gameVersions":["12.0.7","1.15.7"],
            "fileInfoUri":"https://www.wowinterface.com/downloads/info26065.html"}]"#;
        let entries: Vec<WowiEntry> = serde_json::from_slice(json).unwrap();

        assert_eq!(entries[0].flavors(), ["12.0.7", "1.15.7"]);
        assert!(entries[0].file_info_uri.is_some());
        assert_eq!(entries[0].downloads, 556);
    }

    #[test]
    fn a_null_game_versions_does_not_kill_the_whole_catalogue() {
        // 3,510 of 7,702 real entries send `"gameVersions": null`. Parsing must
        // survive them, or searching finds nothing at all — which is exactly
        // what happened before this was caught.
        let json = br#"[{"id":1,"title":"Nulled","author":"x","version":"1",
                         "downloads":5,"gameVersions":null,"fileInfoUri":"u"},
                        {"id":2,"title":"Fine","author":"x","version":"1",
                         "downloads":5,"gameVersions":["12.0.7"],"fileInfoUri":"u"}]"#;
        let entries: Vec<WowiEntry> = serde_json::from_slice(json).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries[0].flavors().is_empty());
        // And the one with no versions is still searchable.
        assert_eq!(search_wowi(&entries, "nulled", "retail").len(), 1);
    }

    #[test]
    fn an_exact_match_outranks_a_more_popular_near_match() {
        // Searching "BigWigs" must not bury BigWigs under its own plugins.
        let entries = vec![
            wowi(1, "BigWigs Countdown: Heroes of the Storm", 900_000, &["12.0.7"]),
            wowi(2, "BigWigs", 500, &["12.0.7"]),
        ];
        let mut found = search_wowi(&entries, "BigWigs", "retail");
        rank(&mut found, "BigWigs");

        assert_eq!(found[0].name, "BigWigs");
    }

    #[test]
    fn a_name_typed_with_a_space_still_matches_and_outranks_a_popular_plugin() {
        // "big wigs" is how it gets typed from memory. Folded, it is an exact
        // match for BigWigs and only a prefix of BigWigs_Voice, so the core
        // addon wins on match quality despite 1800x fewer downloads.
        let entries = vec![
            wowi(1, "BigWigs_Voice", 900_000, &["12.0.7"]),
            wowi(2, "BigWigs", 500, &["12.0.7"]),
        ];
        let mut found = search_wowi(&entries, "big wigs", "retail");
        rank(&mut found, "big wigs");

        assert_eq!(found.len(), 2, "a space must not filter both out");
        assert_eq!(found[0].name, "BigWigs");
    }

    #[test]
    fn a_decorated_title_is_found_by_the_bare_name() {
        // Real catalogue titles carry parentheticals. Folding reaches past
        // them; it cannot, however, tell "BigWigs (Boss Timers)" apart from
        // "BigWigs_Voice" on name alone — both are prefix matches, and
        // downloads are the only honest signal left to separate them.
        let entries = vec![wowi(1, "BigWigs (Boss Timers & Tools)", 500, &["12.0.7"])];

        assert_eq!(search_wowi(&entries, "big wigs", "retail").len(), 1);
        assert_eq!(fold("Elv UI"), "elvui");
        assert_eq!(fold("Big-Wigs!"), "bigwigs");
    }

    #[test]
    fn popularity_breaks_ties_within_the_same_match_quality() {
        let entries = vec![
            wowi(1, "Quest Helper Lite", 10, &["12.0.7"]),
            wowi(2, "Quest Helper Pro", 9_000, &["12.0.7"]),
        ];
        let mut found = search_wowi(&entries, "quest helper", "retail");
        rank(&mut found, "quest helper");

        assert_eq!(found[0].name, "Quest Helper Pro");
    }

    #[test]
    fn flavor_filtering_is_by_version_line_not_exact_patch() {
        let entries = vec![
            wowi(1, "Retail Thing", 1, &["12.0.7"]),
            wowi(2, "Era Thing", 1, &["1.15.7"]),
            wowi(3, "MoP Thing", 1, &["5.5.0"]),
        ];

        let names = |flavor: &str| -> Vec<String> {
            search_wowi(&entries, "thing", flavor).into_iter().map(|l| l.name).collect()
        };
        assert_eq!(names("retail"), ["Retail Thing"]);
        assert_eq!(names("classic_era"), ["Era Thing"]);
        assert_eq!(names("classic"), ["MoP Thing"]);
    }

    #[test]
    fn an_entry_declaring_no_versions_is_kept_rather_than_hidden() {
        // Plenty of older entries list nothing; hiding them would make the
        // catalogue look far emptier than it is.
        let entries = vec![wowi(1, "Ancient Thing", 1, &[])];
        assert_eq!(search_wowi(&entries, "ancient", "retail").len(), 1);
    }

    #[test]
    fn owner_slash_repo_is_offered_as_a_github_result() {
        let listing = github_direct("WeakAuras/WeakAuras2").unwrap();
        assert_eq!(listing.provider, Provider::GitHub);
        assert_eq!(listing.id, "WeakAuras/WeakAuras2");

        // Ordinary searches are not mistaken for repos.
        assert!(github_direct("boss mods").is_none());
        assert!(github_direct("elvui").is_none());
        assert!(github_direct("a//b").is_none());
    }
}
