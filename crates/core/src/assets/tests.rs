use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::*;
use crate::net::Http;
use crate::text::hex;
use crate::product::{Branch, ProductCode};

const FIXTURE: &str = r#"{
  "schema": 1,
  "default": "wow",
  "products": {
    "wow":             { "label": "Retail",
                         "logo":       { "url": "retail-logo.png" },
                         "background": { "url": "retail-bg.jpg" } },
    "wow_classic_era": { "label": "Classic Era",
                         "logo":       { "url": "era-logo.png" },
                         "background": { "url": "era-bg.jpg" } }
  }
}"#;

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightforge-assets-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest() -> Manifest {
    Manifest::parse(FIXTURE.as_bytes()).unwrap()
}

#[test]
fn labels_are_per_product_and_stable() {
    let m = manifest();
    assert_eq!(m.theme(&ProductCode::Retail).unwrap().label, "Retail");
    assert_eq!(m.theme(&ProductCode::ClassicEra).unwrap().label, "Classic Era");
}

#[test]
fn unnamed_products_fall_back_to_the_default() {
    let m = manifest();
    // Retail's PTR and a user-added custom product both theme as Retail,
    // the PTR because retail *is* its line's head, the custom code because
    // nothing is known about what it is a branch of.
    assert_eq!(m.theme(&ProductCode::Ptr).unwrap().label, "Retail");
    let custom = ProductCode::Custom("turtle_wow".into());
    assert_eq!(m.theme(&custom).unwrap().label, "Retail");
}

/// The bug this prevents: Era PTR and Classic PTR wearing retail's logo
/// and backdrop because `default` is what they fell through to.
#[test]
fn a_ptr_the_manifest_does_not_name_wears_its_own_lines_art() {
    let m = manifest();
    assert_eq!(m.theme(&ProductCode::EraPtr).unwrap().label, "Classic Era");
    assert_eq!(
        m.slot(&ProductCode::EraPtr, |t| t.logo.as_ref()).map(|a| a.url.as_str()),
        Some("era-logo.png"),
    );
    assert_eq!(
        m.slot(&ProductCode::EraPtr, |t| t.background.as_ref()).map(|a| a.url.as_str()),
        Some("era-bg.jpg"),
    );
}

/// A line whose head the manifest does not name either still reaches
/// `default`: the head must not resolve to itself and dead-end.
#[test]
fn a_line_with_no_art_of_its_own_still_reaches_the_default() {
    let m = manifest();
    assert_eq!(m.theme(&ProductCode::ClassicPtr).unwrap().label, "Retail");
}

/// Forever's art is published against the beta, and must stay there while
/// 1.0.0-beta.3 is in use: that build does not know `wowf` and would
/// install it into `wowf/` the moment a manifest named it.
#[test]
fn a_head_with_no_art_borrows_a_sibling_branch() {
    let body = FIXTURE.replace(
        r#""wow_classic_era": {"#,
        r#""wow_classic_beta": { "label": "WoW Forever",
                                 "logo":       { "url": "forever-logo.png" },
                                 "background": { "url": "forever-bg.jpg" } },
        "wow_classic_era": {"#,
    );
    let m = Manifest::parse(body.as_bytes()).unwrap();
    assert_eq!(
        m.slot(&ProductCode::Forever, |t| t.logo.as_ref()).map(|a| a.url.as_str()),
        Some("forever-logo.png"),
    );
}

#[test]
fn a_default_naming_a_missing_product_is_not_a_theme() {
    let mut m = manifest();
    m.default = Some("nonexistent".into());
    // A custom code, not a WoW branch: a branch would reach its line's
    // head and be themed after all, which is the point of `head_theme`.
    assert!(m.theme(&ProductCode::Custom("turtle_wow".into())).is_none());
}

