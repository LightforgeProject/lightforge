//! The download loop: a [`Plan`](super::plan::Plan) in, a CASC store out.
//!
//! Fetches overlap, writes do not: every byte goes through one
//! [`StoreWriter`](super::writer::StoreWriter), which owns the archive offsets.
//!
//! Three things happen to every file between the socket and the archive:
//!
//! 1. **Split.** A run's response covers several files plus the padding
//!    between them; each file is sliced back out at the offset the plan
//!    recorded.
//! 2. **Verify.** Decoded and checked: `md5(decoded) == ckey`.
//! 3. **Store.** The *encoded* bytes, as an archive holds them.
//!
//! Encrypted content (about 2% of a build) cannot be decoded without its
//! key, so like Agent we store it unverified, counted in the outcome. The
//! encoded bytes must still match their encoding key and every chunk its
//! checksum.
//!
//! Every [`Options::checkpoint_bytes`] the writer rewrites its indices, so
//! resuming is re-planning against the store; there is no progress file.

mod downloader;
mod guard;
mod install_files;
mod rate;
#[cfg(test)]
mod tests;

pub use downloader::Downloader;
pub use guard::Downloads;
pub use install_files::{extract_install, write_flavor_info, Catalog, Extracted, InstallReport};
pub use rate::RateLimit;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ngdp_crypto::KeyService;

use super::cdn::{CdnError, Client};
use super::storage::EKey;
use super::writer::WriteError;
use crate::net::Http;
// The tests reach the verifier and the install-file writer directly.
#[cfg(test)]
use {downloader::*, install_files::*};

/// A cooperative cancel token. The loop stops taking work and checkpoints,
/// so a cancelled download resumes without refetching.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Resolves once `cancel` trips. Polled: `Cancel` is a plain flag shared with
/// synchronous code.
async fn cancelled(cancel: &Cancel) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// How the loop is tuned.
#[derive(Debug, Clone)]
pub struct Options {
    /// Requests in flight at once.
    pub concurrency: usize,
    /// Cap on how fast bytes may be pulled, shared across the run.
    pub limit: Option<RateLimit>,
    /// Upper bound on the bytes stored between checkpoints; the interval is
    /// [`Options::interval_for`].
    pub checkpoint_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self { concurrency: 8, limit: None, checkpoint_bytes: 4 << 30 }
    }
}

/// Checkpoints a download aims for: an interrupt costs about an eighth, and
/// the generation counter stays in a real install's range.
const TARGET_CHECKPOINTS: u64 = 8;

/// Floor on the interval, so a tiny plan does not rewrite sixteen indices
/// every few megabytes.
const MIN_CHECKPOINT_BYTES: u64 = 64 << 20;

impl Options {
    /// The interval for a plan of `total` bytes, scaled so small products
    /// checkpoint several times too.
    pub fn interval_for(&self, total: u64) -> u64 {
        self.checkpoint_bytes
            .min((total / TARGET_CHECKPOINTS).max(MIN_CHECKPOINT_BYTES))
            .max(1)
    }
}

/// Where the download has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub files_stored: usize,
    pub files_total: usize,
    /// Bytes written into the store.
    pub bytes_stored: u64,
    /// Bytes the plan expects to store in total.
    pub bytes_total: u64,
    pub requests_done: usize,
    pub requests_total: usize,
}

impl Progress {
    /// Completion as a fraction of bytes: file sizes span 176 bytes to 209 MB.
    pub fn fraction(&self) -> f64 {
        if self.bytes_total == 0 {
            return 1.0;
        }
        (self.bytes_stored as f64 / self.bytes_total as f64).clamp(0.0, 1.0)
    }
}

/// What a download run achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub files_stored: usize,
    pub bytes_stored: u64,
    /// Stored without a content-key check: encrypted, or not listed by
    /// encoding. Expected to be non-zero, about 2% of a build is encrypted.
    pub unverified: usize,
    /// Files that could not be fetched or failed verification.
    pub failed: Vec<Failure>,
    /// True if the loop stopped early because the token was tripped.
    pub cancelled: bool,
}

impl Outcome {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty() && !self.cancelled
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub ekey: EKey,
    pub reason: String,
}

/// Where bytes come from: the HTTP client, the CDN endpoints, and whatever
/// decryption keys this machine has. No `Debug`: keys stay out of logs.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub http: &'a Http,
    pub client: &'a Client,
    /// Keys for encrypted content. `None` is normal: most machines have none,
    /// and the files that need them are stored unverified.
    pub keys: Option<&'a KeyService>,
}

/// A key service loaded the way [`super::storage::Storage`] loads one, so a
/// front end never names a cascette type. No key files is normal: the
/// built-in keys still decrypt some content.
pub fn default_key_service() -> KeyService {
    let mut keys = KeyService::new();
    let _ = keys.load_from_standard_dirs();
    keys
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error(transparent)]
    Write(#[from] WriteError),

    #[error(transparent)]
    Cdn(#[from] CdnError),
}
