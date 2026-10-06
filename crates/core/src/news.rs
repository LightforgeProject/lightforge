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

use crate::net::{ETagCache, Http};

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
/// Never returns an error for a network problem — only for a cache that is
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feed_url_sits_under_its_own_namespace() {
        let origin = Origin::Http("https://lightforge.tuxcore.pro/".into());
        assert_eq!(origin.url(), "https://lightforge.tuxcore.pro/news/v1/feed");
    }

    /// A base without the trailing slash must not produce a doubled one, and
    /// one with several must not either — both have reached the addon catalog
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
    /// nothing to show — and it must be `None`, not a panic or an empty feed
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