#[test]
fn a_newer_schema_is_refused_rather_than_guessed_at() {
    let next = SCHEMA_VERSION + 1;
    let body = FIXTURE.replace("\"schema\": 1", &format!("\"schema\": {next}"));
    assert!(matches!(
        Manifest::parse(body.as_bytes()),
        Err(AssetError::UnsupportedSchema { found, supported })
            if found == next && supported == SCHEMA_VERSION
    ));
}

/// Schema 1 manifests still parse: the fields went optional, not away.
#[test]
fn a_schema_1_manifest_still_reads() {
    let m = Manifest::parse(FIXTURE.as_bytes()).expect("schema 1 is still readable");
    let theme = m.theme(&ProductCode::Retail).unwrap();
    assert!(theme.logo.is_some());
    assert!(theme.background.is_some());
}

/// What schema 2 exists for: a product themed with nothing but its mark.
#[test]
fn a_mark_only_theme_parses_and_has_no_branding() {
    let body = r#"{
        "schema": 2,
        "products": {
            "d3": { "label": "Diablo III",
                    "icon": { "url": "d3-icon.1234abcd.png" } }
        }
    }"#;
    let m = Manifest::parse(body.as_bytes()).expect("a mark-only theme is valid");
    let theme = m.theme(&ProductCode::from("d3".to_owned())).unwrap();
    assert_eq!(theme.label, "Diablo III");
    assert!(theme.logo.is_none());
    assert!(theme.background.is_none());
    assert!(theme.icon.is_some());
    // Its one URL still has to be prunable, or the mark would be deleted
    // from the cache on the next manifest fetch.
    assert_eq!(
        m.urls("https://x/assets/manifest.json"),
        ["https://x/assets/d3-icon.1234abcd.png"]
    );
}

#[test]
fn unknown_manifest_fields_are_tolerated() {
    // Future metadata must not break older clients, which is the whole
    // reason the URLs live in a manifest instead of being hard-coded.
    let body = FIXTURE.replace("\"schema\": 1", "\"schema\": 1, \"banners\": []");
    assert!(Manifest::parse(body.as_bytes()).is_ok());
}

#[test]
fn missing_sha256_is_optional() {
    let m = manifest();
    assert!(m.theme(&ProductCode::Retail).unwrap().logo.as_ref().unwrap().sha256.is_none());
}

#[test]
fn cache_names_are_url_derived_and_keep_the_extension() {
    let a = Store::cache_name("https://cdn.example/a/retail-logo.png");
    let b = Store::cache_name("https://cdn.example/b/retail-logo.png");
    assert_ne!(a, b, "same file name in different directories must not collide");
    assert!(a.ends_with(".png"));
    assert_eq!(a.len(), 16 + ".png".len());
    // An extension the app does not itself understand still survives to
    // the cache file. It has to: the decoder picks its format from that
    // extension, so an `.ico` cached without one cannot be read back.
    assert!(Store::cache_name("https://cdn.example/retail-icon.ico").ends_with(".ico"));
    assert!(Store::cache_name("https://cdn.example/retail-icon.ICO").ends_with(".ico"));

    // Query strings are not part of the extension, and it is normalized.
    assert!(Store::cache_name("https://cdn.example/bg.JPG?v=2").ends_with(".jpg"));
}

#[test]
fn urls_join_the_way_a_browser_would() {
    let base = "https://assets.tuxcore.pro/wow/manifest.json";
    assert_eq!(join(base, "logo.png"), "https://assets.tuxcore.pro/wow/logo.png");
    assert_eq!(join(base, "/art/logo.png"), "https://assets.tuxcore.pro/art/logo.png");
    assert_eq!(join(base, "https://other/logo.png"), "https://other/logo.png");
    // A base given as a directory keeps its trailing segment.
    assert_eq!(
        join("https://assets.tuxcore.pro/wow/", "logo.png"),
        "https://assets.tuxcore.pro/wow/logo.png"
    );
}

