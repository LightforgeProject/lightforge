//! The async edge.
//!
//! `core` is otherwise synchronous; everything that touches the network lives
//! behind `async fn` here and in its callers. TLS is rustls rather than the
//! system OpenSSL, so behavior doesn't drift with the distribution's crypto
//! policy or library version.
//!
//! The [`ETagCache`] built here is shared by Proton-GE release polling and
//! the themed asset pipeline: both revalidate a small document on every
//! launch and want a `304` to cost nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt;

/// Sent on every request. GitHub rejects API calls without a User-Agent.
const USER_AGENT: &str = concat!("Lightforge/", env!("CARGO_PKG_VERSION"));

/// How long a response may go without sending a byte before it fails.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on a JSON or text document read whole into memory. The largest
/// real one, GitHub's release list, is a few hundred KB; a server streaming
/// without end should fail here rather than in the allocator.
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Ceiling on a cached picture, key art the largest of them.
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Name prefix of an unfinished [`Http::download`].
const DOWNLOAD_PREFIX: &str = ".lightforge-download-";

/// How old an unfinished download must be before the next one in the same
/// directory clears it away. A live transfer touches its file with every
/// chunk and fails after [`READ_TIMEOUT`] of silence, so an hour is only ever
/// one a crash or a kill left behind.
const STALE_DOWNLOAD: Duration = Duration::from_secs(60 * 60);

