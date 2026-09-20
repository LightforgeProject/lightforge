//! The async edge.
//!
//! `core` is otherwise synchronous; everything that touches the network lives
//! behind `async fn` here and in its callers. TLS is rustls rather than the
//! system OpenSSL, so behavior doesn't drift with the distribution's crypto
//! policy or library version.
//!
//! The [`ETagCache`] built here is shared by Proton-GE release polling and, in
//! Milestone 5, the themed asset pipeline — both revalidate a small document on
//! every launch and want a `304` to cost nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt;

/// Sent on every request. GitHub rejects API calls without a User-Agent.
const USER_AGENT: &str = concat!("Lightforge/", env!("CARGO_PKG_VERSION"));

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
            // ~400 MB and a slow link is not an error.
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

        let body = response.bytes().await.map_err(|e| NetError::Request {
            url: url.to_owned(),
            source: e,
        })?;

        cache.store(&body, etag.as_deref())?;

        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|e| NetError::Decode {
                url: url.to_owned(),
                source: e,
            })
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

        let body = response.bytes().await.map_err(|e| NetError::Request {
            url: url.to_owned(),
            source: e,
        })?;

        cache.store(&body, etag.as_deref())?;
        Ok(Some(body.to_vec()))
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

        let bytes = check_status(url, response)?
            .bytes()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;

        serde_json::from_slice(&bytes).map_err(|e| NetError::Decode {
            url: url.to_owned(),
            source: e,
        })
    }

    /// POST JSON and deserialize the reply.
    ///
    /// The addon catalog resolves a whole import list in one request, which is
    /// a body rather than a query string — twenty round trips to answer one
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

        let bytes = check_status(url, response)?
            .bytes()
            .await
            .map_err(|e| NetError::Request { url: url.to_owned(), source: e })?;

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

        check_status(url, response)?
            .text()
            .await
            .map_err(|e| NetError::Request {
                url: url.to_owned(),
                source: e,
            })
    }

    /// Stream a download to `dest`, reporting progress and returning its
    /// SHA-512 digest.
    ///
    /// The digest is computed during the transfer rather than by re-reading the
    /// file afterwards — for a 400 MB tarball that saves a full pass over disk.
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

    pub async fn download(
        &self,
        url: &str,
        dest: &Path,
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

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| NetError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }

        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| NetError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?;

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

        file.flush().await.map_err(|e| NetError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;

        Ok(hex(&hasher.finalize()))
    }
}

fn check_status(url: &str, response: reqwest::Response) -> Result<reqwest::Response, NetError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // GitHub answers 403 with a rate-limit body; call it out by name, since
    // "forbidden" sends people hunting for a credentials problem they don't have.
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        if let Some(reset) = response.headers().get("x-ratelimit-remaining") {
            if reset.to_str().ok() == Some("0") {
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
    /// Both must exist — sending `If-None-Match` without a body to fall back on
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
        if let Some(parent) = self.body_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| NetError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::write(&self.body_path, body).map_err(|e| NetError::Io {
            path: self.body_path.clone(),
            source: e,
        })?;

        match etag {
            Some(etag) => std::fs::write(self.etag_path(), etag).map_err(|e| NetError::Io {
                path: self.etag_path(),
                source: e,
            })?,
            // No ETag offered: drop any stale one so the next request is
            // unconditional rather than validated against the wrong version.
            None => {
                let _ = std::fs::remove_file(self.etag_path());
            }
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
}