#[test]
fn a_checksum_mismatch_is_an_error_not_a_cache_entry() {
    let asset = AssetRef {
        url: "logo.png".into(),
        sha256: Some("00".repeat(32)),
        height: None,
    };
    let err = verify(b"bytes", &asset, Slot::Logo, "logo.png").unwrap_err();
    assert!(matches!(err, AssetError::Checksum { slot: Slot::Logo, .. }));
}

#[test]
fn a_matching_checksum_passes_in_either_case() {
    let bytes = b"artwork";
    let digest = hex(&Sha256::digest(bytes));
    let asset = AssetRef {
        url: "logo.png".into(),
        sha256: Some(digest.to_uppercase()),
        height: None,
    };
    assert!(verify(bytes, &asset, Slot::Logo, "logo.png").is_ok());
}

/// A mark of its own must not cost a product its inherited artwork.
///
/// Anniversary had no entry at all and so fell back to Retail's logo and
/// background. Giving it an icon made the entry exist, and a per-theme
/// fallback would have dropped it to no art whatsoever, a worse outcome
/// produced by adding art.
#[test]
fn a_mark_only_entry_still_inherits_the_default_artwork() {
    let body = r#"{
        "schema": 2,
        "default": "wow",
        "products": {
            "wow": { "label": "Retail",
                     "logo": { "url": "l.png" },
                     "background": { "url": "b.jpg" } },
            "wow_anniversary": { "label": "Anniversary",
                                 "icon": { "url": "a-icon.png" } }
        }
    }"#;
    let m = Manifest::parse(body.as_bytes()).unwrap();
    let anniversary = ProductCode::from("wow_anniversary".to_owned());

    // Its own mark …
    assert_eq!(
        m.own_theme(&anniversary).unwrap().icon.as_ref().unwrap().url,
        "a-icon.png"
    );
    // … and Retail's artwork behind it.
    assert_eq!(m.slot(&anniversary, |t| t.logo.as_ref()).unwrap().url, "l.png");
    assert_eq!(m.slot(&anniversary, |t| t.background.as_ref()).unwrap().url, "b.jpg");

    // A hand-off with a mark and no default artwork to inherit is still
    // mark-only; nothing invents art for it.
    let d3 = ProductCode::from("d3".to_owned());
    assert!(m.own_theme(&d3).is_none());
}

/// A mark is identity: a product with no entry of its own must show no
/// mark rather than another game's. This put World of Warcraft's icon
/// beside "Hearthstone" in the browse list, because Battle.net calls
/// Hearthstone `hs_beta` on a machine where it is installed and the
/// manifest entry was under `hsb`.
#[test]
fn a_product_without_its_own_entry_has_no_mark() {
    let m = manifest();
    let stranger = ProductCode::from("hs_beta".to_owned());

    // The default fallback still answers for logo and background …
    assert!(m.theme(&stranger).is_some());
    // … but never lends another product its mark.
    assert!(m.own_theme(&stranger).is_none());
}