/// Shared HTTP client.
///
/// `reqwest::Client` owns a connection pool and is designed to be created once
/// and cloned, so hand this around rather than building clients per request.
#[derive(Debug, Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    /// One client, one connection pool.
    ///
    /// Spreading a bulk download over several pools was tried and **measured
    /// slower**, so it is deliberately not done. Blizzard's CDN speaks HTTP/2
    /// and reqwest multiplexes every request over one connection per host;
    /// isolated `curl` runs suggested four connections should beat one
    /// (33.3 vs 28.8 MB/s on 48 concurrent 2 MB ranges), but in the real
    /// download loop four pools measured 25.3 and 21.7 MB/s against one pool's
    /// 26.8 and 25.5. The warm, single connection wins. Do not re-add this
    /// without an A/B in the downloader itself.
    pub fn new() -> Result<Self, NetError> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            // Deliberately no overall request timeout: a Proton tarball is
            // ~400 MB and a slow link is not an error. A *silent* one is: this
            // resets on every chunk, so it only fires on a server that keeps
            // the connection open and stops sending, which, without it,
            // stalled a download (and any pause waiting on it) forever.
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(NetError::Client)?;
        Ok(Self { client })
    }

    /// Fetch and deserialize JSON, revalidating against `cache` when possible.
    ///
    /// Returns `None` when the server answers `304 Not Modified` and the cached
    /// copy is still good.
    pub async fn get_json_cached<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        cache: &ETagCache,
    ) -> Result<Option<T>, NetError> {
        let mut request = self.client.get(url);
        if let Some(etag) = cache.etag() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }

        let response = request.send().await.map_err(|e| NetError::Request {
            url: url.to_owned(),
            source: e,
        })?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let response = check_status(url, response)?;

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let body = bounded_body(url, response, MAX_DOCUMENT_BYTES).await?;

        // Decoded before caching: a stored body that does not parse would be
        // served back on every 304 until the server's ETag changes.
        let value = serde_json::from_slice(&body).map_err(|e| NetError::Decode {
            url: url.to_owned(),
            source: e,
        })?;
        cache.store(&body, etag.as_deref())?;
        Ok(Some(value))
    }

    /// POST a body and read a small JSON answer back.
    ///
    /// The one thing the launcher ever *sends*: the screenshot that goes with
    /// a bug report, which is uploaded so the report can carry a link rather
    /// than a megabyte of base64. It has its own timeout because, unlike a
    /// download, there is somebody watching a dialog wait for it, and
    /// because the whole call is optional: a failure here means the picture
    /// travels inside the report instead, so it must fail quickly rather than
    /// sit on the read timeout.
    pub async fn post_bytes<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<T, NetError> {
        let body = self.post(url, content_type, &[], body, timeout).await?;
        serde_json::from_slice(&body)
            .map_err(|e| NetError::Decode { url: url.to_owned(), source: e })
    }

    /// POST a body with whatever extra headers it needs, and hand back the
    /// response.
    ///
    /// The second sender, and the reason this takes headers at all: a bug
    /// report's event goes to our own Sentry server, which authenticates it
    /// with an `X-Sentry-Auth` header. Sending it through this client rather
    /// than through the one the sentry crate brings with it is what keeps a
    /// second HTTP stack, and a second TLS verifier, out of the release.
    pub async fn post(
        &self,
        url: &str,
        content_type: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<Vec<u8>, NetError> {
        let mut request = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .timeout(timeout);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        let response = request
            .body(body)
            .send()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;

        let response = check_status(url, response)?;
        response
            .bytes()
            .await
            .map(|body| body.to_vec())
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })
    }

    /// Fetch a body, revalidating against `cache` when possible.
    ///
    /// Returns `None` on `304 Not Modified`; the caller reads the cached copy
    /// with [`ETagCache::read`]. Used for the themed art, which is binary and
    /// wants its own ETag independent of the manifest that names it.
    pub async fn get_bytes_cached(
        &self,
        url: &str,
        cache: &ETagCache,
    ) -> Result<Option<Vec<u8>>, NetError> {
        let mut request = self.client.get(url);
        if let Some(etag) = cache.etag() {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }

        let response = request.send().await.map_err(|e| NetError::Request {
            url: url.to_owned(),
            source: e,
        })?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let response = check_status(url, response)?;

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let body = bounded_body(url, response, MAX_IMAGE_BYTES).await?;

        cache.store(&body, etag.as_deref())?;
        Ok(Some(body))
    }

    /// Fetch and deserialize JSON, without caching.
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, NetError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;

        let bytes = bounded_body(url, check_status(url, response)?, MAX_DOCUMENT_BYTES).await?;

        serde_json::from_slice(&bytes).map_err(|e| NetError::Decode {
            url: url.to_owned(),
            source: e,
        })
    }

    /// POST JSON and deserialize the reply.
    ///
    /// The addon catalog resolves a whole import list in one request, which is
    /// a body rather than a query string, twenty round trips to answer one
    /// paste is not a design.
    pub async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T, NetError> {
        let response = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;

        let bytes = bounded_body(url, check_status(url, response)?, MAX_DOCUMENT_BYTES).await?;

        serde_json::from_slice(&bytes).map_err(|e| NetError::Decode {
            url: url.to_owned(),
            source: e,
        })
    }

    /// Fetch a small document as text, without caching.
    pub async fn get_text(&self, url: &str) -> Result<String, NetError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NetError::Request {
                url: url.to_owned(),
                source: e,
            })?;

        let body = bounded_body(url, check_status(url, response)?, MAX_DOCUMENT_BYTES).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// Stream a download to `dest`, reporting progress and returning its
    /// SHA-512 digest.
    ///
    /// The digest is computed during the transfer rather than by re-reading the
    /// file afterwards, for a 400 MB tarball that saves a full pass over disk.
    /// `on_progress` is called as bytes arrive; keep it cheap.
    /// Fetch a whole response into memory.
    ///
    /// For manifests and configs, which are read whole and hashed whole.
    /// Bulk content goes through [`Http::download`] instead, which streams to
    /// disk rather than holding a multi-gigabyte archive in memory.
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, NetError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;
        let response = check_status(url, response)?;
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })
    }

    /// Fetch a byte range. `range` is a `Range` header value such as
    /// `bytes=0-1023`.
    ///
    /// The server is expected to answer 206; a 200 means it ignored the range
    /// and sent the whole resource, which for a 256 MB archive is a very
    /// expensive way to get one file and worth failing on rather than
    /// silently accepting.
    pub async fn get_range(&self, url: &str, range: &str) -> Result<Vec<u8>, NetError> {
        let response = self
            .client
            .get(url)
            .header(reqwest::header::RANGE, range)
            .send()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;

        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            let status = response.status();
            let response = check_status(url, response)?;
            drop(response);
            return Err(NetError::Status { url: url.to_owned(), status: status.as_u16() });
        }
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })
    }

    /// Stream `url` to `dest`, returning the SHA-512 of what was written.
    ///
    /// Written to a fresh, randomly named file beside `dest` and renamed only
    /// once complete, so `dest` existing means the whole file arrived. Callers
    /// skip a download whose file is already there, and a truncated one in its
    /// place used to be run, or unpacked, on every attempt after.
    ///
    /// The name is created exclusively, so nothing already sitting at it is
    /// followed or truncated, and the file deletes itself when the transfer
    /// fails or the future is dropped, a cancelled download included. The
    /// fixed `<dest>.part` this replaces survived a cancel.
    pub async fn download(
        &self,
        url: &str,
        dest: &Path,
        on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<String, NetError> {
        let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let io = |source| NetError::Io { path: dir.to_path_buf(), source };
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
        sweep_stale_downloads(dir);

        let temp = tempfile::Builder::new()
            .prefix(DOWNLOAD_PREFIX)
            .tempfile_in(dir)
            .map_err(io)?;
        let file = tokio::fs::File::from_std(temp.as_file().try_clone().map_err(io)?);
        let digest = self.download_to(url, temp.path(), file, on_progress).await?;

        temp.persist(dest)
            .map_err(|e| NetError::Io { path: dest.to_path_buf(), source: e.error })?;
        Ok(digest)
    }

    async fn download_to(
        &self,
        url: &str,
        dest: &Path,
        mut file: tokio::fs::File,
        mut on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<String, NetError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NetError::Request {
                url: url.to_owned(),
                source: e,
            })?;
        let response = check_status(url, response)?;
        let total = response.content_length();

        let mut hasher = Sha512::new();
        let mut written = 0u64;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| NetError::Request {
                url: url.to_owned(),
                source: e,
            })?;
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| NetError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?;
            written += chunk.len() as u64;
            on_progress(written, total);
        }

        // A connection closed cleanly mid-body is not an error to the stream.
        if let Some(total) = total.filter(|&total| total != written) {
            return Err(NetError::Truncated { url: url.to_owned(), expected: total, got: written });
        }
        file.flush().await.map_err(|e| NetError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;
        file.sync_all().await.map_err(|e| NetError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;

        Ok(hex(&hasher.finalize()))
    }
}

