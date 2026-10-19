//! The addon catalog: Lightforge's view of CurseForge, through the Tuxcore
//! proxy.
//!
//! The proxy exists because the CurseForge API needs a key, and a key cannot
//! live in a GPL repo or in a binary anyone can unpack. It holds the key, adds
//! caching and rate limiting, and hands back a trimmed shape of its own, so
//! this module never talks to CurseForge, and nothing here has to change if the
//! upstream API does.
//!
//! Two outcomes are normal rather than failures, and both are modelled as
//! [`Status`] instead of errors:
//!
//! - **`opt_out`**: an author can switch off third-party distribution, and the
//!   API then returns no download URL. The only correct response is to send the
//!   user to the project page. Never synthesise a CDN URL around it.
//! - **`not_found`**: a project id in someone's shared list that no longer
//!   exists. One dead entry must not fail the other nineteen.
//!
//! File bytes are fetched straight from CurseForge's CDN using the URL the
//! proxy returns; they never transit Tuxcore.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::richtext;

use crate::net::{Http, NetError};

/// Path prefix of the proxy API, versioned so the server can move on.
pub const API_PREFIX: &str = "addons/v1";

/// Most entries a single resolve request may carry, matching the server's cap.
pub const RESOLVE_LIMIT: usize = 50;

/// Where the catalog is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Base URL of the proxy, e.g. `https://lightforge.tuxcore.pro/`.
    Http(String),
    /// A fixture directory, for building and testing against without a server:
    /// `resolve.json`, `search.json`, `project-<id>.json`.
    Dir(PathBuf),
}

/// What became of one requested addon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Ok,
    /// The author disabled third-party distribution.
    OptOut,
    /// No such project.
    NotFound,
}

/// One resolved addon, ready to download, or a reason it is not.
/// `Default` exists so a field the catalog starts reporting does not mean
/// editing every construction that never cared about it, the same reason
/// [`crate::addons::Record`] has one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub id: String,
    pub status: Status,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub file_length: Option<u64>,
    /// Package MD5, when the provider publishes one. Only WoWInterface does
    /// today, and it is the one provider whose downloads can be verified.
    #[serde(default)]
    pub md5: Option<String>,
    /// Absent for anything but [`Status::Ok`].
    #[serde(default)]
    pub download_url: Option<String>,
    /// Addon folders the package contains, when the catalog knows them.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Where to send the user when the download is unavailable.
    #[serde(default)]
    pub website_url: Option<String>,
    /// The author's own page at the provider, when it names one.
    ///
    /// Carried on the resolve reply so an installed addon can offer it
    /// without a second request; see [`crate::addons::install::Record`],
    /// which is where it ends up.
    #[serde(default)]
    pub author_url: Option<String>,
    /// The project's source repository, when the provider publishes one.
    ///
    /// Worth having beside [`Resolved::author_url`] because this is where
    /// sponsorship actually lives: a CurseForge author profile takes no
    /// money, a GitHub repository usually has a Sponsor button.
    #[serde(default)]
    pub source_url: Option<String>,
    /// When the provider published this file, RFC 3339 as it states it.
    ///
    /// Carried on the resolve reply rather than fetched per addon: the files
    /// endpoint answers this for one project, and an installed list of thirty
    /// addons would be thirty requests for one column.
    #[serde(default)]
    pub file_date: Option<String>,
    /// Anything worth telling the user about how this was resolved, a guessed
    /// flavor, or why a provider could not answer.
    #[serde(default)]
    pub note: Option<String>,
    /// What kind of build this is. Absent from providers that publish only
    /// one, and from an older proxy.
    #[serde(default)]
    pub channel: Option<super::Channel>,
}

impl Resolved {
    /// The URL to fetch, if there is one to fetch.
    pub fn downloadable(&self) -> Option<&str> {
        match self.status {
            Status::Ok => self.download_url.as_deref(),
            _ => None,
        }
    }

    /// A user-facing explanation of why this one cannot be installed.
    pub fn unavailable_reason(&self) -> Option<String> {
        match self.status {
            Status::Ok if self.download_url.is_some() => None,
            Status::Ok => Some("the catalog returned no download URL".to_owned()),
            Status::OptOut => Some(
                "the author has disabled third-party downloads — install it from the project page"
                    .to_owned(),
            ),
            Status::NotFound => Some("this project no longer exists".to_owned()),
        }
    }

