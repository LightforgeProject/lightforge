//! The async edge.
//!
//! `core` is otherwise synchronous. TLS is rustls, so it does not vary with
//! the distribution's OpenSSL. [`ETagCache`] serves Proton release polling
//! and the asset pipeline.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt;

use crate::text::hex;

/// Sent on every request. GitHub rejects API calls without a User-Agent.
const USER_AGENT: &str = concat!("Lightforge/", env!("CARGO_PKG_VERSION"));

/// How long a response may go without sending a byte before it fails.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on a JSON or text document read whole; the largest real one,
/// GitHub's release list, is a few hundred KB.
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Ceiling on a cached picture, key art the largest of them.
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Ceiling on one CDN file read whole: an encoding table, an index or a loose
/// file. Measured on 12.1.5, the encoding table is 191 MiB and the largest
/// loose file 200 MiB.
pub const MAX_CDN_BYTES: usize = 1024 * 1024 * 1024;

/// Name prefix of an unfinished [`Http::download`].
const DOWNLOAD_PREFIX: &str = ".lightforge-download-";

/// How old an unfinished download must be before it is cleared: a live one
/// fails after [`READ_TIMEOUT`] of silence.
const STALE_DOWNLOAD: Duration = Duration::from_secs(60 * 60);

/// Shared HTTP client, one connection pool; clone it, do not build more.
#[derive(Debug, Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    /// One client, one connection pool. Several pools **measured slower** in
    /// the download loop (25.3 and 21.7 MB/s against 26.8 and 25.5), though
    /// isolated `curl` runs favoured them. Do not re-add without an A/B there.
    pub fn new() -> Result<Self, NetError> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            // No overall timeout (a slow link is fine); this one resets per
            // chunk and catches a server that goes silent.
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(NetError::Client)?;
        Ok(Self { client })
    }

    /// Fetch and deserialize JSON, revalidating against `cache`; `None` on `304`.
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

        // Decoded before caching, or a bad body is served on every 304.
        let value = serde_json::from_slice(&body).map_err(|e| NetError::Decode {
            url: url.to_owned(),
            source: e,
        })?;
        cache.store(&body, etag.as_deref())?;
        Ok(Some(value))
    }

    /// POST a body and read a small JSON answer back: a bug report's
    /// screenshot. Its own short timeout: someone is watching, and on failure
    /// the picture travels inside the report.
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

    /// POST a body with extra headers: a bug report to our Sentry server
    /// (`X-Sentry-Auth`), through this client, not a second HTTP stack.
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
    /// Returns `None` on `304`; the caller reads [`ETagCache::read`].
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

    /// Fetch a whole response into memory, refusing one past `max` bytes.
    ///
    /// For manifests and configs; bulk content streams through [`Http::download`].
    pub async fn get_bytes(&self, url: &str, max: usize) -> Result<Vec<u8>, NetError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;
        bounded_body(url, check_status(url, response)?, max).await
    }

    /// Fetch a byte range. `range` is a `Range` header value such as
    /// `bytes=0-1023`.
    ///
    /// Anything but 206 fails: a 200 is the whole archive.
    pub async fn get_range(&self, url: &str, range: &str) -> Result<Vec<u8>, NetError> {
        let want = range_len(range).ok_or_else(|| NetError::BadRange(range.to_owned()))?;
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
        // Exactly the bytes asked for, or the pieces fail far from the cause.
        let body = bounded_body(url, response, usize::try_from(want).unwrap_or(usize::MAX)).await?;
        if body.len() as u64 != want {
            return Err(NetError::Truncated { url: url.to_owned(), expected: want, got: body.len() as u64 });
        }
        Ok(body)
    }

    /// Stream `url` to `dest`, returning the SHA-512 of what was written.
    ///
    /// Into a randomly named file beside `dest`, created exclusively and
    /// renamed once complete, so `dest` existing means it all arrived. It
    /// deletes itself on failure or when the future is dropped.
    pub async fn download(
        &self,
        url: &str,
        dest: &Path,
        on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<String, NetError> {
        let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        tokio::fs::create_dir_all(dir).await.map_err(io(dir))?;
        sweep_stale_downloads(dir);

        let temp = tempfile::Builder::new()
            .prefix(DOWNLOAD_PREFIX)
            .tempfile_in(dir).map_err(io(dir))?;
        let file = tokio::fs::File::from_std(temp.as_file().try_clone().map_err(io(dir))?);
        let digest = self.download_to(url, temp.path(), file, on_progress).await?;

        temp.persist(dest).map_err(|e| io(dest)(e.error))?;
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
            file.write_all(&chunk).await.map_err(io(dest))?;
            written += chunk.len() as u64;
            on_progress(written, total);
        }

        // A connection closed cleanly mid-body is not an error to the stream.
        if let Some(total) = total.filter(|&total| total != written) {
            return Err(NetError::Truncated { url: url.to_owned(), expected: total, got: written });
        }
        file.flush().await.map_err(io(dest))?;
        file.sync_all().await.map_err(io(dest))?;

        Ok(hex(&hasher.finalize()))
    }
}

