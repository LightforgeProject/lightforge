//! Headlines for the News page.
//!
//! The client parses no RSS. The proxy that fronts CurseForge also merges the
//! feeds and hands over one JSON document, which is the whole reason this
//! module is forty lines of `serde` and a cache rather than a feed reader: a
//! site changing its XML is a redeploy over there, not a release of this.
//!
//! Cached under [`crate::paths::AppDirs::news_cache_file`] with its ETag, the
//! same mechanism the asset manifest uses. The difference is what happens when
//! the fetch fails: art falls back to the last good copy silently, while news
//! that is a few hours old is still worth reading, so a failure here *always*
//! yields the cached copy and says it is stale rather than returning an error.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::net::{ETagCache, Http};
use crate::product::ProductLine;

/// One headline.
///
/// Every field but the first four is optional because the feeds disagree about
/// what they carry: an article with no image is ordinary, and a category is
/// whatever the author tagged it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// RFC3339, UTC. Kept as the server's own string: the UI prints a date,
    /// and parsing one to re-format it is how time zones get invented.
    #[serde(default)]
    pub published: String,
}

impl Item {
    /// Which WoW line this article is about, as far as its category says.
    ///
    /// `None` for about a third of the feed (BlizzCon, site news): filing it
    /// under the selected game would be a lie. Measured on 99 items: Live 27,
    /// Forever 28, Classic 8, PTR 4, none 32. Classic and Classic Era are not
    /// separable (Wowhead tags both `Classic`). Nothing maps to a non-WoW game,
    /// since every source is Wowhead.
    pub fn line(&self) -> Option<ProductLine> {
        let category = self.category.as_deref()?.trim();
        // Case-insensitively, because the tag is whatever an author typed.
        match category.to_ascii_lowercase().as_str() {
            "live" | "retail" | "ptr" => Some(ProductLine::Retail),
            "forever" => Some(ProductLine::Forever),
            "classic" | "classic era" | "era" | "hardcore" | "season of discovery" => {
                Some(ProductLine::Classic)
            }
            _ => None,
        }
    }

    /// Whether this article belongs on `line`'s panel.
    ///
    /// Classic and Classic Era share, for the reason in [`Item::line`].
    pub fn is_about(&self, line: &ProductLine) -> bool {
        match (self.line(), line) {
            (Some(ProductLine::Classic), ProductLine::Classic | ProductLine::ClassicEra) => true,
            (Some(mine), wanted) => &mine == wanted,
            (None, _) => false,
        }
    }
}

impl Feed {
    /// The newest `limit` articles about one line, newest first.
    ///
    /// The feed arrives newest first and nothing re-sorts it, so this is a
    /// filter and a truncation rather than a search.
    pub fn about(&self, line: &ProductLine, limit: usize) -> Vec<&Item> {
        self.items.iter().filter(|item| item.is_about(line)).take(limit).collect()
    }
}

/// What the proxy served.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    #[serde(default)]
    pub items: Vec<Item>,
    /// Feeds the proxy could not read this time. A partial answer is still a
    /// `200`, so this is the only way to know one source is missing.
    #[serde(default)]
    pub degraded: Vec<String>,
}

/// A feed and how current it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headlines {
    pub feed: Feed,
    /// `true` when the network could not be reached and this came off disk.
    /// The page says so quietly; it does not refuse to draw.
    pub stale: bool,
}

/// Where headlines are read from.
///
/// Deliberately derived from the addon catalog's base rather than configured
/// separately: it is one proxy serving two namespaces, and a second URL to
/// keep in step would be a second thing to get wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Base URL of the proxy, e.g. `https://lightforge.tuxcore.pro/`.
    Http(String),
    /// A fixture directory holding `news.json`.
    Dir(PathBuf),
}

const FEED_PATH: &str = "news/v1/feed";
const FIXTURE_FILE: &str = "news.json";

impl Origin {
    fn url(&self) -> String {
        match self {
            Self::Http(base) => format!("{}/{FEED_PATH}", base.trim_end_matches('/')),
            Self::Dir(dir) => dir.join(FIXTURE_FILE).display().to_string(),
        }
    }
}

