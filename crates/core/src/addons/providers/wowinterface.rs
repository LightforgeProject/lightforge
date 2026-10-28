//! WoWInterface, through the MMOUI API. No key required.
//!
//! **Pinned to `v3`**: v4's `filedetails` answers 404. Ids are a `.toc`'s
//! `X-WoWI-ID`. The only provider returning an **MD5** of the package.

use serde::Deserialize;

use super::{blank, missing, ProviderResult};
use crate::addons::catalog::Request;
use crate::net::Http;

/// Several ids can be fetched in one request, comma-separated, verified
/// against the live API.
const BATCH: usize = 25;

fn details_url(ids: &str) -> String {
    format!("https://api.mmoui.com/v3/game/WOW/filedetails/{ids}.json")
}

/// The fields we use.
#[derive(Debug, Clone, Deserialize)]
struct Details {
    #[serde(rename = "UID")]
    uid: String,
    #[serde(rename = "UIName")]
    name: Option<String>,
    #[serde(rename = "UIVersion")]
    version: Option<String>,
    #[serde(rename = "UIFileName")]
    file_name: Option<String>,
    #[serde(rename = "UIDownload")]
    download: Option<String>,
    #[serde(rename = "UIMD5")]
    md5: Option<String>,
}

pub async fn resolve(http: &Http, requests: &[Request]) -> ProviderResult {
    let mut resolved = Vec::with_capacity(requests.len());

    for chunk in requests.chunks(BATCH) {
        let ids: Vec<&str> = chunk.iter().map(|r| r.id.as_str()).collect();
        let details: Vec<Details> = http.get_json(&details_url(&ids.join(","))).await?;

        for request in chunk {
            let Some(found) = details.iter().find(|d| d.uid == request.id) else {
                resolved.push(missing(&request.id, "no such addon on WoWInterface"));
                continue;
            };

            resolved.push(Resolved {
                name: found.name.clone(),
                // No per-file id; the version plays that role.
                file_id: found.version.clone(),
                file_name: found.file_name.clone(),
                md5: found.md5.clone(),
                download_url: found.download.clone(),
                website_url: Some(format!("https://www.wowinterface.com/downloads/info{}", found.uid)),
                ..blank(&request.id)
            });
        }
    }
    Ok(resolved)
}

use crate::addons::catalog::Resolved;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_v3_endpoint_is_the_one_that_answers() {
        // v4's filedetails 404s.
        assert!(details_url("5086").starts_with("https://api.mmoui.com/v3/"));
        assert!(details_url("5086").ends_with("filedetails/5086.json"));
    }

    #[test]
    fn several_ids_share_one_request() {
        assert_eq!(
            details_url("5086,14403"),
            "https://api.mmoui.com/v3/game/WOW/filedetails/5086,14403.json"
        );
    }

    #[test]
    fn a_real_payload_maps_onto_our_shape() {
        // Trimmed from the live response for BigWigs (UID 5086).
        let json = br#"[{"UID":"5086","UIName":"BigWigs (Boss Timers & Tools)",
            "UIVersion":"v424.8","UIFileName":"BigWigs-v424.8.zip",
            "UIDownload":"https://cdn.wowinterface.com/downloads/getfile.php?id=5086",
            "UIMD5":"f02f41bd451882f08907fed3a2b1a4e5"}]"#;
        let details: Vec<Details> = serde_json::from_slice(json).unwrap();

        assert_eq!(details[0].uid, "5086");
        assert_eq!(details[0].version.as_deref(), Some("v424.8"));
        // The MD5 makes this provider's downloads verifiable.
        assert_eq!(details[0].md5.as_deref(), Some("f02f41bd451882f08907fed3a2b1a4e5"));
    }
}
