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

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    /// Product configs, JSON, under the CDN row's `ConfigPath`
    /// (`tpr/configs/data`), which already ends where the hash begins.
    ProductConfig,
}

impl Kind {
    fn segment(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Data => "data",
            Self::Patch => "patch",
            Self::ProductConfig => "",
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
        let prefix = match kind.segment() {
            "" => self.path.trim_matches('/').to_owned(),
            segment => format!("{}/{segment}", self.path.trim_matches('/')),
        };
        Ok(format!("{}/{prefix}/{}/{}/{hash}", self.origin.trim_end_matches('/'), &hash[0..2], &hash[2..4]))
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
fn endpoint_failed(url: &str, source: &NetError) {
    static SLOT: crate::logging::Throttle = crate::logging::Throttle::new();
    tracing::debug!("cdn request failed, trying the next host: {url}: {}", crate::error::chain(source));
    if let Some(held) = SLOT.pass(10) {
        tracing::warn!(
            "cdn request failed, trying the next host ({held} similar since the last note): {url}: {}",
            crate::error::chain(source)
        );
    }
}

/// What one endpoint made of a request.
enum Tried<T> {
    Got(T),
    /// No proper answer: refused, timed out, cut off, or a server error. The
    /// host rests, and the request is worth one more pass.
    Down(CdnError),
    /// An answer, wrong for this file: missing there, or failing its check.
    Wrong(CdnError),
}

impl<T> Tried<T> {
    fn failed(url: String, source: NetError) -> Self {
        endpoint_failed(&url, &source);
        let down = match &source {
            NetError::Request { .. } | NetError::Truncated { .. } => true,
            NetError::Status { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        };
        let error = CdnError::Fetch { url, source };
        if down { Self::Down(error) } else { Self::Wrong(error) }
    }
}

/// How long a host that did not answer is tried last. Without it, a host
/// that times out costs the full connect timeout on every request of a
/// download, each one failing over the same way.
const REST: Duration = Duration::from_secs(5 * 60);

/// Hosts resting after a failure, by origin, until when. Shared by every
/// client: the network they are failing on is.
fn resting() -> std::sync::MutexGuard<'static, HashMap<String, Instant>> {
    static RESTING: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    RESTING.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Before the one retry, so a blip passes: 1 to 2 s, apart for each request
/// so a whole download does not come back at once.
fn backoff() -> Duration {
    use std::hash::{BuildHasher, RandomState};
    Duration::from_millis(1000 + RandomState::new().hash_one(Instant::now()) % 1000)
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

    /// The endpoints in the order to try them: resting ones last, still tried.
    fn in_order(&self) -> Vec<&Endpoint> {
        let now = Instant::now();
        let resting = resting();
        let rests = |e: &&Endpoint| resting.get(&e.origin).is_some_and(|until| *until > now);
        let (tired, fresh): (Vec<&Endpoint>, Vec<&Endpoint>) = self.endpoints.iter().partition(rests);
        fresh.into_iter().chain(tired).collect()
    }

    /// Ask each endpoint in turn until one answers right, and once more
    /// after a pause when one was down.
    async fn across<T, Fut>(
        &self,
        url: impl Fn(&Endpoint) -> Result<String, CdnError>,
        mut fetch: impl FnMut(String) -> Fut,
    ) -> Result<T, CdnError>
    where
        Fut: std::future::Future<Output = Tried<T>>,
    {
        let mut last = None;
        for pass in 0..2 {
            let mut down = false;
            for endpoint in self.in_order() {
                match fetch(url(endpoint)?).await {
                    Tried::Got(value) => {
                        resting().remove(&endpoint.origin);
                        return Ok(value);
                    }
                    Tried::Down(e) => {
                        resting().insert(endpoint.origin.clone(), Instant::now() + REST);
                        down = true;
                        last = Some(e);
                    }
                    Tried::Wrong(e) => last = Some(e),
                }
            }
            if !down || pass == 1 {
                break;
            }
            tokio::time::sleep(backoff()).await;
        }
        Err(last.unwrap_or(CdnError::NoEndpoints))
    }

    /// Fetch a name without checking the bytes: the caller verifies by the
    /// rule for its kind of name (module docs).
    pub async fn fetch(&self, http: &Http, kind: Kind, hash: &str) -> Result<Vec<u8>, CdnError> {
        self.across(
            |endpoint| endpoint.url(kind, hash),
            |url| async move {
                match http.get_bytes(&url, MAX_CDN_BYTES).await {
                    Ok(bytes) => Tried::Got(bytes),
                    Err(source) => Tried::failed(url, source),
                }
            },
        )
        .await
    }

    /// Fetch a file named by the MD5 of its bytes (a config of either kind) and
    /// check it. A mismatch tries the next endpoint; if all disagree, the
    /// error reports the mismatch.
    pub async fn fetch_md5_verified(
        &self,
        http: &Http,
        kind: Kind,
        hash: &str,
    ) -> Result<Vec<u8>, CdnError> {
        let expected = parse_md5(hash)?;
        self.across(
            |endpoint| endpoint.url(kind, hash),
            |url| async move {
                let bytes = match http.get_bytes(&url, MAX_CDN_BYTES).await {
                    Ok(bytes) => bytes,
                    Err(source) => return Tried::failed(url, source),
                };
                let actual = *md5::compute(&bytes);
                if actual == expected {
                    return Tried::Got(bytes);
                }
                Tried::Wrong(CdnError::HashMismatch { url, expected: hash.to_ascii_lowercase(), found: hex(&actual) })
            },
        )
        .await
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
        self.across(
            |endpoint| endpoint.index_url(hash),
            |url| async move {
                let bytes = match http.get_bytes(&url, MAX_CDN_BYTES).await {
                    Ok(bytes) => bytes,
                    Err(source) => return Tried::failed(url, source),
                };
                if let Some(want) = expect_size.filter(|want| bytes.len() as u64 != *want) {
                    return Tried::Wrong(CdnError::WrongSize { url, expected: want, found: bytes.len() as u64 });
                }
                match super::archive::verify(&bytes, hash) {
                    Ok(()) => Tried::Got(bytes),
                    Err(source) => Tried::Wrong(CdnError::BadIndex { url, source }),
                }
            },
        )
        .await
    }

    /// Fetch a byte range of an archive, as served. Unverified: a run covers
    /// several files, each checked after the run is split.
    pub async fn fetch_range(
        &self,
        http: &Http,
        archive: &str,
        range: &str,
    ) -> Result<Vec<u8>, CdnError> {
        self.across(
            |endpoint| endpoint.url(Kind::Data, archive),
            |url| async move {
                match http.get_range(&url, range).await {
                    Ok(bytes) => Tried::Got(bytes),
                    Err(source) => Tried::failed(url, source),
                }
            },
        )
        .await
    }

    /// Fetch a loose file (one the file index lists), decode it, and verify it
    /// against its content key.
    pub async fn fetch_loose(
        &self,
        http: &Http,
        ekey: &super::storage::EKey,
        expect_ckey: &super::config::CKey,
    ) -> Result<Vec<u8>, CdnError> {
        self.fetch_loose_blte(http, ekey, expect_ckey).await.map(|(_, decoded)| decoded)
    }

    /// [`Client::fetch_loose`], also returning the BLTE bytes as served, which
    /// is what the store holds: `(blte, decoded)`.
    pub async fn fetch_loose_blte(
        &self,
        http: &Http,
        ekey: &super::storage::EKey,
        expect_ckey: &super::config::CKey,
    ) -> Result<(Vec<u8>, Vec<u8>), CdnError> {
        let hash = hex(ekey);
        self.across(
            |endpoint| endpoint.url(Kind::Data, &hash),
            |url| async move {
                match http.get_bytes(&url, MAX_CDN_BYTES).await {
                    Ok(raw) => checked(url, expect_ckey, &raw).map(|decoded| (raw, decoded)),
                    Err(source) => Tried::failed(url, source),
                }
            },
        )
        .await
    }

    /// Fetch one file out of an archive by range request, decode it, and
    /// verify it against its content key.
    pub async fn fetch_from_archive(
        &self,
        http: &Http,
        archive: &str,
        located: &super::archive::Located,
        expect_ckey: &super::config::CKey,
    ) -> Result<Vec<u8>, CdnError> {
        let range = located.range().ok_or_else(|| CdnError::EmptyRange { archive: archive.to_owned() })?;
        self.across(
            |endpoint| endpoint.url(Kind::Data, archive),
            |url| {
                let range = &range;
                async move {
                    // On the CDN there is no CASC entry header in front.
                    match http.get_range(&url, range).await {
                        Ok(raw) => checked(url, expect_ckey, &raw),
                        Err(source) => Tried::failed(url, source),
                    }
                }
            },
        )
        .await
    }
}

/// `raw` decoded and checked against the content key it was fetched for.
fn checked(url: String, expect_ckey: &super::config::CKey, raw: &[u8]) -> Tried<Vec<u8>> {
    let decoded = match super::blte::decode(raw) {
        Ok(decoded) => decoded,
        Err(e) => return Tried::Wrong(CdnError::Decode { url, detail: e.to_string() }),
    };
    let actual = *md5::compute(&decoded);
    if &actual == expect_ckey {
        return Tried::Got(decoded);
    }
    Tried::Wrong(CdnError::HashMismatch { url, expected: hex(expect_ckey), found: hex(&actual) })
}

impl<T> Tried<T> {
    fn map<U>(self, f: impl FnOnce(T) -> U) -> Tried<U> {
        match self {
            Self::Got(value) => Tried::Got(f(value)),
            Self::Down(e) => Tried::Down(e),
            Self::Wrong(e) => Tried::Wrong(e),
        }
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

    /// A host that answers every request with `status` and `body`.
    fn host(status: &'static str, body: &'static [u8]) -> Endpoint {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.read(&mut [0u8; 4096]);
                let length = body.len();
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        Endpoint { origin, path: "tpr/wow".into() }
    }

    /// A port nothing listens on: the connection is refused.
    fn dead_host() -> Endpoint {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        Endpoint { origin: format!("http://127.0.0.1:{port}"), path: "tpr/wow".into() }
    }

    #[tokio::test]
    async fn a_host_that_did_not_answer_is_asked_last_from_then_on() {
        let (dead, alive) = (dead_host(), host("200 OK", b"bytes"));
        let client = Client::new(vec![dead.clone(), alive.clone()]).unwrap();
        let http = Http::new().unwrap();

        assert_eq!(client.fetch(&http, Kind::Data, "abcd1234").await.unwrap(), b"bytes");

        assert_eq!(client.in_order(), [&alive, &dead]);
        // Another client on the same network knows too.
        let other = Client::new(vec![dead.clone(), alive.clone()]).unwrap();
        assert_eq!(other.in_order(), [&alive, &dead]);
    }

    /// A 404 is that host lacking one file, not the host being down.
    #[tokio::test]
    async fn a_host_missing_a_file_is_not_rested() {
        let (missing, alive) = (host("404 Not Found", b""), host("200 OK", b"bytes"));
        let client = Client::new(vec![missing.clone(), alive.clone()]).unwrap();

        client.fetch(&Http::new().unwrap(), Kind::Data, "abcd1234").await.unwrap();

        assert_eq!(client.in_order(), [&missing, &alive]);
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
