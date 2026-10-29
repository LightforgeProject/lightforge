//! Tukui, through their v1 API. No key required.
//!
//! The only source of **ElvUI**, which is not on CurseForge. From the live API:
//!
//! - **Download URLs are tokenised** per request: fetch fresh, never store.
//! - One package covers every game version.
//! - The response declares its folders; the archive still wins.

use serde::Deserialize;

use super::{blank, missing, ProviderResult};
use crate::addons::catalog::{Request, Resolved};
use crate::net::Http;

const ADDONS_URL: &str = "https://api.tukui.org/v1/addons";

#[derive(Debug, Clone, Deserialize)]
struct Addon {
    id: i64,
    slug: String,
    name: String,
    version: String,
    /// Tokenised, single-use-ish download URL.
    url: String,
    #[serde(default)]
    directories: Vec<String>,
    #[serde(default)]
    web_url: Option<String>,
}

/// Resolve against the full addon list: one request answers every id.
pub async fn resolve(http: &Http, requests: &[Request]) -> ProviderResult {
    let addons: Vec<Addon> = http.get_json(ADDONS_URL).await?;

    Ok(requests
        .iter()
        .map(|request| {
            // Ids can be negative (ElvUI is -2); the slug works too.
            let found = addons.iter().find(|a| {
                a.slug.eq_ignore_ascii_case(&request.id) || a.id.to_string() == request.id
            });

            let Some(addon) = found else {
                return missing(&request.id, "no such addon on Tukui");
            };

            Resolved {
                name: Some(addon.name.clone()),
                // Tukui exposes no file id; the version is what an update
                // compares against.
                file_id: Some(addon.version.clone()),
                file_name: Some(format!("{}-{}.zip", addon.slug, addon.version)),
                download_url: Some(addon.url.clone()),
                folders: addon.directories.clone(),
                website_url: addon.web_url.clone(),
                ..blank(&request.id)
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the live response for ElvUI.
    const ELVUI: &str = r#"[{
        "id": -2, "slug": "elvui", "author": "Elv", "name": "ElvUI",
        "url": "https://api.tukui.org/v1/download/elvui/VCv-XgT_GElUYXhiZc8V",
        "version": "15.26",
        "patch": ["12.1.0", "5.5.4", "1.15.9", "2.5.6"],
        "web_url": "https://tukui.org/elvui",
        "directories": ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"]
    }]"#;

    #[test]
    fn elvui_resolves_by_slug_and_by_its_negative_id() {
        let addons: Vec<Addon> = serde_json::from_str(ELVUI).unwrap();
        assert_eq!(addons[0].id, -2);

        for key in ["elvui", "ElvUI", "-2"] {
            let found = addons
                .iter()
                .find(|a| a.slug.eq_ignore_ascii_case(key) || a.id.to_string() == key);
            assert!(found.is_some(), "{key} should resolve");
        }
    }

    #[test]
    fn the_declared_folders_come_through() {
        let addons: Vec<Addon> = serde_json::from_str(ELVUI).unwrap();
        assert_eq!(addons[0].directories, ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"]);
    }

    #[test]
    fn the_extra_fields_tukui_sends_are_ignored_rather_than_fatal() {
        // The live response carries a dozen fields we do not model.
        let addons: Vec<Addon> = serde_json::from_str(ELVUI).unwrap();
        assert_eq!(addons[0].version, "15.26");
    }
}
