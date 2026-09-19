//! GitHub releases. No key required, though unauthenticated calls are rate
//! limited to 60/hour per IP.
//!
//! Ids are `owner/repo`. Many authors release here first — RaiderIO's own
//! `.toc` declares `X-AddonProvider: github`.
//!
//! **Flavor selection uses `release.json` when it is present.** The BigWigs
//! packager, which most serious addons use, publishes a `release.json` asset
//! mapping each zip to the flavors it is built for:
//!
//! ```json
//! { "releases": [ { "filename": "WeakAuras-5.22.0.zip",
//!                   "metadata": [ { "flavor": "mists", "interface": 50504 } ] } ] }
//! ```
//!
//! Without it there is nothing to go on but the file name, so a single zip is
//! taken as universal and a multi-zip release picks the one whose name matches
//! the flavor — a guess, and marked as one in the note.

use serde::Deserialize;

use super::{blank, missing, ProviderResult};
use crate::addons::catalog::{Request, Resolved};
use crate::net::Http;

#[derive(Debug, Clone, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseManifest {
    #[serde(default)]
    releases: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    filename: String,
    #[serde(default)]
    nolib: bool,
    #[serde(default)]
    metadata: Vec<ManifestFlavor>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestFlavor {
    flavor: String,
}

pub async fn resolve(http: &Http, requests: &[Request], flavor: &str) -> ProviderResult {
    let mut resolved = Vec::with_capacity(requests.len());

    for request in requests {
        let url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            request.id.trim_matches('/')
        );
        let release: Release = match http.get_json(&url).await {
            Ok(release) => release,
            // A repo with no releases at all is the common case, not a fault —
            // ElvUI's own repo has none.
            Err(e) => {
                resolved.push(missing(&request.id, format!("no GitHub release found ({e})")));
                continue;
            }
        };

        let manifest = fetch_manifest(http, &release).await;
        let picked = pick_asset(&release, manifest.as_ref(), flavor);

        resolved.push(match picked {
            Pick::Exact(asset) | Pick::Guess(asset) => {
                let guessed = matches!(picked, Pick::Guess(_));
                Resolved {
                    name: Some(request.id.clone()),
                    file_id: Some(release.tag_name.clone()),
                    file_name: Some(asset.name.clone()),
                    file_length: Some(asset.size),
                    download_url: Some(asset.browser_download_url.clone()),
                    website_url: release.html_url.clone(),
                    note: guessed.then(|| {
                        "flavor inferred from the file name — this release publishes no release.json"
                            .to_owned()
                    }),
                    ..blank(&request.id)
                }
            }
            Pick::WrongFlavor(mut flavors) => {
                flavors.sort_unstable();
                flavors.dedup();
                missing(
                    &request.id,
                    format!(
                        "release {} has no {flavor} build (it publishes: {})",
                        release.tag_name,
                        flavors.join(", ")
                    ),
                )
            }
            Pick::None => missing(
                &request.id,
                format!("release {} contains no installable zip", release.tag_name),
            ),
        });
    }
    Ok(resolved)
}

/// Fetch the packager's `release.json`, if the release publishes one.
async fn fetch_manifest(http: &Http, release: &Release) -> Option<ReleaseManifest> {
    let asset = release.assets.iter().find(|a| a.name == "release.json")?;
    http.get_json(&asset.browser_download_url).await.ok()
}

/// What choosing an asset produced.
enum Pick<'a> {
    /// Chosen from `release.json`: certain.
    Exact(&'a Asset),
    /// Chosen from the file name because the release publishes no manifest.
    Guess(&'a Asset),
    /// The release declares its flavors and ours is not among them.
    WrongFlavor(Vec<String>),
    /// Nothing installable at all.
    None,
}

/// Choose the asset for `flavor`.
///
/// A release that publishes `release.json` is *authoritative*: if it lists no
/// build for our flavor, there is no build for our flavor, and falling back to
/// "take the only zip" would install a Classic package into a retail install.
/// Real case: WeakAuras 5.22.0 ships one zip declaring `mists, titan, cata,
/// classic, bcc` and no mainline build.
fn pick_asset<'a>(
    release: &'a Release,
    manifest: Option<&ReleaseManifest>,
    flavor: &str,
) -> Pick<'a> {
    let zips: Vec<&Asset> = release
        .assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".zip"))
        .collect();
    if zips.is_empty() {
        return Pick::None;
    }
    let wanted = packager_flavors(flavor);

    if let Some(manifest) = manifest {
        let entry = manifest
            .releases
            .iter()
            // A `nolib` build omits embedded libraries and only works next to
            // a separate library addon; never pick one automatically.
            .filter(|e| !e.nolib)
            .find(|e| e.metadata.iter().any(|m| wanted.contains(&m.flavor.as_str())));

        return match entry.and_then(|e| zips.iter().find(|a| a.name == e.filename)) {
            Some(asset) => Pick::Exact(asset),
            None => Pick::WrongFlavor(
                manifest
                    .releases
                    .iter()
                    .flat_map(|e| e.metadata.iter().map(|m| m.flavor.clone()))
                    .collect(),
            ),
        };
    }

    // No manifest: everything from here is inference.
    if zips.len() == 1 {
        return Pick::Guess(zips[0]);
    }
    zips.iter()
        .find(|a| {
            let name = a.name.to_lowercase();
            wanted.iter().any(|f| name.contains(f))
        })
        .or_else(|| {
            // Nothing matched by name: the plain zip is usually the mainline one.
            zips.iter().find(|a| {
                !["classic", "vanilla", "bcc", "wrath", "cata", "mists"]
                    .iter()
                    .any(|f| a.name.to_lowercase().contains(f))
            })
        })
        .map(|a| Pick::Guess(a))
        .unwrap_or(Pick::None)
}

