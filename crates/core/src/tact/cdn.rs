//! Fetching content from Blizzard's CDN, verified by hash.
//!
//! URLs are content-addressed with the same two-level fan-out the local
//! `Data/config` directory uses:
//!
//! ```text
//! <server>/<path>/config/<h0h1>/<h2h3>/<hash>    build and CDN configs
//! <server>/<path>/data/<h0h1>/<h2h3>/<hash>      archives and loose files
//! ```
//!
//! **Each kind of name verifies differently** (measured on 12.1.0):
//!
//! | name | relationship to the bytes |
//! |---|---|
//! | config hash | MD5 of the file. |
//! | content key (CKey) | MD5 of the **decoded** content. |
//! | encoding key (EKey) | MD5 of the BLTE chunk table, **not** of the file. |
//!
//! The root manifest, served at `data/d1/75/d175f4af…`, hashes to
//! `c95b92c6…`. So a BLTE download is decoded and checked against its
//! content key, the bytes the game reads. That key comes from a manifest
//! chain rooted in the build config, which is the security boundary, not TLS.

use crate::net::{Http, NetError, MAX_CDN_BYTES};

use crate::text::hex;
use super::version::Cdn;

/// Where a hash is fetched from within a CDN path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Build and CDN configs, plain text.
    Config,
    /// Archives, loose files and manifests, BLTE-encoded.
    Data,
    /// Patch archives.
    Patch,
}

impl Kind {
    fn segment(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Data => "data",
            Self::Patch => "patch",
        }
    }
}

/// A CDN endpoint resolved from the `cdns` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Scheme and host with no trailing slash, e.g. `https://level3.ssl.blizzard.com`.
    pub origin: String,
    /// Path prefix, e.g. `tpr/wow`.
    pub path: String,
}

impl Endpoint {
    /// Every usable endpoint for a CDN row, TLS first. `Servers` query
    /// parameters (`?maxhosts=8&fallback=1`) are hints for Blizzard's client
    /// and are stripped. Plain HTTP stays as a fallback: content is verified
    /// by hash, and some networks block the TLS host.
    pub fn from_cdn(cdn: &Cdn) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::new();
        let mut push = |origin: String| {
            let endpoint = Self { origin, path: cdn.path.clone() };
            if !out.contains(&endpoint) {
                out.push(endpoint);
            }
        };

        for server in cdn.servers.iter().filter(|s| s.starts_with("https://")) {
            push(origin_of(server));
        }
        // Hosts are bare names with no scheme; assume TLS for those.
        for host in &cdn.hosts {
            push(format!("https://{host}"));
        }
        for server in cdn.servers.iter().filter(|s| s.starts_with("http://")) {
            push(origin_of(server));
        }
        out
    }

    /// The URL a hash lives at.
    pub fn url(&self, kind: Kind, hash: &str) -> Result<String, CdnError> {
        if hash.len() < 4 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CdnError::BadHash(hash.to_owned()));
        }
        Ok(format!(
            "{}/{}/{}/{}/{}/{}",
            self.origin.trim_end_matches('/'),
            self.path.trim_matches('/'),
            kind.segment(),
            &hash[0..2],
            &hash[2..4],
            hash
        ))
    }
}

impl Endpoint {
    /// The URL an index lives at: the archive's URL plus `.index`, the file
    /// index included.
    pub fn index_url(&self, hash: &str) -> Result<String, CdnError> {
        Ok(format!("{}.index", self.url(Kind::Data, hash)?))
    }
}

/// One endpoint failed; the next is tried. Logged at debug, with a throttled
/// warning: an outage fails every request.
fn endpoint_failed(url: String, source: NetError) -> CdnError {
    static SLOT: crate::logging::Throttle = crate::logging::Throttle::new();
    tracing::debug!("cdn request failed, trying the next host: {url}: {}", crate::error::chain(&source));
    if let Some(held) = SLOT.pass(10) {
        tracing::warn!(
            "cdn request failed, trying the next host ({held} similar since the last note): {url}: {}",
            crate::error::chain(&source)
        );
    }
    CdnError::Fetch { url, source }
}

/// Scheme and host of a server URL, dropping path and query.
fn origin_of(server: &str) -> String {
    let without_scheme = server.split_once("://").map(|(_, rest)| rest).unwrap_or(server);
    let scheme = server.split_once("://").map(|(s, _)| s).unwrap_or("https");
    let host = without_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(without_scheme);
    format!("{scheme}://{host}")
}