/// Fetch the headlines, falling back to the last good copy.
///
/// Never returns an error for a network problem, only for a cache that is
/// both absent and unreachable, which is the one case where there is genuinely
/// nothing to show. A `304` is not a failure and is not reported as stale: the
/// cached copy *is* the current one.
pub async fn headlines(http: &Http, origin: &Origin, cache_file: &Path) -> Option<Headlines> {
    if let Origin::Dir(dir) = origin {
        let path = dir.join(FIXTURE_FILE);
        let bytes = std::fs::read(&path).ok()?;
        let feed: Feed = serde_json::from_slice(&bytes).ok()?;
        return Some(Headlines { feed, stale: false });
    }

    let cache = ETagCache::new(cache_file);
    match http.get_json_cached::<Feed>(&origin.url(), &cache).await {
        Ok(Some(feed)) => Some(Headlines { feed, stale: false }),
        // 304: what is on disk is current, not stale.
        Ok(None) => cache.read_json().ok().map(|feed| Headlines { feed, stale: false }),
        Err(_) => cache.read_json().ok().map(|feed| Headlines { feed, stale: true }),
    }
}

/// Fetch one headline's picture into `dir`, returning the file.
///
/// Content-addressed by its URL rather than by its own bytes: the file name
/// has to be known *before* anything is downloaded, so that an image already
/// on disk costs a `stat` and a `304` at most. Wowhead's own paths carry a
/// numeric id and a slug, which are neither unique across hosts nor safe as
/// file names, so the hash is what makes them both.
///
/// A failure is `None`, never an error. A card with no picture is a card; a
/// news page that refuses to draw because a CDN hiccuped is not.
pub async fn image(http: &Http, dir: &Path, url: &str) -> Option<PathBuf> {
    let path = dir.join(image_name(url));
    let cache = ETagCache::new(&path);

    match http.get_bytes_cached(url, &cache).await {
        // Downloaded, and already written to `path` by the cache.
        Ok(Some(_)) => Some(path),
        // 304, what is on disk is current.
        Ok(None) => path.is_file().then_some(path),
        // Unreachable: a copy from an earlier run is still worth drawing.
        Err(_) => path.is_file().then_some(path),
    }
}