    /// The author's own GitHub repository, `owner/repo`, for an opt-out.
    ///
    /// Offered as a button the user presses, never followed on their behalf:
    /// the opt-out is the author saying no to downloads through someone
    /// else's catalogue, and quietly switching sources because of it would
    /// be ignoring that. A release the author published themselves, fetched
    /// because the user asked for it, is the author's own channel.
    pub fn github_repo(&self) -> Option<String> {
        if self.status != Status::OptOut {
            return None;
        }
        super::browse::github_repo_of(self.source_url.as_deref()?)
    }
}

/// One addon to resolve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub provider: String,
    pub id: String,
    /// Pin to an exact file, as a WoWUp token records. Omit for the newest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    /// Which builds are acceptable. Omitted means the proxy's own default,
    /// which is stable, so an older client keeps getting releases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<super::Channel>,
}

impl From<&super::Entry> for Request {
    fn from(entry: &super::Entry) -> Self {
        Self {
            provider: entry.provider().as_str().to_owned(),
            id: entry.id.clone(),
            version_id: entry.version_id.clone(),
            // An import token says nothing about channels; the importing
            // machine's own default applies, filled in by `install_all`.
            channel: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ResolveBody<'a> {
    flavor: &'a str,
    addons: &'a [Request],
}

#[derive(Debug, Clone, Deserialize)]
struct ResolveReply {
    #[serde(default)]
    results: Vec<Resolved>,
}

/// A project as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub website_url: Option<String>,
    /// The author's own page at the provider. Absent on an older proxy.
    #[serde(default)]
    pub author_url: Option<String>,
    /// The project's source repository. Absent on an older proxy.
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    /// Lifetime downloads, as the proxy reports them. Absent on an older
    /// proxy, which then ranks every CurseForge hit equally, see
    /// [`crate::addons::browse`].
    #[serde(default)]
    pub download_count: u64,
    /// The author turned third-party downloads off. Absent on an older proxy,
    /// which leaves it to resolve to find out.
    #[serde(default)]
    pub opted_out: bool,
}

/// A project the catalog recognised from a folder's fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub file_id: Option<String>,
    /// Every folder of the file this fingerprint belongs to.
    ///
    /// A match on one folder identifies the whole package, and the package
    /// is what a ledger entry has to record: adopting one folder of BigWigs
    /// and leaving twelve unmanaged would be worse than adopting none.
    #[serde(default)]
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct FingerprintRequest<'a> {
    fingerprints: &'a [u32],
}

#[derive(Debug, Clone, Deserialize)]
struct MatchReply {
    #[serde(default)]
    matches: BTreeMap<String, Match>,
}

/// One file a project offers, as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub file_id: String,
    /// What the author calls this build, a version people recognise, where
    /// the file id is an opaque number.
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    /// `release`, `beta` or `alpha`, in the provider's own vocabulary.
    #[serde(default)]
    pub release_type: Option<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub file_date: Option<String>,
    #[serde(default)]
    pub file_length: Option<u64>,
    #[serde(default)]
    pub md5: Option<String>,
    #[serde(default)]
    pub folders: Vec<String>,
    /// Folder name to the fingerprint the provider expects it to hash to.
    ///
    /// The authority's own statement of what a correct install looks like,
    /// which is the only way to check our hashing is right: a fingerprint
    /// that is wrong matches nothing, and matching nothing is also what a
    /// genuinely unknown addon looks like.
    #[serde(default)]
    pub fingerprints: std::collections::BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchReply {
    #[serde(default)]
    results: Vec<Project>,
}

/// The catalog client.
#[derive(Debug, Clone)]
pub struct Catalog {
    origin: Origin,
}