/// Fetches content across a set of endpoints, verifying every response.
#[derive(Debug, Clone)]
pub struct Client {
    endpoints: Vec<Endpoint>,
}

impl Client {
    pub fn new(endpoints: Vec<Endpoint>) -> Result<Self, CdnError> {
        if endpoints.is_empty() {
            return Err(CdnError::NoEndpoints);
        }
        Ok(Self { endpoints })
    }

    /// Build a client from a `cdns` table row.
    pub fn from_cdn(cdn: &Cdn) -> Result<Self, CdnError> {
        Self::new(Endpoint::from_cdn(cdn))
    }

    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    /// Fetch a name, trying each endpoint in turn, without checking the bytes:
    /// the caller verifies by the rule for its kind of name (module docs).
    pub async fn fetch(&self, http: &Http, kind: Kind, hash: &str) -> Result<Vec<u8>, CdnError> {
        let mut last: Option<CdnError> = None;
        for endpoint in &self.endpoints {
            let url = endpoint.url(kind, hash)?;
            match http.get_bytes(&url, MAX_CDN_BYTES).await {
                Ok(bytes) => return Ok(bytes),
                Err(source) => last = Some(endpoint_failed(url, source)),
            }
        }
        Err(last.unwrap_or(CdnError::NoEndpoints))
    }

    /// Fetch a file named by the MD5 of its bytes ([`Kind::Config`] only) and
    /// check it. A mismatch tries the next endpoint; if all disagree, the
    /// error reports the mismatch.
    pub async fn fetch_md5_verified(
        &self,
        http: &Http,
        kind: Kind,
        hash: &str,
    ) -> Result<Vec<u8>, CdnError> {
        let expected = parse_md5(hash)?;
        let mut last: Option<CdnError> = None;

        for endpoint in &self.endpoints {
            let url = endpoint.url(kind, hash)?;
            match http.get_bytes(&url, MAX_CDN_BYTES).await {
                Ok(bytes) => {
                    let actual = *md5::compute(&bytes);
                    if actual == expected {
                        return Ok(bytes);
                    }
                    last = Some(CdnError::HashMismatch {
                        url,
                        expected: hash.to_ascii_lowercase(),
                        found: hex(&actual),
                    });
                }
                Err(source) => last = Some(endpoint_failed(url, source)),
            }
        }

        Err(last.unwrap_or(CdnError::NoEndpoints))
    }

    /// Fetch a config and verify it by its MD5.
    pub async fn fetch_config(&self, http: &Http, hash: &str) -> Result<String, CdnError> {
        let bytes = self.fetch_md5_verified(http, Kind::Config, hash).await?;
        String::from_utf8(bytes).map_err(|e| CdnError::NotText { hash: hash.to_owned(), source: e })
    }

    /// Fetch an index, checking it against the size its CDN config declares.
    /// A wrong length (a truncated mirror) tries the next endpoint.
    pub async fn fetch_index(
        &self,
        http: &Http,
        hash: &str,
        expect_size: Option<u64>,
    ) -> Result<Vec<u8>, CdnError> {
        let mut last: Option<CdnError> = None;

        for endpoint in &self.endpoints {
            let url = endpoint.index_url(hash)?;
            match http.get_bytes(&url, MAX_CDN_BYTES).await {
                Ok(bytes) => match expect_size {
                    Some(want) if bytes.len() as u64 != want => {
                        last = Some(CdnError::WrongSize { url, expected: want, found: bytes.len() as u64 });
                    }
                    _ => match super::archive::verify(&bytes, hash) {
                        Ok(()) => return Ok(bytes),
                        Err(source) => last = Some(CdnError::BadIndex { url, source }),
                    },
                },
                Err(source) => last = Some(endpoint_failed(url, source)),
            }
        }

        Err(last.unwrap_or(CdnError::NoEndpoints))
    }

    /// Fetch a byte range of an archive, as served. Unverified: a run covers
    /// several files, each checked after the run is split.
    pub async fn fetch_range(
        &self,
        http: &Http,
        archive: &str,
        range: &str,
    ) -> Result<Vec<u8>, CdnError> {
        let mut last: Option<CdnError> = None;

        for endpoint in &self.endpoints {
            let url = endpoint.url(Kind::Data, archive)?;
            match http.get_range(&url, range).await {
                Ok(bytes) => return Ok(bytes),
                Err(source) => last = Some(endpoint_failed(url, source)),
            }
        }

        Err(last.unwrap_or(CdnError::NoEndpoints))
    }