#[test]
fn prune_keeps_what_the_manifest_names_and_drops_the_rest() {
    let dir = std::env::temp_dir().join("lightforge-prune-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::new(&dir);

    let live = "https://x/assets/retail-bg.0080bd8a.jpg".to_owned();
    let stale = "https://x/assets/retail-bg.deadbeef.jpg".to_owned();
    std::fs::write(store.path_for(&live), b"live").unwrap();
    std::fs::write(store.path_for(&stale), b"stale").unwrap();
    std::fs::write(dir.join(MANIFEST_FILE), b"{}").unwrap();
    // Something this module did not write. A cache directory is still
    // someone's disk; a prune that reaches past its own files is worse
    // than the leak it fixes.
    std::fs::write(dir.join("notes.txt"), b"not ours").unwrap();

    // The ETag sidecars the fetcher writes beside each body.
    std::fs::write(format!("{}.etag", store.path_for(&live).display()), b"\"v1\"").unwrap();
    std::fs::write(format!("{}.etag", store.path_for(&stale).display()), b"\"v2\"").unwrap();
    std::fs::write(dir.join(format!("{MANIFEST_FILE}.etag")), b"\"m\"").unwrap();

    // Two gone: the stale body and its validator.
    assert_eq!(store.prune(std::slice::from_ref(&live)), 2);
    assert!(
        std::path::Path::new(&format!("{}.etag", store.path_for(&live).display())).is_file(),
        "a live asset must keep its validator, or the next launch re-downloads it"
    );
    assert!(
        dir.join(format!("{MANIFEST_FILE}.etag")).is_file(),
        "the manifest's own validator is not art"
    );
    assert!(store.path_for(&live).is_file(), "live art must survive");
    assert!(!store.path_for(&stale).is_file(), "stale art must go");
    assert!(dir.join(MANIFEST_FILE).is_file(), "the manifest is not art");
    assert!(dir.join("notes.txt").is_file(), "foreign files are not ours to delete");

    // Idempotent: nothing left to remove.
    assert_eq!(store.prune(&[live]), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_names_this_module_produces_are_prunable() {
    assert!(is_cache_name(&Store::cache_name("https://x/a.jpg")));
    assert!(is_cache_name("0123456789abcdef.png"));
    assert!(is_cache_name("0123456789abcdef"));
    assert!(!is_cache_name("0123456789ABCDEF.png"), "uppercase is not ours");
    assert!(!is_cache_name("retail-bg.jpg"));
    assert!(!is_cache_name("manifest.json"));
    assert!(!is_cache_name("0123456789abcde.png"), "fifteen characters");
}

/// The URL set is what pruning keeps, so a product whose icon is absent
/// must not silently drop its logo and background too.
#[test]
fn manifest_urls_cover_every_slot_and_resolve_relative() {
    let manifest = Manifest {
        schema: SCHEMA_VERSION,
        default: None,
        products: BTreeMap::from([(
            "wow".to_owned(),
            Theme {
                label: "Retail".into(),
                line: None,
                branch: None,
                logo: Some(AssetRef { url: "l.png".into(), sha256: None, height: None }),
                background: Some(AssetRef { url: "b.jpg".into(), sha256: None, height: None }),
                icon: None,
            },
        )]),
    };

    let urls = manifest.urls("https://x/assets/manifest.json");
    assert_eq!(urls, ["https://x/assets/l.png", "https://x/assets/b.jpg"]);
}

#[tokio::test]
async fn a_fixture_resolves_without_touching_the_network() {
    let dir = tmpdir("fixture");
    let logo = b"PNG-logo".to_vec();
    std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
    std::fs::write(dir.join("retail-logo.png"), &logo).unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir.clone()), Store::new(tmpdir("fixture-cache")));
    let http = Http::new().unwrap();
    let branding = pipeline.resolve(&http, &ProductCode::Retail).await.unwrap();

    assert_eq!(branding.label, "Retail");
    assert_eq!(branding.logo.path, dir.join("retail-logo.png"));
    assert!(!branding.logo.fetched);
    assert_eq!(std::fs::read(&branding.logo.path).unwrap(), logo);
}

#[test]
fn a_manifest_states_the_catalog_over_the_builtin_one() {
    let body = r#"{
        "schema": 1,
        "products": {
          "wow_midnight_ptr": {
            "label": "Midnight PTR",
            "line": "retail",
            "branch": "ptr",
            "logo": { "url": "l.png" },
            "background": { "url": "b.jpg" }
          }
        }
    }"#;
    let catalog = Manifest::parse(body.as_bytes()).unwrap().catalog();

    // A code this build never heard of, placed in an existing line.
    let code = ProductCode::Custom("wow_midnight_ptr".into());
    let entry = catalog.get(&code).unwrap();
    assert_eq!(entry.line, crate::product::ProductLine::Retail);
    assert_eq!(entry.branch, Branch::Ptr);
    assert_eq!(entry.label, "Midnight PTR");

    // And it joins retail's version list rather than standing alone,
    // which is the whole point of stating it.
    let retail = catalog.branches(&crate::product::ProductLine::Retail);
    assert!(retail.iter().any(|e| e.code == code));
    assert!(retail.iter().any(|e| e.code == ProductCode::Retail));

    // Everything the binary knows is still there, unmentioned.
    assert!(catalog.get(&ProductCode::ClassicEra).is_some());
}

