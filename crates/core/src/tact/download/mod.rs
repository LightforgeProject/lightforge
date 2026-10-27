//! The download loop: a [`Plan`] in, a CASC store out.
//!
//! Fetches overlap, writes do not. Runs and loose files are requested
//! concurrently, but every byte goes into the store through one
//! [`StoreWriter`] in the order it arrives, the writer appends to an archive
//! and owns the offsets, so there is exactly one of it and no lock to get
//! wrong.
//!
//! Three things happen to every file between the socket and the archive:
//!
//! 1. **Split.** A run's response covers several files plus the padding
//!    between them; each file is sliced back out at the offset the plan
//!    recorded.
//! 2. **Verify.** The bytes are decoded and checked against the content key.
//!    The encoding key names the encoded bytes but cannot check them, only
//!    `md5(decoded) == ckey` says the file is what the build says it is.
//! 3. **Store.** The *encoded* bytes are written, because that is what an
//!    archive holds. Decoding is for verification only; the decoded copy is
//!    dropped.
//!
//! Encrypted content (about 2% of a build, unreleased material) cannot be
//! decoded without a key this machine does not have. Those files are stored
//! unverified rather than refused, which is what Agent does, and the outcome
//! counts them so the caller can say so honestly. Unverified means the content
//! key went unchecked, not that nothing was: the encoded bytes must still match
//! their encoding key and every chunk its checksum, or one chunk marked
//! encrypted would carry anything past the check.
//!
//! Durability is the store's own: every [`Options::checkpoint_bytes`] the
//! writer flushes and rewrites its indices, leaving a store that opens and
//! reads. Resume is then nothing more than re-planning against it, there is
//! no progress file to get out of step with the bytes on disk.

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

/// A cooperative cancel token, checked between requests.
///
/// Cancellation is not abandonment: the loop stops taking new work, finishes
/// what is in flight, and checkpoints, so a cancelled download leaves a valid
/// store and resumes without refetching what it already had.
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

/// Resolves once `cancel` trips. Polled rather than notified: `Cancel` is a
/// plain flag shared with synchronous code, and a tenth of a second is far
/// inside what a pause click needs.
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
    /// Upper bound on the bytes stored between checkpoints.
    ///
    /// An *upper* bound, not the interval: a plan smaller than this would
    /// otherwise never checkpoint at all, and a download with no intermediate
    /// index write is one an interrupt costs entirely. Classic Era is 4.5 GiB
    /// against this 4 GiB default, 3.2 GB of it was orphaned by a restart
    /// before the first checkpoint was ever due. The effective interval is
    /// [`Options::interval_for`].
    pub checkpoint_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self { concurrency: 8, limit: None, checkpoint_bytes: 4 << 30 }
    }
}

/// Checkpoints a download aims for, however big it is.
///
/// Eight keeps an interrupt's cost near an eighth of the work while leaving
/// the generation counter in the range a real install's sits in.
const TARGET_CHECKPOINTS: u64 = 8;

/// Floor on the interval, so a tiny plan does not rewrite sixteen indices
/// every few megabytes for no gain.
const MIN_CHECKPOINT_BYTES: u64 = 64 << 20;

impl Options {
    /// The interval actually used for a plan of `total` bytes.
    ///
    /// Scales down for small products so every download checkpoints several
    /// times, rather than depending on whether it happens to exceed a constant
    /// tuned on a 123 GiB install.
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
    /// Completion as a fraction of bytes, which tracks time far better than a
    /// fraction of files: sizes here span 176 bytes to 209 MB.
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
/// decryption keys this machine has.
///
/// No `Debug`: `KeyService` has none, and printing key material would be the
/// wrong thing to add.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub http: &'a Http,
    pub client: &'a Client,
    /// Keys for encrypted content. `None` is normal: most machines have none,
    /// and the files that need them are stored unverified.
    pub keys: Option<&'a KeyService>,
}

/// A key service loaded the way [`super::storage::Storage`] loads one.
///
/// Every caller wants this, and the one that did not, the first Classic Era
/// download, which passed `None`: logged its encrypted files as corruption.
/// Exposed here so a front-end never has to name a cascette type to build one.
///
/// A machine with no key files is the normal case and not an error: the
/// built-in keys still decrypt some content, and what they cannot is stored
/// unverified.
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