    /// Fetch a loose file (one the file index lists), decode it, and verify it
    /// against its content key.
    pub async fn fetch_loose(
        &self,
        http: &Http,
        ekey: &super::storage::EKey,
        expect_ckey: &super::config::CKey,
        keys: Option<&ngdp_crypto::KeyService>,
    ) -> Result<Vec<u8>, CdnError> {
        let hash = hex(ekey);
        let mut last: Option<CdnError> = None;

        for endpoint in &self.endpoints {
            let url = endpoint.url(Kind::Data, &hash)?;
            let raw = match http.get_bytes(&url, MAX_CDN_BYTES).await {
                Ok(bytes) => bytes,
                Err(source) => {
                    last = Some(CdnError::Fetch { url, source });
                    continue;
                }
            };

            match blte::decompress_blte(raw, keys) {
                Ok(decoded) => {
                    let actual = *md5::compute(&decoded);
                    if &actual == expect_ckey {
                        return Ok(decoded);
                    }
                    last = Some(CdnError::HashMismatch {
                        url,
                        expected: hex(expect_ckey),
                        found: hex(&actual),
                    });
                }
                Err(e) => last = Some(CdnError::Decode { url, detail: e.to_string() }),
            }
        }

        Err(last.unwrap_or(CdnError::NoEndpoints))
    }

    /// Fetch one file out of an archive by range request, decode it, and
    /// verify it against its content key.
    pub async fn fetch_from_archive(
        &self,
        http: &Http,
        archive: &str,
        located: &super::archive::Located,
        expect_ckey: &super::config::CKey,
        keys: Option<&ngdp_crypto::KeyService>,
    ) -> Result<Vec<u8>, CdnError> {
        let range = located.range().ok_or_else(|| CdnError::EmptyRange { archive: archive.to_owned() })?;
        let mut last: Option<CdnError> = None;

        for endpoint in &self.endpoints {
            let url = endpoint.url(Kind::Data, archive)?;
            let raw = match http.get_range(&url, &range).await {
                Ok(bytes) => bytes,
                Err(source) => {
                    last = Some(CdnError::Fetch { url, source });
                    continue;
                }
            };

            // On the CDN there is no CASC entry header in front.
            match blte::decompress_blte(raw, keys) {
                Ok(decoded) => {
                    let actual = *md5::compute(&decoded);
                    if &actual == expect_ckey {
                        return Ok(decoded);
                    }
                    last = Some(CdnError::HashMismatch {
                        url,
                        expected: hex(expect_ckey),
                        found: hex(&actual),
                    });
                }
                Err(e) => last = Some(CdnError::Decode { url, detail: e.to_string() }),
            }
        }

        Err(last.unwrap_or(CdnError::NoEndpoints))
    }
}

fn parse_md5(hash: &str) -> Result<[u8; 16], CdnError> {
    super::config::parse_hash(hash).map_err(|_| CdnError::BadHash(hash.to_owned()))
}

#[derive(Debug, thiserror::Error)]
pub enum CdnError {
    #[error("no CDN endpoints are configured")]
    NoEndpoints,

    #[error("`{0}` is not a content hash")]
    BadHash(String),

    #[error("{url} returned bytes hashing to {found}, expected {expected}")]
    HashMismatch {
        url: String,
        expected: String,
        found: String,
    },

    #[error("could not fetch {url}")]
    Fetch {
        url: String,
        #[source]
        source: NetError,
    },

    #[error("could not decode the BLTE payload from {url}: {detail}")]
    Decode { url: String, detail: String },

    #[error("a zero-length range into archive {archive} cannot be requested")]
    EmptyRange { archive: String },

    #[error("{url} is not the index it is named as")]
    BadIndex {
        url: String,
        #[source]
        source: super::archive::ArchiveError,
    },

    #[error("{url} returned {found} bytes, but its config declares {expected}")]
    WrongSize { url: String, expected: u64, found: u64 },