#[test]
fn a_published_code_that_is_not_a_folder_name_is_left_out() {
    let body = r#"{
        "schema": 1,
        "products": {
          "../wow": { "label": "Escape", "logo": { "url": "l.png" }, "background": { "url": "b.jpg" } },
          "wow_midnight_ptr": { "label": "Midnight PTR", "logo": { "url": "l.png" }, "background": { "url": "b.jpg" } }
        }
    }"#;
    let catalog = Manifest::parse(body.as_bytes()).unwrap().catalog();

    assert!(catalog.get(&ProductCode::Custom("../wow".into())).is_none());
    assert!(catalog.get(&ProductCode::Custom("wow_midnight_ptr".into())).is_some());
}

#[test]
fn a_branch_the_client_cannot_render_falls_back_instead_of_vanishing() {
    let body = r#"{
        "schema": 1,
        "products": {
          "wowt": {
            "label": "Retail PTR",
            "branch": "pre-alpha",
            "logo": { "url": "l.png" },
            "background": { "url": "b.jpg" }
          }
        }
    }"#;
    let catalog = Manifest::parse(body.as_bytes()).unwrap().catalog();
    let entry = catalog.get(&ProductCode::Ptr).unwrap();
    // The stated branch is nonsense, so the code's own answer stands.
    assert_eq!(entry.branch, Branch::Ptr);
    assert_eq!(entry.label, "Retail PTR");
}

#[test]
fn a_theme_without_an_icon_is_still_a_theme() {
    let dir = tmpdir("no-icon");
    std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("no-icon-c")));
    let branding = pipeline.cached(&ProductCode::Retail).unwrap();
    assert!(branding.icon.is_none());
}

#[test]
fn a_declared_icon_resolves_beside_the_rest() {
    let dir = tmpdir("icon");
    let body = FIXTURE.replace(
        "\"background\": { \"url\": \"retail-bg.jpg\" }",
        "\"background\": { \"url\": \"retail-bg.jpg\" }, \"icon\": { \"url\": \"retail-icon.png\" }",
    );
    std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();
    std::fs::write(dir.join("retail-icon.png"), b"PNG-icon").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir.clone()), Store::new(tmpdir("icon-c")));
    let manifest = pipeline.cached_manifest().unwrap();

    let icon = pipeline.cached_icon(&manifest, &ProductCode::Retail).unwrap();
    assert_eq!(icon.path, dir.join("retail-icon.png"));
    assert_eq!(
        pipeline.cached(&ProductCode::Retail).unwrap().icon,
        Some(icon)
    );
}

/// The cache-first and the revalidated branding are compared to decide
/// whether to swap the art. A product on a borrowed theme (live Forever
/// wearing the beta's) must read the same icon both ways, which is none.
#[test]
fn a_borrowed_theme_does_not_lend_its_icon_to_the_cached_branding() {
    let dir = tmpdir("borrowed");
    let body = r#"{
        "schema": 2,
        "default": "wow",
        "products": {
          "wow": { "label": "Retail",
                   "logo": { "url": "retail-logo.png" },
                   "background": { "url": "retail-bg.jpg" } },
          "wow_classic_beta": { "label": "WoW Forever",
                   "logo": { "url": "retail-logo.png" },
                   "background": { "url": "retail-bg.jpg" },
                   "icon": { "url": "forever-icon.png" } }
        }
    }"#;
    std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();
    std::fs::write(dir.join("forever-icon.png"), b"PNG-icon").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("borrowed-c")));
    let manifest = pipeline.cached_manifest().unwrap();

    let branding = pipeline.cached(&ProductCode::Forever).unwrap();
    assert_eq!(branding.label, "WoW Forever", "the art is still borrowed");
    assert_eq!(branding.icon, pipeline.cached_icon(&manifest, &ProductCode::Forever));
    assert!(branding.icon.is_none());
    // The beta's own mark is its own.
    assert!(pipeline.cached(&ProductCode::ClassicBeta).unwrap().icon.is_some());
}