/// Read a whole body, failing once it passes `max` bytes.
///
/// Both the declared length and the bytes actually received are checked: a
/// chunked response declares nothing, and a declared one can be wrong.
async fn bounded_body(url: &str, response: reqwest::Response, max: usize) -> Result<Vec<u8>, NetError> {
    let too_large = || NetError::TooLarge { url: url.to_owned(), max };
    if response.content_length().is_some_and(|len| len > max as u64) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;
        if body.len() + chunk.len() > max {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Delete unfinished downloads a crash or a kill left in `dir`.
///
/// Their names are random, so unlike the old `.part` no later attempt ever
/// reuses one, and a Proton tarball is ~400 MB. Only stale ones go: another
/// download may be writing beside this one right now.
fn sweep_stale_downloads(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with(DOWNLOAD_PREFIX)
            && entry.metadata().is_ok_and(|m| {
                m.is_file() && m.modified().is_ok_and(|t| t.elapsed().is_ok_and(|age| age > STALE_DOWNLOAD))
            });
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn check_status(url: &str, response: reqwest::Response) -> Result<reqwest::Response, NetError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    tracing::debug!("http {} for {url}", status.as_u16());
    // GitHub answers 403 with a rate-limit body; call it out by name, since
    // "forbidden" sends people hunting for a credentials problem they don't have.
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        if let Some(reset) = response.headers().get("x-ratelimit-remaining") {
            if reset.to_str().ok() == Some("0") {
                tracing::warn!("rate limited by {url} ({})", status.as_u16());
                return Err(NetError::RateLimited {
                    url: url.to_owned(),
                });
            }
        }
    }
    Err(NetError::Status {
        url: url.to_owned(),
        status: status.as_u16(),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A cached response body plus the ETag that validates it.
///
/// Body and ETag are stored as two files next to each other; a missing or
/// unreadable ETag simply means the next request is unconditional.
#[derive(Debug, Clone)]
pub struct ETagCache {
    body_path: PathBuf,
}

impl ETagCache {
    pub fn new(body_path: impl Into<PathBuf>) -> Self {
        Self {
            body_path: body_path.into(),
        }
    }

    fn etag_path(&self) -> PathBuf {
        let mut p = self.body_path.clone().into_os_string();
        p.push(".etag");
        PathBuf::from(p)
    }

    /// The saved ETag, if the cached body is actually present.
    ///
    /// Both must exist, sending `If-None-Match` without a body to fall back on
    /// would turn a `304` into a failure.
    pub fn etag(&self) -> Option<String> {
        if !self.body_path.is_file() {
            return None;
        }
        std::fs::read_to_string(self.etag_path())
            .ok()
            .map(|s| s.trim().to_owned())
    }

    /// The cached body, used when the server reports `304`.
    pub fn read(&self) -> Result<Vec<u8>, NetError> {
        std::fs::read(&self.body_path).map_err(|e| NetError::Io {
            path: self.body_path.clone(),
            source: e,
        })
    }

    /// Read and deserialize the cached body.
    pub fn read_json<T: serde::de::DeserializeOwned>(&self) -> Result<T, NetError> {
        let bytes = self.read()?;
        serde_json::from_slice(&bytes).map_err(|e| NetError::Decode {
            url: self.body_path.display().to_string(),
            source: e,
        })
    }

    fn store(&self, body: &[u8], etag: Option<&str>) -> Result<(), NetError> {
        use std::io::Write;

        let dir = self.body_path.parent().unwrap_or_else(|| Path::new("."));
        let io = |p: &Path| {
            let p = p.to_path_buf();
            move |source| NetError::Io { path: p, source }
        };
        std::fs::create_dir_all(dir).map_err(io(dir))?;

        // The old ETag goes first and the new one last, so an interruption
        // anywhere in between leaves no ETag and the next request is
        // unconditional, never a 304 validated against the wrong body.
        match std::fs::remove_file(self.etag_path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&self.etag_path())(e)),
            _ => {}
        }

        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(io(dir))?;
        temp.write_all(body).map_err(io(temp.path()))?;
        temp.as_file().sync_all().map_err(io(temp.path()))?;
        temp.persist(&self.body_path).map_err(|e| io(&self.body_path)(e.error))?;

        if let Some(etag) = etag {
            std::fs::write(self.etag_path(), etag).map_err(io(&self.etag_path()))?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("could not build the HTTP client")]
    Client(#[source] reqwest::Error),

    #[error("request to {url} failed")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("{url} returned HTTP {status}")]
    Status { url: String, status: u16 },

    #[error("{url} sent more than the {max} bytes allowed")]
    TooLarge { url: String, max: usize },

    #[error("{url} ended after {got} of {expected} bytes")]
    Truncated { url: String, expected: u64, got: u64 },

    #[error("GitHub API rate limit reached for {url} — retry within the hour")]
    RateLimited { url: String },

    #[error("could not decode the response from {url}")]
    Decode {
        url: String,
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

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-net-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn etag_is_ignored_without_a_cached_body() {
        let dir = tmpdir("no-body");
        let cache = ETagCache::new(dir.join("doc.json"));
        std::fs::write(dir.join("doc.json.etag"), "\"abc\"").unwrap();

        // A 304 would leave nothing to serve, so the ETag must not be sent.
        assert_eq!(cache.etag(), None);
    }

    #[test]
    fn stored_body_and_etag_round_trip() {
        let dir = tmpdir("round-trip");
        let cache = ETagCache::new(dir.join("doc.json"));
        cache.store(br#"{"k":1}"#, Some("\"v1\"")).unwrap();

        assert_eq!(cache.etag().as_deref(), Some("\"v1\""));
        let parsed: serde_json::Value = cache.read_json().unwrap();
        assert_eq!(parsed["k"], 1);
    }

    #[test]
    fn storing_without_an_etag_clears_the_stale_one() {
        let dir = tmpdir("clear-etag");
        let cache = ETagCache::new(dir.join("doc.json"));
        cache.store(b"one", Some("\"v1\"")).unwrap();
        cache.store(b"two", None).unwrap();

        assert_eq!(cache.etag(), None);
        assert_eq!(cache.read().unwrap(), b"two");
    }

    #[test]
    fn hex_is_lowercase_and_zero_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    /// A server that answers every request with `head` and then `body`, and
    /// holds the connection open afterwards when `hold` is set.
    fn server(head: String, body: Vec<u8>, hold: bool) -> String {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for mut stream in listener.incoming().flatten() {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                if hold {
                    held.push(stream);
                }
            }
        });
        url
    }

    fn sized(body: &[u8], declared: usize) -> String {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n");
        server(head, body.to_vec(), false)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_finished_download_lands_whole_and_leaves_nothing_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("release.tar.gz");
        let digest = Http::new()
            .unwrap()
            .download(&sized(b"downloaded", 10), &dest, |_, _| {})
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"downloaded");
        assert_eq!(digest, hex(&Sha512::digest(b"downloaded")));
        assert_eq!(names(dir.path()), ["release.tar.gz"]);
    }

    #[tokio::test]
    async fn a_cancelled_download_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("release.tar.gz");
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\n\r\n".to_owned();
        let url = server(head, vec![0; 16], true);
        let http = Http::new().unwrap();

        // Dropping the future is how a cancel reaches it.
        let download = http.download(&url, &dest, |_, _| {});
        assert!(tokio::time::timeout(Duration::from_millis(300), download).await.is_err());

        assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
    }

    #[tokio::test]
    async fn a_truncated_download_keeps_the_file_it_would_have_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("release");
        std::fs::write(&dest, b"old complete version").unwrap();

        let result = Http::new().unwrap().download(&sized(b"partial", 100), &dest, |_, _| {}).await;

        assert!(result.is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"old complete version");
        assert_eq!(names(dir.path()), ["release"]);
    }

    #[test]
    fn only_stale_unfinished_downloads_are_swept() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(format!("{DOWNLOAD_PREFIX}old"));
        let live = dir.path().join(format!("{DOWNLOAD_PREFIX}live"));
        let other = dir.path().join("Battle.net-Setup.exe");
        for path in [&old, &live, &other] {
            std::fs::write(path, b"x").unwrap();
        }
        let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        for path in [&old, &other] {
            std::fs::File::options().write(true).open(path).unwrap().set_modified(two_hours_ago).unwrap();
        }

        sweep_stale_downloads(dir.path());

        assert!(!old.exists());
        assert!(live.exists(), "a download in progress beside this one was deleted");
        assert!(other.exists());
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_never_cached() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ETagCache::new(dir.path().join("releases.json"));
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n";
        let url = server(head.to_owned(), b"<html>oops".to_vec(), false);

        let result = Http::new().unwrap().get_json_cached::<serde_json::Value>(&url, &cache).await;

        assert!(matches!(result, Err(NetError::Decode { .. })), "{result:?}");
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn storing_leaves_no_temp_file_beside_the_body() {
        let dir = tmpdir("atomic-store");
        let cache = ETagCache::new(dir.join("doc.json"));
        cache.store(b"{}", Some("\"v1\"")).unwrap();

        assert_eq!(names(&dir), ["doc.json", "doc.json.etag"]);
    }

    #[tokio::test]
    async fn an_oversized_document_is_refused_by_its_declared_length_before_caching() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ETagCache::new(dir.path().join("manifest.json"));
        let url = sized(b"{}", MAX_DOCUMENT_BYTES + 1);

        let result = Http::new().unwrap().get_json_cached::<serde_json::Value>(&url, &cache).await;

        assert!(matches!(result, Err(NetError::TooLarge { .. })), "{result:?}");
        assert!(names(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn an_oversized_image_is_refused_by_what_arrives_when_no_length_is_declared() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ETagCache::new(dir.path().join("art.png"));
        // HTTP/1.0 without a length: the body runs until the connection closes.
        let url = server("HTTP/1.0 200 OK\r\n\r\n".to_owned(), vec![0; MAX_IMAGE_BYTES + 1], false);

        let result = Http::new().unwrap().get_bytes_cached(&url, &cache).await;

        assert!(matches!(result, Err(NetError::TooLarge { .. })), "{:?}", result.map(|_| ()));
        assert!(names(dir.path()).is_empty());
    }
}
