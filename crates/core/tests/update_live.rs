//! The update check against the real GitHub API.
//!
//! **Ignored by default**: one request against GitHub's 60/hour budget.
//!
//! ```text
//! cargo test -p lightforge-core --test update_live -- --ignored --nocapture
//! ```
//!
//! Proves the endpoint answers, the JSON has `Release`'s fields, and a real
//! tag like `v1.0.0-beta.2` parses.

use lightforge_core::net::{ETagCache, Http};
use lightforge_core::update;

#[tokio::test]
#[ignore = "makes a live request to api.github.com"]
async fn the_published_releases_parse_and_order() {
    let http = Http::new().expect("an http client");
    let dir = std::env::temp_dir().join(format!("lightforge-update-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cache = ETagCache::new(dir.join("releases.json"));

    let releases = update::fetch(&http, &cache).await.expect("GitHub answers");
    assert!(!releases.is_empty(), "the repository has published releases");

    println!("\npublished releases:");
    for release in &releases {
        println!(
            "  {:<20} prerelease={:<5} {}",
            release.tag_name, release.prerelease, release.html_url
        );
    }

    // Every tag has to parse once the `v` comes off, or the comparison below
    // silently skips it and the check goes quiet for everyone.
    for release in &releases {
        let bare = release.tag_name.trim_start_matches('v');
        assert!(
            semver::Version::parse(bare).is_ok(),
            "{} is not a version the check can order",
            release.tag_name
        );
    }

    // The oldest published release must see every later one.
    let oldest = releases.last().expect("at least one release");
    let from_oldest = oldest.tag_name.trim_start_matches('v');
    println!("\nas {from_oldest}: {:?}", update::newer_than(from_oldest, &releases));

    // And the newest must see nothing, which is the case that goes wrong
    // quietly: a check that always fires is a check nobody reads.
    let newest = releases.first().expect("at least one release");
    let from_newest = newest.tag_name.trim_start_matches('v');
    println!("as {from_newest}: {:?}", update::newer_than(from_newest, &releases));
    assert!(
        update::newer_than(from_newest, &releases).is_none(),
        "the newest published release must not be told it is out of date"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