#[test]
fn a_declared_icon_that_is_missing_costs_only_the_icon() {
    let dir = tmpdir("icon-gone");
    let body = FIXTURE.replace(
        "\"background\": { \"url\": \"retail-bg.jpg\" }",
        "\"background\": { \"url\": \"retail-bg.jpg\" }, \"icon\": { \"url\": \"retail-icon.png\" }",
    );
    std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();
    // No icon file: the bar falls back, the theme survives.

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("icon-gone-c")));
    let branding = pipeline.cached(&ProductCode::Retail).unwrap();
    assert_eq!(branding.label, "Retail");
    assert!(branding.icon.is_none());
}

#[test]
fn a_logo_height_hint_survives_the_round_trip() {
    let dir = tmpdir("logo-height");
    let body = FIXTURE.replace(
        "\"url\": \"retail-logo.png\"",
        "\"url\": \"retail-logo.png\", \"height\": 150.0",
    );
    std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("logo-height-c")));
    let branding = pipeline.cached(&ProductCode::Retail).unwrap();

    assert_eq!(branding.logo.height, Some(150.0));
    // A background has no use for one, and none was given.
    assert_eq!(branding.background.height, None);
}

#[test]
fn a_manifest_without_height_hints_still_parses() {
    let manifest = Manifest::parse(FIXTURE.as_bytes()).unwrap();
    let theme = manifest.theme(&ProductCode::Retail).unwrap();
    assert_eq!(theme.logo.as_ref().unwrap().height, None);
}

#[test]
fn the_cache_paints_before_the_network_is_asked() {
    let dir = tmpdir("cached-hit");
    std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir.clone()), Store::new(tmpdir("cached-hit-c")));
    let branding = pipeline.cached(&ProductCode::Retail).unwrap();

    assert_eq!(branding.label, "Retail");
    assert_eq!(branding.logo.path, dir.join("retail-logo.png"));
    assert!(!branding.logo.fetched);
}

#[test]
fn half_a_cache_is_no_cache() {
    let dir = tmpdir("cached-partial");
    std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
    // Logo only: the background was never fetched.
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("cached-partial-c")));
    assert!(pipeline.cached(&ProductCode::Retail).is_none());
}

#[test]
fn the_cache_refuses_bytes_a_fetch_would_have_refused() {
    let dir = tmpdir("cached-checksum");
    // A checksum the files cannot match, standing in for truncation.
    let body = FIXTURE.replace(
        "\"url\": \"retail-logo.png\"",
        "\"url\": \"retail-logo.png\", \"sha256\": \"00\"",
    );
    std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
    std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
    std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("cached-checksum-c")));
    assert!(pipeline.cached(&ProductCode::Retail).is_none());
}

#[test]
fn an_empty_cache_is_not_an_error() {
    let pipeline = Pipeline::new(
        Origin::Http("https://assets.example/wow/".to_owned()),
        Store::new(tmpdir("cached-cold")),
    );
    assert!(pipeline.cached(&ProductCode::Retail).is_none());
}

#[tokio::test]
async fn a_fixture_pointing_at_the_network_is_refused() {
    let dir = tmpdir("fixture-remote");
    let body = FIXTURE.replace("\"url\": \"retail-logo.png\"", "\"url\": \"https://x/l.png\"");
    std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();

    let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("fixture-remote-cache")));
    let http = Http::new().unwrap();
    let err = pipeline.resolve(&http, &ProductCode::Retail).await.unwrap_err();
    assert!(matches!(err, AssetError::RemoteInFixture { .. }));
}
