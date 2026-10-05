//! The addon catalog's wire contract, pinned against the proxy that serves it.
//!
//! The fixtures under `fixtures/catalog/` are **generated** by the addon
//! proxy's own test suite (`lightforge-assets/addon-proxy`, `go test ./...`),
//! from its real handlers. They are copied in here rather than written by
//! hand, which is the whole point: one artifact, produced by the server,
//! parsed by the client, so the two cannot drift apart unnoticed.
//!
//! They had drifted. All three of these shipped at once, and none was caught,
//! because a rejected API key meant the proxy never produced a successful
//! response for anyone to parse:
//!
//! - `file_id` was emitted as a JSON number against [`Resolved`]'s
//!   `Option<String>`;
//! - `/search` answered with a bare array against [`SearchReply`]'s envelope;
//! - project ids were numbers against [`Project`]'s `String`.
//!
//! Each one fails the *entire* response rather than a single entry, so any one
//! of them would have emptied a user's addon list rather than degrading it.
//!
//! Regenerate after changing the proxy's output:
//!
//! ```text
//! cd ../lightforge-assets/addon-proxy && go test ./...
//! cp ../fixtures/catalog/*.json <this dir>/fixtures/catalog/
//! ```

use std::path::PathBuf;

use lightforge_core::addons::catalog::{Catalog, Origin, Request, Resolved, Status};
use lightforge_core::net::Http;

fn catalog() -> Catalog {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("catalog");
    Catalog::new(Origin::Dir(dir))
}

fn request(id: &str) -> Request {
    Request {
        provider: "curse".to_owned(),
        id: id.to_owned(),
        version_id: None,
        channel: None,
    }
}

/// The fixture carries one of each outcome on purpose, so the happy path is
/// not the only thing exercised.
#[tokio::test]
async fn resolve_parses_every_outcome() {
    let http = Http::new().expect("client");
    let requests = [request("3358"), request("4242"), request("9999")];

    let resolved = catalog()
        .resolve(&http, "retail", &requests)
        .await
        .expect("the proxy's own output must parse");

    assert_eq!(resolved.len(), 3, "one result per request, in order");

    let ok = &resolved[0];
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.id, "3358");
    assert_eq!(
        ok.file_date.as_deref(),
        Some("2026-09-01T00:00:00Z"),
        "the publish date rides along, so a list of thirty addons is still one request"
    );
    assert_eq!(
        ok.file_id.as_deref(),
        Some("800800"),
        "ids are strings on the wire, including the numeric ones"
    );
    assert_eq!(ok.file_length, Some(1_234_567), "needed to show a size");
    assert_eq!(
        ok.md5.as_deref(),
        Some("0123456789abcdef0123456789abcdef"),
        "CurseForge publishes an MD5 and the download is verified with it"
    );
    assert!(ok.downloadable().is_some());
    assert!(ok.unavailable_reason().is_none());
    assert_eq!(ok.folders, ["BigWigs", "BigWigs_Core"]);
    // Which build this is, in the same vocabulary the request's channel uses.
    // "release" on CurseForge is "stable" here, and the proxy does that
    // translation so the client has one set of names.
    assert_eq!(ok.channel, Some(lightforge_core::addons::Channel::Stable));

    let opted_out = &resolved[1];
    assert_eq!(opted_out.status, Status::OptOut);
    assert!(
        opted_out.downloadable().is_none(),
        "an opt-out must never yield a URL to fetch"
    );
    assert!(
        opted_out.website_url.is_some(),
        "an opt-out has to say where to go instead"
    );
    assert!(opted_out.unavailable_reason().is_some());

    let gone = &resolved[2];
    assert_eq!(gone.status, Status::NotFound);
    assert!(gone.downloadable().is_none());
}

#[tokio::test]
async fn search_parses_the_results_envelope() {
    let http = Http::new().expect("client");

    let hits = catalog()
        .search(&http, "retail", "bigwigs", 10)
        .await
        .expect("search must read `{\"results\": [...]}`, not a bare array");

    // Two: the project the proxy pinned from an exact slug, then the text
    // hit. The pin is the contract — a search for an addon by name must
    // contain that addon, which CurseForge's own text search does not
    // guarantee.
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, "3358", "a project id is a string");
    assert_eq!(hits[0].name, "BigWigs", "the exact slug match comes first");
    assert_eq!(hits[0].authors, ["Author One"]);
    assert!(hits[0].website_url.is_some());

    // Carried as an integer, from a count CurseForge sends in exponential
    // form. Without it every CurseForge hit ranks as equally unpopular.
    assert_eq!(hits[0].download_count, 98_400_000);
}

#[tokio::test]
async fn project_parses() {
    let http = Http::new().expect("client");

    let project = catalog().project(&http, "3358").await.expect("project");

    assert_eq!(project.id, "3358");
    assert_eq!(project.name, "BigWigs");
    assert_eq!(project.summary.as_deref(), Some("BigWigs summary"));
    assert!(project.thumbnail_url.is_some());
}

/// Why the server has to quote its ids, stated as a test rather than a comment.
///
/// This is the exact payload the proxy used to emit. serde rejects an integer
/// for `Option<String>`, and because the field sits inside a list element the
/// failure takes the whole batch down — forty working addons reported as
/// nothing at all, not thirty-nine plus one bad entry.
///
/// Deliberately *not* fixed by making the field lenient: the strictness is
/// what makes the generated fixtures a real check. A tolerant deserializer
/// would absorb server drift silently, which is how this went unnoticed for
/// two weeks in the first place.
#[test]
fn a_numeric_file_id_is_rejected_rather_than_coerced() {
    let numeric = r#"{"id":"3358","status":"ok","file_id":800800}"#;
    assert!(
        serde_json::from_str::<Resolved>(numeric).is_err(),
        "a bare number must not be silently accepted as a file id"
    );

    let quoted = r#"{"id":"3358","status":"ok","file_id":"800800"}"#;
    let parsed: Resolved = serde_json::from_str(quoted).expect("the agreed shape");
    assert_eq!(parsed.file_id.as_deref(), Some("800800"));
}

/// The files endpoint is what a version picker is built on: resolve returns
/// the file the proxy chose, this returns the ones it chose between.
#[tokio::test]
async fn files_parse_into_a_list_of_choices() {
    let http = Http::new().expect("client");

    let files = catalog()
        .files(&http, "3358", "retail", 20)
        .await
        .expect("the proxy's own output must parse");

    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.file_id, "800800", "a string here too, not the number upstream sends");
    assert_eq!(
        file.display_name.as_deref(),
        Some("v1.2.3"),
        "what the author calls the build, which is what a picker lists"
    );
    assert_eq!(
        file.release_type.as_deref(),
        Some("release"),
        "the provider's own vocabulary, not the channel names we request with"
    );
    assert_eq!(file.file_date.as_deref(), Some("2026-09-01T00:00:00Z"));
    assert_eq!(file.file_length, Some(1_234_567));
    assert_eq!(file.folders, ["BigWigs", "BigWigs_Core"]);
}