/// The packager's flavor names for one of our catalog flavors.
///
/// The packager distinguishes every Classic line separately, so our coarser
/// `classic` maps to several of them.
fn packager_flavors(flavor: &str) -> Vec<&'static str> {
    match flavor {
        "retail" => vec!["mainline", "retail"],
        "classic_era" => vec!["classic", "vanilla", "era"],
        "classic" => vec!["mists", "cata", "wrath", "bcc", "tbc"],
        _ => vec!["mainline"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(assets: &[(&str, u64)]) -> Release {
        Release {
            tag_name: "5.22.0".into(),
            html_url: Some("https://github.com/WeakAuras/WeakAuras2".into()),
            assets: assets
                .iter()
                .map(|(name, size)| Asset {
                    name: (*name).to_owned(),
                    browser_download_url: format!("https://example/{name}"),
                    size: *size,
                })
                .collect(),
        }
    }

    /// Trimmed from WeakAuras 5.22.0's real release.json.
    const MANIFEST: &str = r#"{ "releases": [
        { "name": "WeakAuras", "version": "5.22.0", "filename": "WeakAuras-5.22.0.zip",
          "nolib": false,
          "metadata": [ { "flavor": "mainline", "interface": 120100 },
                        { "flavor": "mists", "interface": 50504 },
                        { "flavor": "classic", "interface": 11509 } ] },
        { "filename": "WeakAuras-5.22.0-nolib.zip", "nolib": true, "metadata": [] } ] }"#;

    fn name_of<'a>(pick: &'a Pick<'a>) -> Option<&'a str> {
        match pick {
            Pick::Exact(a) | Pick::Guess(a) => Some(&a.name),
            _ => None,
        }
    }

    #[test]
    fn release_json_makes_the_flavor_choice_exact() {
        let manifest: ReleaseManifest = serde_json::from_str(MANIFEST).unwrap();
        let release = release(&[("release.json", 300), ("WeakAuras-5.22.0.zip", 10_529_827)]);

        let pick = pick_asset(&release, Some(&manifest), "retail");
        assert!(matches!(pick, Pick::Exact(_)), "a release.json match is not a guess");
        assert_eq!(name_of(&pick), Some("WeakAuras-5.22.0.zip"));
    }

    #[test]
    fn a_manifest_without_our_flavor_installs_nothing() {
        // The real case this exists for: WeakAuras 5.22.0 declares
        // mists/titan/cata/classic/bcc and no mainline build. Taking "the only
        // zip" would put a Classic package into a retail install.
        let manifest: ReleaseManifest = serde_json::from_str(
            r#"{"releases":[{"filename":"WeakAuras-5.22.0.zip","nolib":false,
                 "metadata":[{"flavor":"mists"},{"flavor":"classic"}]}]}"#,
        )
        .unwrap();
        let release = release(&[("WeakAuras-5.22.0.zip", 10_529_827)]);

        match pick_asset(&release, Some(&manifest), "retail") {
            Pick::WrongFlavor(flavors) => assert!(flavors.contains(&"mists".to_owned())),
            other => panic!("expected WrongFlavor, got {:?}", name_of(&other)),
        }
        // The same release IS installable for the flavors it does declare.
        assert!(matches!(pick_asset(&release, Some(&manifest), "classic"), Pick::Exact(_)));
    }

    #[test]
    fn a_nolib_build_is_never_chosen_automatically() {
        // nolib packages omit embedded libraries and break on their own.
        let manifest: ReleaseManifest = serde_json::from_str(MANIFEST).unwrap();
        let release = release(&[("WeakAuras-5.22.0-nolib.zip", 100), ("WeakAuras-5.22.0.zip", 200)]);

        let pick = pick_asset(&release, Some(&manifest), "retail");
        assert_eq!(name_of(&pick), Some("WeakAuras-5.22.0.zip"));
    }

    #[test]
    fn without_a_manifest_a_lone_zip_is_a_guess_not_a_certainty() {
        let release = release(&[("Addon-1.0.zip", 500)]);
        let pick = pick_asset(&release, None, "retail");
        assert!(matches!(pick, Pick::Guess(_)));
        assert_eq!(name_of(&pick), Some("Addon-1.0.zip"));
    }

    #[test]
    fn without_a_manifest_several_zips_are_matched_by_name() {
        let release = release(&[("Addon-classic.zip", 1), ("Addon-mainline.zip", 2)]);
        assert_eq!(name_of(&pick_asset(&release, None, "retail")), Some("Addon-mainline.zip"));
        assert_eq!(
            name_of(&pick_asset(&release, None, "classic_era")),
            Some("Addon-classic.zip")
        );
    }

    #[test]
    fn a_release_with_no_zip_resolves_to_nothing() {
        let release = release(&[("release.json", 300), ("source.tar.gz", 900)]);
        assert!(matches!(pick_asset(&release, None, "retail"), Pick::None));
    }
}