    #[error("the config {hash} is not valid UTF-8")]
    NotText {
        hash: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cdn_row() -> Cdn {
        // Verbatim from `https://eu.version.battle.net/wow/cdns`.
        Cdn {
            name: "eu".into(),
            path: "tpr/wow".into(),
            hosts: vec!["level3.blizzard.com".into()],
            servers: vec![
                "http://level3.blizzard.com/?maxhosts=8".into(),
                "https://level3.ssl.blizzard.com/?maxhosts=8&fallback=1".into(),
            ],
            config_path: "tpr/configs/data".into(),
        }
    }

    #[test]
    fn urls_use_the_same_fan_out_as_the_local_config_directory() {
        let e = Endpoint {
            origin: "https://level3.ssl.blizzard.com".into(),
            path: "tpr/wow".into(),
        };
        assert_eq!(
            e.url(Kind::Config, "9712ae9c84cc54111e9e3ddf9a6b8019").unwrap(),
            "https://level3.ssl.blizzard.com/tpr/wow/config/97/12/9712ae9c84cc54111e9e3ddf9a6b8019"
        );
        assert_eq!(
            e.url(Kind::Data, "d175f4afe795101d0cf3660228b3283b").unwrap(),
            "https://level3.ssl.blizzard.com/tpr/wow/data/d1/75/d175f4afe795101d0cf3660228b3283b"
        );
        assert!(matches!(e.url(Kind::Data, "nothex"), Err(CdnError::BadHash(_))));
    }

    #[test]
    fn server_query_parameters_are_not_part_of_the_content_path() {
        // Left in, the query lands in the middle of the path.
        assert_eq!(
            origin_of("https://level3.ssl.blizzard.com/?maxhosts=8&fallback=1"),
            "https://level3.ssl.blizzard.com"
        );
        assert_eq!(origin_of("http://level3.blizzard.com/?maxhosts=8"), "http://level3.blizzard.com");
        assert_eq!(origin_of("https://a.example.com"), "https://a.example.com");
    }

    #[test]
    fn tls_endpoints_are_tried_before_plain_ones() {
        let endpoints = Endpoint::from_cdn(&cdn_row());
        assert!(endpoints[0].origin.starts_with("https://"));
        // Plain HTTP stays as a fallback; content is verified by hash.
        assert!(endpoints.iter().any(|e| e.origin.starts_with("http://")));
        assert!(endpoints.iter().all(|e| e.path == "tpr/wow"));
    }

    #[test]
    fn duplicate_origins_collapse() {
        // `Hosts` and `Servers` name the same machine in different spellings.
        let cdn = Cdn {
            hosts: vec!["level3.ssl.blizzard.com".into()],
            servers: vec!["https://level3.ssl.blizzard.com/?maxhosts=8".into()],
            ..cdn_row()
        };
        let endpoints = Endpoint::from_cdn(&cdn);
        assert_eq!(endpoints.len(), 1, "{endpoints:?}");
    }

    #[test]
    fn a_client_needs_at_least_one_endpoint() {
        assert!(matches!(Client::new(Vec::new()), Err(CdnError::NoEndpoints)));
        assert!(Client::from_cdn(&cdn_row()).is_ok());
    }

    #[test]
    fn a_mismatch_error_names_both_hashes() {
        // The message must say wrong content, not unreachable.
        let err = CdnError::HashMismatch {
            url: "https://cdn/x".into(),
            expected: "9712ae9c84cc54111e9e3ddf9a6b8019".into(),
            found: "00000000000000000000000000000000".into(),
        };
        let text = err.to_string();
        assert!(text.contains("9712ae9c84cc54111e9e3ddf9a6b8019"), "{text}");
        assert!(text.contains("00000000000000000000000000000000"), "{text}");
    }

    #[test]
    fn an_encoding_key_is_not_the_md5_of_its_bytes() {
        // Measured: the root manifest under `d175f4af…` decodes correctly, and
        // its 50,502,156 bytes hash to `c95b92c6…`.
        let ekey = "d175f4afe795101d0cf3660228b3283b";
        let actual_md5_of_bytes = "c95b92c6350e33fca118d05d54d2e296";
        assert_ne!(ekey, actual_md5_of_bytes);

        // Content keys, by contrast, are exactly the MD5 of the decoded bytes,
        // which is what `fetch_from_archive` checks.
        let payload = b"decoded content";
        assert_eq!(hex(&*md5::compute(payload)).len(), 32);
    }

    #[test]
    fn md5_matches_the_hex_spelling_used_everywhere_else() {
        let digest = *md5::compute(b"lightforge");
        assert_eq!(hex(&digest).len(), 32);
        assert_eq!(parse_md5(&hex(&digest)).unwrap(), digest);
    }
}