/// Delete cached pictures no current headline names.
///
/// Called after a fetch, so the directory holds one feed's worth, about
/// forty files, rather than growing for the life of the install. Wowhead
/// publishes these at full article size (250-800 KB each), which is the
/// difference between a bounded cache and a slow leak nobody notices.
///
/// Errors are ignored throughout: failing to tidy up is not a reason to fail
/// a page that has already drawn.
pub fn prune_images(dir: &Path, keep: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut wanted: Vec<String> = keep.iter().map(|url| image_name(url)).collect();
    // The ETag sidecar belongs to its body: dropping one and keeping the
    // other would send `If-None-Match` with nothing to fall back on.
    wanted.extend(wanted.clone().iter().map(|name| format!("{name}.etag")));

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !wanted.iter().any(|w| w == name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The cache file name for an image URL.
///
/// The extension is taken from the URL rather than sniffed, because the only
/// thing reading it back is the `image` crate, which guesses from content
/// anyway. It exists so the directory is browsable by a human debugging it.
fn image_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let stem: String = format!("{digest:x}").chars().take(32).collect();

    let extension = url
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .map(|(_, ext)| ext)
        // Strip a query string, and refuse anything that is not a plain
        // extension: this ends up in a path.
        .map(|ext| ext.split(['?', '#']).next().unwrap_or_default())
        .filter(|ext| !ext.is_empty() && ext.len() <= 5)
        .filter(|ext| ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("img");

    format!("{stem}.{extension}")
}

#[cfg(test)]
mod tests {

    /// The mapping exists to answer "what is this article about", and the
    /// honest answer for a third of the feed is "nothing in particular".
    #[test]
    fn a_category_places_an_article_on_a_line_or_on_none() {
        let tagged = |category: &str| Item {
            category: Some(category.to_owned()),
            ..Item::default()
        };

        assert_eq!(tagged("Live").line(), Some(ProductLine::Retail));
        assert_eq!(tagged("PTR").line(), Some(ProductLine::Retail));
        assert_eq!(tagged("Forever").line(), Some(ProductLine::Forever));
        assert_eq!(tagged("Classic").line(), Some(ProductLine::Classic));
        // Whatever an author typed.
        assert_eq!(tagged("  live  ").line(), Some(ProductLine::Retail));

        // The third of the feed that belongs to no line: BlizzCon coverage,
        // site news, Blizzard-wide announcements. Filing these under whichever
        // game happens to be selected would be a lie.
        assert_eq!(tagged("Blizzard").line(), None);
        assert_eq!(tagged("Wowhead").line(), None);
        assert_eq!(tagged("WoW").line(), None);
        assert_eq!(Item::default().line(), None, "no category at all");
    }

    /// Wowhead tags Classic and Classic Era alike, so they share. A limit of
    /// the source: the alternative is guessing which one an article meant.
    #[test]
    fn classic_and_classic_era_see_the_same_articles() {
        let item = Item { category: Some("Classic".to_owned()), ..Item::default() };
        assert!(item.is_about(&ProductLine::Classic));
        assert!(item.is_about(&ProductLine::ClassicEra));
        assert!(!item.is_about(&ProductLine::Retail));
    }

    /// Nothing in this feed is about a game Battle.net hands off, because
    /// every source the proxy merges is Wowhead. The caller relies on this to
    /// know it should draw no panel at all rather than an empty one.
    #[test]
    fn a_handoff_title_matches_nothing() {
        let feed = Feed {
            items: vec![
                Item { category: Some("Live".to_owned()), ..Item::default() },
                Item { category: Some("Forever".to_owned()), ..Item::default() },
                Item { category: Some("Blizzard".to_owned()), ..Item::default() },
            ],
            degraded: Vec::new(),
        };
        let diablo = ProductLine::Custom("d4".to_owned());
        assert!(feed.about(&diablo, 3).is_empty());
        // And the lines that do exist get only their own.
        assert_eq!(feed.about(&ProductLine::Retail, 3).len(), 1);
        assert_eq!(feed.about(&ProductLine::Forever, 3).len(), 1);
    }

    /// Newest first and no more than asked for: the feed arrives in order and
    /// nothing here re-sorts it.
    #[test]
    fn the_panel_takes_the_newest_few() {
        let live = |title: &str| Item {
            title: title.to_owned(),
            category: Some("Live".to_owned()),
            ..Item::default()
        };
        let feed = Feed {
            items: vec![live("newest"), live("middle"), live("oldest")],
            degraded: Vec::new(),
        };
        let shown: Vec<&str> = feed.about(&ProductLine::Retail, 2).iter().map(|i| i.title.as_str()).collect();
        assert_eq!(shown, vec!["newest", "middle"]);
    }
    use super::*;

    #[test]
    fn the_feed_url_sits_under_its_own_namespace() {
        let origin = Origin::Http("https://lightforge.tuxcore.pro/".into());
        assert_eq!(origin.url(), "https://lightforge.tuxcore.pro/news/v1/feed");
    }

    /// A base without the trailing slash must not produce a doubled one, and
    /// one with several must not either: both have reached the addon catalog
    /// from hand-edited config before.
    #[test]
    fn a_base_url_is_joined_exactly_once() {
        for base in ["https://example.test", "https://example.test/", "https://example.test///"] {
            assert_eq!(
                Origin::Http(base.into()).url(),
                "https://example.test/news/v1/feed",
                "base {base} joined wrong",
            );
        }
    }

    /// The shape contract with the Go proxy, read from the fixture that
    /// repository generates from its real handler. A field renamed there
    /// fails here rather than in someone's launcher.
    #[test]
    fn the_proxy_fixture_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog");
        let bytes = std::fs::read(dir.join(FIXTURE_FILE)).expect("news.json fixture");
        let feed: Feed = serde_json::from_slice(&bytes).expect("fixture parses as a Feed");

        let item = feed.items.first().expect("the fixture carries an item");
        assert!(!item.title.is_empty());
        assert!(item.url.starts_with("http"));
        assert!(item.published.ends_with('Z'), "published is RFC3339 UTC");
        // The fixture is generated with one feed deliberately missing, so a
        // partial answer is exercised rather than only the happy path.
        assert!(!feed.degraded.is_empty(), "the fixture should carry a degraded feed");
    }

    #[test]
    fn an_image_name_is_derived_from_the_whole_url() {
        let a = image_name("https://wow.zamimg.com/uploads/blog/images/55602-stitches.jpg");
        let b = image_name("https://wow.zamimg.com/uploads/blog/images/55603-stitches.jpg");
        assert_ne!(a, b);
        assert!(a.ends_with(".jpg"), "{a}");
        // Stable across calls, or every run would re-download everything.
        assert_eq!(a, image_name("https://wow.zamimg.com/uploads/blog/images/55602-stitches.jpg"));
    }

    /// The name ends up in a path, so nothing from the URL may travel in it.
    #[test]
    fn an_image_name_is_never_a_path() {
        for url in [
            "https://x.test/a/../../etc/passwd",
            "https://x.test/image.jpg?size=/../../nope",
            "https://x.test/image.a/b",
            "https://x.test/",
            "https://x.test/no-extension",
        ] {
            let name = image_name(url);
            assert!(!name.contains('/'), "{url} -> {name}");
            assert!(!name.contains(".."), "{url} -> {name}");
        }
    }

    /// A query string is not part of the extension.
    #[test]
    fn a_query_string_does_not_become_the_extension() {
        assert!(image_name("https://x.test/a.jpg?v=2").ends_with(".jpg"));
    }

    #[test]
    fn pruning_keeps_the_current_feed_and_its_etags_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("lf-news-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let keep = "https://x.test/keep.jpg".to_owned();
        let drop = "https://x.test/drop.jpg".to_owned();
        for url in [&keep, &drop] {
            let name = image_name(url);
            std::fs::write(dir.join(&name), b"body").unwrap();
            std::fs::write(dir.join(format!("{name}.etag")), b"\"abc\"").unwrap();
        }
        // Something that was never ours.
        std::fs::write(dir.join("stray.tmp"), b"x").unwrap();

        prune_images(&dir, std::slice::from_ref(&keep));

        assert!(dir.join(image_name(&keep)).is_file(), "the live picture was deleted");
        assert!(
            dir.join(format!("{}.etag", image_name(&keep))).is_file(),
            "its ETag was deleted, so the next request cannot revalidate",
        );
        assert!(!dir.join(image_name(&drop)).exists(), "a stale picture survived");
        assert!(!dir.join("stray.tmp").exists(), "an unrelated file survived");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that does not exist yet is the normal first run.
    #[test]
    fn pruning_a_missing_directory_is_not_an_error() {
        prune_images(Path::new("/nonexistent/lightforge/news"), &[]);
    }

    /// An item the server has gained a field for must still parse here: the
    /// client is deployed far more slowly than the proxy.
    #[test]
    fn an_unknown_field_does_not_fail_the_document() {
        let raw = r#"{"items":[{"id":"1","title":"T","url":"https://x.test",
                     "published":"2026-10-05T21:10:19Z","mood":"cheerful"}]}"#;
        let feed: Feed = serde_json::from_str(raw).expect("unknown fields are ignored");
        assert_eq!(feed.items.len(), 1);
    }

    /// Missing optionals are the normal case, not an error.
    #[test]
    fn an_item_without_an_image_or_category_is_fine() {
        let raw = r#"{"items":[{"id":"1","title":"T","url":"https://x.test",
                     "published":"2026-10-05T21:10:19Z"}]}"#;
        let feed: Feed = serde_json::from_str(raw).unwrap();
        assert_eq!(feed.items[0].image, None);
        assert_eq!(feed.items[0].category, None);
    }

    #[tokio::test]
    async fn a_fixture_directory_is_read_without_a_server() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog");
        let http = Http::new().unwrap();
        let got = headlines(&http, &Origin::Dir(dir), Path::new("/nonexistent"))
            .await
            .expect("the fixture directory answers");
        assert!(!got.stale);
        assert!(!got.feed.items.is_empty());
    }

    /// Nothing cached and nothing reachable is the one case with genuinely
    /// nothing to show, and it must be `None`, not a panic or an empty feed
    /// the page would render as "no news today".
    #[tokio::test]
    async fn an_unreachable_proxy_with_no_cache_has_nothing_to_say() {
        let http = Http::new().unwrap();
        let cache = std::env::temp_dir().join("lightforge-news-absent.json");
        let _ = std::fs::remove_file(&cache);
        let got = headlines(
            &http,
            &Origin::Http("http://127.0.0.1:1/".into()),
            &cache,
        )
        .await;
        assert!(got.is_none());
    }
}