/// Read a whole body, failing once it passes `max` bytes.
///
/// Both the declared length and the bytes actually received are checked: a
/// chunked response declares nothing, and a declared one can be wrong.
async fn bounded_body(url: &str, response: reqwest::Response, max: usize) -> Result<Vec<u8>, NetError> {
    let too_large = || NetError::TooLarge { url: url.to_owned(), max };
    let declared = response.content_length();
    if declared.is_some_and(|len| len > max as u64) {
        return Err(too_large());
    }
    // Sized up front when declared.
    let mut body = Vec::with_capacity(declared.map_or(0, |len| len as usize));
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

/// Bytes covered by a `bytes=first-last` header value.
fn range_len(range: &str) -> Option<u64> {
    let (first, last) = range.strip_prefix("bytes=")?.split_once('-')?;
    let (first, last): (u64, u64) = (first.parse().ok()?, last.parse().ok()?);
    last.checked_sub(first)?.checked_add(1)
}

/// Delete unfinished downloads a crash or a kill left in `dir`.
///
/// Only stale ones: another download may be writing beside this one.
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
    // GitHub's rate limit is a 403; name it, not "forbidden".
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
    /// Both must exist, or a `304` has nothing to serve.
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
        std::fs::read(&self.body_path).map_err(io(&self.body_path))
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

        let dir = self.body_path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(io(dir))?;

        // Old ETag first, new one last: an interruption leaves none.
        match std::fs::remove_file(self.etag_path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(&self.etag_path())(e)),
            _ => {}
        }

        crate::fsutil::write_atomic(&self.body_path, body).map_err(io(&self.body_path))?;

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

    #[error("`{0}` is not a single byte range")]
    BadRange(String),

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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> NetError + '_ {
    move |source| NetError::Io { path: path.to_path_buf(), source }
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

    fn partial(body: &[u8], declared: usize) -> String {
        let head = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n");
        server(head, body.to_vec(), false)
    }

    #[test]
    fn a_range_header_is_measured_inclusively() {
        assert_eq!(range_len("bytes=0-0"), Some(1));
        assert_eq!(range_len("bytes=1000-1099"), Some(100));
        assert_eq!(range_len("bytes=5-4"), None);
        assert_eq!(range_len("bytes=0-"), None);
        assert_eq!(range_len("0-9"), None);
    }

    #[tokio::test]
    async fn a_range_answered_in_full_is_returned() {
        let body = Http::new().unwrap().get_range(&partial(b"0123456789", 10), "bytes=0-9").await.unwrap();
        assert_eq!(body, b"0123456789");
    }

    #[tokio::test]
    async fn a_range_answered_short_is_truncated_not_accepted() {
        let result = Http::new().unwrap().get_range(&partial(b"0123", 4), "bytes=0-9").await;
        assert!(matches!(result, Err(NetError::Truncated { expected: 10, got: 4, .. })), "{result:?}");
    }

    #[tokio::test]
    async fn a_range_answered_long_is_refused() {
        let result = Http::new().unwrap().get_range(&partial(&[0; 20], 20), "bytes=0-9").await;
        assert!(matches!(result, Err(NetError::TooLarge { max: 10, .. })), "{result:?}");
    }

    #[tokio::test]
    async fn a_whole_fetch_past_its_limit_is_refused() {
        let result = Http::new().unwrap().get_bytes(&sized(&[0; 64], 64), 63).await;
        assert!(matches!(result, Err(NetError::TooLarge { max: 63, .. })), "{result:?}");
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