impl Catalog {
    pub fn new(origin: Origin) -> Self {
        Self { origin }
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    fn url(&self, path: &str) -> Option<String> {
        match &self.origin {
            Origin::Http(base) => {
                Some(format!("{}/{API_PREFIX}/{path}", base.trim_end_matches('/')))
            }
            Origin::Dir(_) => None,
        }
    }

    fn fixture<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<T, CatalogError> {
        let Origin::Dir(dir) = &self.origin else {
            unreachable!("fixture read on an http origin")
        };
        let path = dir.join(name);
        let bytes = std::fs::read(&path).map_err(|source| CatalogError::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| CatalogError::Fixture { path, source })
    }

    /// Resolve a batch of addons to download URLs.
    ///
    /// Requests beyond [`RESOLVE_LIMIT`] are split into several calls rather
    /// than rejected, a shared list is allowed to be long, and the chunking is
    /// nobody else's problem.
    pub async fn resolve(
        &self,
        http: &Http,
        flavor: &str,
        requests: &[Request],
    ) -> Result<Vec<Resolved>, CatalogError> {
        if let Origin::Dir(_) = self.origin {
            let reply: ResolveReply = self.fixture("resolve.json")?;
            // A fixture answers for everything it knows about; anything else is
            // reported missing rather than silently dropped.
            return Ok(requests
                .iter()
                .map(|request| {
                    reply
                        .results
                        .iter()
                        .find(|r| r.id == request.id)
                        .cloned()
                        .unwrap_or_else(|| Resolved {
                            id: request.id.clone(),
                            status: Status::NotFound,
                            ..Resolved::default()
                        })
                })
                .collect());
        }

        let url = self.url("resolve").expect("http origin");
        let mut resolved = Vec::with_capacity(requests.len());
        for chunk in requests.chunks(RESOLVE_LIMIT) {
            let body = ResolveBody { flavor, addons: chunk };
            let reply: ResolveReply = http.post_json(&url, &body).await?;
            resolved.extend(reply.results);
        }
        Ok(resolved)
    }

    /// Search the catalog by text.
    pub async fn search(
        &self,
        http: &Http,
        flavor: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Project>, CatalogError> {
        if let Origin::Dir(_) = self.origin {
            let reply: SearchReply = self.fixture("search.json")?;
            let needle = query.to_lowercase();
            return Ok(reply
                .results
                .into_iter()
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .take(limit)
                .collect());
        }

        let url = format!(
            "{}?q={}&flavor={flavor}&limit={limit}",
            self.url("search").expect("http origin"),
            urlencode(query)
        );
        let reply: SearchReply = http.get_json(&url).await?;
        Ok(reply.results)
    }

    /// Every file a project offers for one flavor, newest first.
    ///
    /// What a version picker is built on, and the one question `resolve`
    /// cannot answer: resolve returns the *chosen* file, this returns the
    /// choices. Asked for one project at a time, because that is when
    /// somebody is looking at one.
    pub async fn files(
        &self,
        http: &Http,
        id: &str,
        flavor: &str,
        limit: usize,
    ) -> Result<Vec<FileInfo>, CatalogError> {
        match &self.origin {
            Origin::Dir(_) => self.fixture(&format!("files-{id}.json")),
            Origin::Http(_) => {
                let url = format!(
                    "{}?flavor={flavor}&limit={limit}",
                    self.url(&format!("project/{id}/files")).expect("http origin")
                );
                Ok(http.get_json(&url).await?)
            }
        }
    }

    /// Which project each folder fingerprint belongs to.
    ///
    /// One call for a whole library: the answer is keyed by the fingerprints
    /// *we* sent, so a caller holding folder-to-hash can turn it into
    /// folder-to-project without a second lookup. Hashes the catalog does
    /// not recognise are simply absent.
    pub async fn match_fingerprints(
        &self,
        http: &Http,
        fingerprints: &[u32],
    ) -> Result<BTreeMap<String, Match>, CatalogError> {
        if fingerprints.is_empty() {
            return Ok(BTreeMap::new());
        }
        match &self.origin {
            Origin::Dir(_) => {
                let reply: MatchReply = self.fixture("fingerprints.json")?;
                Ok(reply.matches)
            }
            Origin::Http(_) => {
                let url = self.url("fingerprints").expect("http origin");
                let body = FingerprintRequest { fingerprints };
                let reply: MatchReply = http.post_json(&url, &body).await?;
                Ok(reply.matches)
            }
        }
    }

    /// One project's metadata.
    pub async fn project(&self, http: &Http, id: &str) -> Result<Project, CatalogError> {
        match &self.origin {
            Origin::Dir(_) => self.fixture(&format!("project-{id}.json")),
            Origin::Http(_) => {
                let url = self.url(&format!("project/{id}")).expect("http origin");
                Ok(http.get_json(&url).await?)
            }
        }
    }

    /// A project's long description, already reduced to blocks by the proxy.
    ///
    /// See [`crate::addons::richtext`] for why no markup reaches us.
    pub async fn description(&self, http: &Http, id: &str) -> Result<richtext::Document, CatalogError> {
        match &self.origin {
            Origin::Dir(_) => self.fixture(&format!("description-{id}.json")),
            Origin::Http(_) => {
                let url = self.url(&format!("project/{id}/description")).expect("http origin");
                Ok(http.get_json(&url).await?)
            }
        }
    }

    /// One file's changelog.
    ///
    /// Keyed by the file as well as the project, because "what changed" is a
    /// question about a build and not about an addon, which is also why the
    /// proxy can cache it for a day: a published file's changelog is final.
    pub async fn changelog(
        &self,
        http: &Http,
        id: &str,
        file_id: &str,
    ) -> Result<richtext::Document, CatalogError> {
        match &self.origin {
            Origin::Dir(_) => self.fixture(&format!("changelog-{id}-{file_id}.json")),
            Origin::Http(_) => {
                let url = self
                    .url(&format!("project/{id}/files/{file_id}/changelog"))
                    .expect("http origin");
                Ok(http.get_json(&url).await?)
            }
        }
    }
}

/// Percent-encode a query value. Only the handful of characters an addon
/// search can realistically contain need escaping.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Net(#[from] NetError),

    #[error("could not read the catalog fixture at {path}")]
    Fixture {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(status: Status, url: Option<&str>) -> Resolved {
        Resolved {
            id: "2382".into(),
            status,
            name: Some("BigWigs".into()),
            file_id: Some("8866508".into()),
            file_name: Some("BigWigs-v424.8.zip".into()),
            file_length: Some(1_234_567),
            download_url: url.map(str::to_owned),
            folders: vec!["BigWigs".into()],
            website_url: Some("https://www.curseforge.com/wow/addons/big-wigs".into()),
            ..Resolved::default()
        }
    }

    #[test]
    fn an_opt_out_is_an_outcome_not_an_error() {
        let entry = resolved(Status::OptOut, None);
        assert!(entry.downloadable().is_none());
        let reason = entry.unavailable_reason().unwrap();
        assert!(reason.contains("project page"), "{reason}");
        // The website is what makes it actionable.
        assert!(entry.website_url.is_some());
    }

    #[test]
    fn only_an_opt_out_offers_the_authors_github() {
        let dbm = "https://github.com/DeadlyBossMods/DeadlyBossMods";
        let mut entry = resolved(Status::OptOut, None);
        entry.source_url = Some(dbm.into());
        assert_eq!(entry.github_repo().as_deref(), Some("DeadlyBossMods/DeadlyBossMods"));

        // Anything that downloads already has no business with a second source.
        let mut ok = resolved(Status::Ok, Some("https://edge.forgecdn.net/x.zip"));
        ok.source_url = Some(dbm.into());
        assert_eq!(ok.github_repo(), None);

        // A repository somewhere else is not something the GitHub provider reads.
        entry.source_url = Some("https://gitlab.com/someone/addon".into());
        assert_eq!(entry.github_repo(), None);
        entry.source_url = None;
        assert_eq!(entry.github_repo(), None);
    }

    #[test]
    fn an_ok_entry_with_a_url_is_downloadable_and_has_no_complaint() {
        let entry = resolved(Status::Ok, Some("https://edge.forgecdn.net/x.zip"));
        assert_eq!(entry.downloadable(), Some("https://edge.forgecdn.net/x.zip"));
        assert!(entry.unavailable_reason().is_none());
    }

    #[test]
    fn an_ok_entry_without_a_url_is_reported_rather_than_trusted() {
        // Defensive: a proxy bug must not become a panic or a silent no-op.
        let entry = resolved(Status::Ok, None);
        assert!(entry.downloadable().is_none());
        assert!(entry.unavailable_reason().is_some());
    }

    #[test]
    fn a_token_entry_carries_its_pin_into_the_request() {
        let entry = super::super::Entry {
            id: "2382".into(),
            name: "BigWigs".into(),
            provider: "Curse".into(),
            version_id: Some("8866508".into()),
        };
        let request = Request::from(&entry);
        assert_eq!(request.provider, "curse");
        assert_eq!(request.version_id.as_deref(), Some("8866508"));
    }

    #[test]
    fn urls_are_built_under_the_versioned_prefix() {
        let catalog = Catalog::new(Origin::Http("https://lightforge.tuxcore.pro/".into()));
        assert_eq!(
            catalog.url("resolve").unwrap(),
            "https://lightforge.tuxcore.pro/addons/v1/resolve"
        );
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(urlencode("big wigs"), "big+wigs");
        assert_eq!(urlencode("a/b&c"), "a%2Fb%26c");
    }
}
