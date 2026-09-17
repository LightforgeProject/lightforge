//! The download loop: a [`Plan`] in, a CASC store out.
//!
//! Fetches overlap, writes do not. Runs and loose files are requested
//! concurrently, but every byte goes into the store through one
//! [`StoreWriter`] in the order it arrives — the writer appends to an archive
//! and owns the offsets, so there is exactly one of it and no lock to get
//! wrong.
//!
//! Three things happen to every file between the socket and the archive:
//!
//! 1. **Split.** A run's response covers several files plus the padding
//!    between them; each file is sliced back out at the offset the plan
//!    recorded.
//! 2. **Verify.** The bytes are decoded and checked against the content key.
//!    The encoding key names the encoded bytes but cannot check them — only
//!    `md5(decoded) == ckey` says the file is what the build says it is.
//! 3. **Store.** The *encoded* bytes are written, because that is what an
//!    archive holds. Decoding is for verification only; the decoded copy is
//!    dropped.
//!
//! Encrypted content (about 2% of a build — unreleased material) cannot be
//! decoded without a key this machine does not have. Those files are stored
//! unverified rather than refused, which is what Agent does, and the outcome
//! counts them so the caller can say so honestly.
//!
//! Durability is the store's own: every [`Options::checkpoint_bytes`] the
//! writer flushes and rewrites its indices, leaving a store that opens and
//! reads. Resume is then nothing more than re-planning against it — there is
//! no progress file to get out of step with the bytes on disk.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use ngdp_crypto::KeyService;

use super::cdn::{CdnError, Client};
use super::config::{to_hex, CKey};
use super::plan::Plan;
use super::storage::EKey;
use super::writer::{StoreWriter, WriteError};
use crate::net::Http;

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

/// How the loop is tuned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Requests in flight at once.
    pub concurrency: usize,
    /// Upper bound on the bytes stored between checkpoints.
    ///
    /// An *upper* bound, not the interval: a plan smaller than this would
    /// otherwise never checkpoint at all, and a download with no intermediate
    /// index write is one an interrupt costs entirely. Classic Era is 4.5 GiB
    /// against this 4 GiB default — 3.2 GB of it was orphaned by a restart
    /// before the first checkpoint was ever due. The effective interval is
    /// [`Options::interval_for`].
    pub checkpoint_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self { concurrency: 8, checkpoint_bytes: 4 << 30 }
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
    /// encoding. Expected to be non-zero — about 2% of a build is encrypted.
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
    /// Keys for encrypted content. `None` is normal — most machines have none,
    /// and the files that need them are stored unverified.
    pub keys: Option<&'a KeyService>,
}

/// A key service loaded the way [`super::storage::Storage`] loads one.
///
/// Every caller wants this, and the one that did not — the first Classic Era
/// download, which passed `None` — logged its encrypted files as corruption.
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

/// Drives a plan into a store.
#[derive(Debug, Clone, Copy, Default)]
pub struct Downloader {
    options: Options,
}

impl Downloader {
    pub fn new(options: Options) -> Self {
        Self { options }
    }

    /// Fetch everything in `plan` and write it through `writer`.
    ///
    /// `writer` is left open: the caller finishes it, so it can write the
    /// install-manifest files or `.build.info` first and still end with one
    /// index write.
    pub async fn run(
        &self,
        source: Source<'_>,
        plan: &Plan,
        writer: &mut StoreWriter,
        cancel: &Cancel,
        mut progress: impl FnMut(Progress),
    ) -> Result<Outcome, DownloadError> {
        let Source { http, client, keys } = source;
        let mut sink = Sink {
            writer,
            keys,
            outcome: Outcome::default(),
            state: Progress {
                files_total: plan.files(),
                bytes_total: plan.bytes(),
                requests_total: plan.requests(),
                ..Progress::default()
            },
            since_checkpoint: 0,
        };
        let checkpoint_every = self.options.interval_for(plan.bytes());

        // Runs first: they are the bulk, and the loose files are large enough
        // that leaving them last keeps the tail of the download responsive.
        let work = plan
            .runs
            .iter()
            .map(Work::Run)
            .chain(plan.loose.iter().map(Work::Loose));

        let mut fetches = stream::iter(work)
            .map(|item| async move {
                if cancel.is_cancelled() {
                    return Fetched::Skipped;
                }
                match &item {
                    Work::Run(run) => match client.fetch_range(http, &run.archive, &run.range()).await {
                        Ok(body) => Fetched::Run { run, body },
                        Err(source) => Fetched::Failed {
                            ekeys: run.pieces.iter().map(|p| p.ekey).collect(),
                            reason: source.to_string(),
                        },
                    },
                    Work::Loose(loose) => {
                        match client.fetch(http, super::cdn::Kind::Data, &to_hex(&loose.ekey)).await {
                            Ok(body) => Fetched::Loose { ekey: loose.ekey, ckey: loose.ckey, body },
                            Err(source) => Fetched::Failed {
                                ekeys: vec![loose.ekey],
                                reason: source.to_string(),
                            },
                        }
                    }
                }
            })
            .buffer_unordered(self.options.concurrency.max(1));

        while let Some(fetched) = fetches.next().await {
            match fetched {
                Fetched::Skipped => continue,
                Fetched::Failed { ekeys, reason } => {
                    sink.state.requests_done += 1;
                    for ekey in ekeys {
                        sink.outcome.failed.push(Failure { ekey, reason: reason.clone() });
                    }
                }
                Fetched::Run { run, body } => {
                    sink.state.requests_done += 1;
                    for piece in &run.pieces {
                        match run.slice(&body, piece) {
                            Some(bytes) => sink.store(&piece.ekey, piece.ckey.as_ref(), bytes)?,
                            None => sink.outcome.failed.push(Failure {
                                ekey: piece.ekey,
                                reason: format!(
                                    "range request returned {} bytes, too short for a file at +{} of {}",
                                    body.len(),
                                    piece.at,
                                    piece.size
                                ),
                            }),
                        }
                    }
                }
                Fetched::Loose { ekey, ckey, body } => {
                    sink.state.requests_done += 1;
                    sink.store(&ekey, ckey.as_ref(), &body)?;
                }
            }

            if sink.since_checkpoint >= checkpoint_every {
                sink.writer.checkpoint()?;
                sink.since_checkpoint = 0;
            }

            progress(sink.state);
        }

        let mut outcome = sink.outcome;
        outcome.cancelled = cancel.is_cancelled();
        outcome.files_stored = sink.state.files_stored;
        outcome.bytes_stored = sink.state.bytes_stored;

        // Whatever happened — finished, failed or cancelled — leave the store
        // readable, so the next run re-plans against real bytes.
        sink.writer.checkpoint()?;

        Ok(outcome)
    }
}

/// The downloads this process has running, keyed by product.
///
/// The real [`crate::preflight::DownloadGuard`], replacing `NoDownloads`. The
/// hazard it exists for is concrete: Agent and Lightforge both writing the
/// same `data.NNN` and `.idx` files is corruption, not a race to win, so a
/// launch pauses any download first and the store is left valid before
/// Battle.net is allowed to start.
///
/// "Resume" here does not restart a transfer — there is no suspended socket to
/// revive. A cancelled download checkpointed its store on the way out, so
/// resuming is re-planning against it, which needs the network and the
/// manifests and therefore belongs to the caller. [`Downloads::resume`]
/// reports *that* a product needs restarting; acting on it is the app's job.
#[derive(Debug, Clone, Default)]
pub struct Downloads {
    running: std::collections::HashMap<crate::ProductCode, Cancel>,
    paused: std::collections::HashSet<crate::ProductCode>,
}

impl Downloads {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a download as running, so a launch can pause it.
    pub fn register(&mut self, code: crate::ProductCode, cancel: Cancel) {
        self.paused.remove(&code);
        self.running.insert(code, cancel);
    }

    /// Drop a download that finished on its own. Not the same as pausing one:
    /// nothing is owed afterwards.
    pub fn finished(&mut self, code: &crate::ProductCode) {
        self.running.remove(code);
        self.paused.remove(code);
    }

    pub fn is_running(&self, code: &crate::ProductCode) -> bool {
        self.running.contains_key(code)
    }

    /// Products paused by a launch and not yet restarted.
    pub fn pending(&self) -> impl Iterator<Item = &crate::ProductCode> {
        self.paused.iter()
    }
}

impl crate::preflight::DownloadGuard for Downloads {
    fn pause(&mut self, code: &crate::ProductCode) -> Result<bool, crate::preflight::PreflightError> {
        match self.running.remove(code) {
            Some(cancel) => {
                cancel.cancel();
                self.paused.insert(code.clone());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn resume(&mut self, code: &crate::ProductCode) -> Result<bool, crate::preflight::PreflightError> {
        Ok(self.paused.remove(code))
    }
}

/// The three lookups that turn a content key into a fetch: what it encodes
/// to, which archive holds it, and whether it is loose instead.
#[derive(Debug, Clone, Copy)]
pub struct Catalog<'a> {
    pub encoding: &'a super::encoding::Encoding,
    pub locations: &'a super::indices::Locations,
    pub loose: &'a super::archive::FileIndex,
}

/// One install-manifest file, written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub path: std::path::PathBuf,
    pub bytes: u64,
}

/// What extracting the install-manifest files achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallReport {
    pub written: Vec<Extracted>,
    pub failed: Vec<(String, String)>,
}

impl InstallReport {
    pub fn bytes(&self) -> u64 {
        self.written.iter().map(|e| e.bytes).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Write the install manifest's files into the flavor directory.
///
/// These are the files that exist as real files rather than as CASC content —
/// `Wow.exe`, `wow_loader.dll`, CEF and its `.pak` resources — and without
/// them there is nothing to launch. They are **not** written into the store:
/// measured against a real install, only 12 of the 36 are in Agent's store,
/// and those 12 are there because the download manifest lists them
/// independently, not because they came from here.
///
/// `dest` is the flavor directory (`<install>/_retail_`), since manifest paths
/// are relative to it. Each file is verified against its content key before it
/// lands, and written via a temporary file so a failure cannot leave a
/// half-written `Wow.exe` behind.
pub async fn extract_install(
    source: Source<'_>,
    entries: &[&super::manifest::InstallEntry],
    catalog: Catalog<'_>,
    dest: &std::path::Path,
    cancel: &Cancel,
    mut progress: impl FnMut(&Extracted),
) -> Result<InstallReport, DownloadError> {
    let Source { http, client, keys } = source;
    let Catalog { encoding, locations, loose } = catalog;
    let mut report = InstallReport::default();

    for entry in entries {
        if cancel.is_cancelled() {
            break;
        }

        let fail = |report: &mut InstallReport, reason: String| {
            report.failed.push((entry.path.clone(), reason));
        };

        let Some(ekey) = encoding.ekey_for(&entry.ckey) else {
            fail(&mut report, format!("encoding does not list content key {}", to_hex(&entry.ckey)));
            continue;
        };

        // The same dispatch the plan makes: archived keys come out of a range
        // request, loose ones whole.
        let encoded = if let Some((archive, located)) = locations.get(&ekey) {
            client.fetch_range(http, archive, &located.range()).await
        } else if loose.contains(&ekey) {
            client.fetch(http, super::cdn::Kind::Data, &to_hex(&ekey)).await
        } else {
            fail(&mut report, format!("encoding key {} is in no index", to_hex(&ekey)));
            continue;
        };

        let encoded = match encoded {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(&mut report, e.to_string());
                continue;
            }
        };

        let decoded = match blte::decompress_blte(encoded, keys) {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(&mut report, e.to_string());
                continue;
            }
        };

        // Unlike store content, an install file is never stored unverified:
        // these are executables, and one that does not match its content key
        // is not something to write to disk and hope.
        let actual = *md5::compute(&decoded);
        if actual != entry.ckey {
            fail(
                &mut report,
                format!("decoded to {} but the build says {}", to_hex(&actual), to_hex(&entry.ckey)),
            );
            continue;
        }

        let path = dest.join(entry.relative_path());
        match write_file(&path, &decoded) {
            Ok(()) => {
                let written = Extracted { path, bytes: decoded.len() as u64 };
                progress(&written);
                report.written.push(written);
            }
            Err(e) => fail(&mut report, e.to_string()),
        }
    }

    Ok(report)
}

/// Write a file and its parent directories, via a temporary file alongside.
fn write_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;

    // tempfile creates at 0600; these are files Wine and the client read, and
    // a launcher that silently tightens permissions on Wow.exe is a bug.
    temp.persist(path).map_err(|e| e.error)?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o644))
}

/// The write side: one store, and the tally of what went into it.
struct Sink<'a> {
    writer: &'a mut StoreWriter,
    keys: Option<&'a KeyService>,
    outcome: Outcome,
    state: Progress,
    since_checkpoint: u64,
}

impl Sink<'_> {
    /// Verify one file and write it, counting what happened.
    fn store(&mut self, ekey: &EKey, ckey: Option<&CKey>, encoded: &[u8]) -> Result<(), DownloadError> {
        match verify(encoded, ckey, self.keys) {
            Verified::Ok => {}
            Verified::Unverifiable => self.outcome.unverified += 1,
            Verified::Wrong(reason) => {
                // Storing a file that failed its own content key would put
                // corruption into the store under a name that says it is fine.
                self.outcome.failed.push(Failure { ekey: *ekey, reason });
                return Ok(());
            }
        }

        let entry = self.writer.write(ekey, encoded)?;
        self.state.files_stored += 1;
        self.state.bytes_stored += u64::from(entry.size);
        self.since_checkpoint += u64::from(entry.size);
        Ok(())
    }
}

enum Verified {
    Ok,
    /// Encrypted without a key, or absent from encoding: store it anyway.
    Unverifiable,
    Wrong(String),
}

/// Decode and check against the content key.
fn verify(encoded: &[u8], ckey: Option<&CKey>, keys: Option<&KeyService>) -> Verified {
    let Some(ckey) = ckey else {
        return Verified::Unverifiable;
    };

    match blte::decompress_blte(encoded.to_vec(), keys) {
        Ok(decoded) => {
            let actual = *md5::compute(&decoded);
            if &actual == ckey {
                Verified::Ok
            } else {
                Verified::Wrong(format!(
                    "decoded to {} but the build says {}",
                    to_hex(&actual),
                    to_hex(ckey)
                ))
            }
        }
        // Classify a failure by what the bytes ARE, not by what the decoder
        // says about them. `blte` 0.4 cannot decode an encrypted block at all
        // (see [`has_encrypted_chunk`]), so its error text does not reliably
        // distinguish "withheld key" from "corrupt".
        Err(e) => {
            if has_encrypted_chunk(encoded) {
                Verified::Unverifiable
            } else {
                Verified::Wrong(e.to_string())
            }
        }
    }
}

/// Whether any chunk of a BLTE payload is encrypted.
///
/// Read directly out of the chunk table rather than inferred from a decode
/// error, because neither of `blte` 0.4's failure modes is trustworthy here:
///
/// - With no key service it returns an untyped
///   `DecompressionFailed("Key service required for encrypted blocks")`.
/// - With one, it cannot parse the block at all. Its encrypted-block reader
///   takes the key-name size as a little-endian **u64** where the format
///   stores a single byte, so it consumes the size plus seven bytes of the key
///   name and reports nonsense: "Invalid key name size: 9560733477763426568".
///   That value is `0829466e718cae84` little-endian — low byte `0x08`, the
///   real size. Verified against two different real files.
///
/// Unreleased content ships encrypted with the key withheld until it goes
/// live, and Agent stores it regardless, so these files are stored unverified.
/// Refusing them cost a real Classic Era download 7 files.
///
/// ```text
/// "BLTE"            4 bytes
/// header_size       u32 BE   0 means one chunk, data immediately after
/// flags             u8
/// chunk_count       u24 BE
/// chunk_count x     compressed u32 BE, decompressed u32 BE, md5 16 bytes
/// <data>            each chunk starts with its mode byte
/// ```
fn has_encrypted_chunk(blte: &[u8]) -> bool {
    /// Mode byte of an encrypted chunk.
    const ENCRYPTED: u8 = b'E';

    if blte.len() < 8 || &blte[..4] != b"BLTE" {
        return false;
    }
    let header_size = u32::from_be_bytes(blte[4..8].try_into().expect("4 bytes")) as usize;

    // A zero header means a single chunk whose data starts right here.
    if header_size == 0 {
        return blte.get(8) == Some(&ENCRYPTED);
    }
    if header_size > blte.len() || blte.len() < 12 {
        return false;
    }

    let chunk_count = u32::from_be_bytes([0, blte[9], blte[10], blte[11]]) as usize;
    let mut table = 12;
    let mut at = header_size;

    for _ in 0..chunk_count {
        let Some(entry) = blte.get(table..table + 24) else {
            return false;
        };
        let compressed = u32::from_be_bytes(entry[..4].try_into().expect("4 bytes")) as usize;

        if blte.get(at) == Some(&ENCRYPTED) {
            return true;
        }
        table += 24;
        match at.checked_add(compressed) {
            Some(next) if next <= blte.len() => at = next,
            _ => return false,
        }
    }

    false
}

enum Work<'a> {
    Run(&'a super::plan::Run),
    Loose(&'a super::plan::Loose),
}

enum Fetched<'a> {
    Run { run: &'a super::plan::Run, body: Vec<u8> },
    Loose { ekey: EKey, ckey: Option<CKey>, body: Vec<u8> },
    Failed { ekeys: Vec<EKey>, reason: String },
    Skipped,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error(transparent)]
    Write(#[from] WriteError),

    #[error(transparent)]
    Cdn(#[from] CdnError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ckey_of(payload: &[u8]) -> CKey {
        *md5::compute(payload)
    }

    fn blte(payload: &[u8]) -> Vec<u8> {
        blte::compress_data_single(payload.to_vec(), blte::CompressionMode::ZLib, None).unwrap()
    }

    #[test]
    fn a_file_matching_its_content_key_verifies() {
        let encoded = blte(b"the real bytes");
        assert!(matches!(
            verify(&encoded, Some(&ckey_of(b"the real bytes")), None),
            Verified::Ok
        ));
    }

    #[test]
    fn a_file_that_decodes_to_something_else_is_refused_not_stored() {
        let encoded = blte(b"the real bytes");
        let wrong = ckey_of(b"something else entirely");

        match verify(&encoded, Some(&wrong), None) {
            Verified::Wrong(reason) => {
                // The message has to name both, or a mismatch is unactionable.
                assert!(reason.contains(&to_hex(&wrong)), "{reason}");
                assert!(reason.contains(&to_hex(&ckey_of(b"the real bytes"))), "{reason}");
            }
            _ => panic!("a content-key mismatch must not pass"),
        }
    }

    #[test]
    fn a_file_encoding_does_not_list_is_stored_unverified() {
        // The 2-in-1.5-million case: no content key to check it against.
        assert!(matches!(verify(&blte(b"x"), None, None), Verified::Unverifiable));
    }

    /// A BLTE container with an explicit chunk table.
    fn blte_chunks(modes: &[u8]) -> Vec<u8> {
        let header_size = 12 + 24 * modes.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"BLTE");
        out.extend_from_slice(&(header_size as u32).to_be_bytes());
        out.push(0x0F);
        out.extend_from_slice(&(modes.len() as u32).to_be_bytes()[1..]);

        // Each chunk here is one mode byte plus three of payload.
        for _ in modes {
            out.extend_from_slice(&4u32.to_be_bytes());
            out.extend_from_slice(&3u32.to_be_bytes());
            out.extend_from_slice(&[0u8; 16]);
        }
        for mode in modes {
            out.push(*mode);
            out.extend_from_slice(b"abc");
        }
        out
    }

    #[test]
    fn an_encrypted_chunk_is_recognised_wherever_it_sits() {
        assert!(has_encrypted_chunk(&blte_chunks(b"E")));
        // Blizzard mixes modes within one file, so a later chunk counts too.
        assert!(has_encrypted_chunk(&blte_chunks(b"ZZE")));
        assert!(has_encrypted_chunk(&blte_chunks(b"ZEZ")));

        assert!(!has_encrypted_chunk(&blte_chunks(b"Z")));
        assert!(!has_encrypted_chunk(&blte_chunks(b"ZNZ")));
    }

    #[test]
    fn a_single_chunk_container_declares_no_header() {
        // header_size 0: one chunk, its mode byte immediately after.
        let mut encrypted = Vec::from(*b"BLTE");
        encrypted.extend_from_slice(&0u32.to_be_bytes());
        encrypted.extend_from_slice(b"Epayload");
        assert!(has_encrypted_chunk(&encrypted));

        let mut plain = Vec::from(*b"BLTE");
        plain.extend_from_slice(&0u32.to_be_bytes());
        plain.extend_from_slice(b"Zpayload");
        assert!(!has_encrypted_chunk(&plain));
    }

    #[test]
    fn malformed_containers_are_not_encrypted_and_do_not_panic() {
        assert!(!has_encrypted_chunk(b""));
        assert!(!has_encrypted_chunk(b"BLTE"));
        assert!(!has_encrypted_chunk(b"not blte at all"));

        // A header claiming more than the file holds must not index past it.
        let mut lying = Vec::from(*b"BLTE");
        lying.extend_from_slice(&9999u32.to_be_bytes());
        lying.extend_from_slice(&[0u8; 8]);
        assert!(!has_encrypted_chunk(&lying));

        // A chunk count larger than the table.
        let mut truncated = blte_chunks(b"ZZE");
        truncated.truncate(20);
        assert!(!has_encrypted_chunk(&truncated));
    }

    #[test]
    fn an_encrypted_file_is_stored_unverified_rather_than_refused() {
        // The end-to-end shape of the Classic Era bug: a real encrypted file
        // fails to decode, and must still reach the store.
        let encrypted = blte_chunks(b"E");
        assert!(matches!(
            verify(&encrypted, Some(&ckey_of(b"whatever")), None),
            Verified::Unverifiable
        ));

        // Corruption that is not encrypted is still refused.
        assert!(matches!(
            verify(&blte_chunks(b"Z"), Some(&ckey_of(b"whatever")), None),
            Verified::Wrong(_)
        ));
    }

    #[test]
    fn junk_is_refused_rather_than_counted_as_unverifiable() {
        // Corruption must not be mistaken for encryption, or a broken download
        // would quietly fill the store.
        match verify(b"not blte at all", Some(&ckey_of(b"x")), None) {
            Verified::Wrong(_) => {}
            _ => panic!("junk must be refused"),
        }
    }

    #[test]
    fn an_extracted_file_lands_readable_with_its_directories_made() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("lightforge-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Utils").join("swiftshader").join("libEGL.dll");

        write_file(&path, b"MZ...").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"MZ...");
        // tempfile would leave these 0600; Wine and the client have to read them.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "{mode:o}");

        // Overwriting an existing file works, and leaves no temporary behind.
        write_file(&path, b"MZ newer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"MZ newer");
        let siblings: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().collect();
        assert_eq!(siblings.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_install_report_totals_what_it_wrote() {
        let mut report = InstallReport::default();
        report.written.push(Extracted { path: "Wow.exe".into(), bytes: 74_778_320 });
        report.written.push(Extracted { path: "wow_loader.dll".into(), bytes: 40_693_456 });

        assert_eq!(report.bytes(), 115_471_776);
        assert!(report.is_complete());

        report.failed.push(("Utils/cef.pak".into(), "connection reset".into()));
        assert!(!report.is_complete());
    }

    #[test]
    fn pausing_a_download_trips_its_token_and_leaves_a_restart_owed() {
        use crate::preflight::DownloadGuard;

        let code: crate::ProductCode = "wow".parse().unwrap();
        let cancel = Cancel::new();
        let mut downloads = Downloads::new();
        downloads.register(code.clone(), cancel.clone());
        assert!(downloads.is_running(&code));

        // The launch path pauses: the loop stops, and a restart is owed.
        assert!(downloads.pause(&code).unwrap());
        assert!(cancel.is_cancelled());
        assert!(!downloads.is_running(&code));
        assert_eq!(downloads.pending().count(), 1);

        // After Battle.net exits, the caller is told to start it again — once.
        assert!(downloads.resume(&code).unwrap());
        assert!(!downloads.resume(&code).unwrap());
        assert_eq!(downloads.pending().count(), 0);
    }

    #[test]
    fn a_product_with_no_download_owes_nothing_either_way() {
        use crate::preflight::DownloadGuard;

        let code: crate::ProductCode = "wow".parse().unwrap();
        let mut downloads = Downloads::new();

        // Pausing nothing must report nothing, or a launch would wait on a
        // resume that is never owed.
        assert!(!downloads.pause(&code).unwrap());
        assert!(!downloads.resume(&code).unwrap());
    }

    #[test]
    fn a_download_that_finished_on_its_own_is_not_owed_a_restart() {
        use crate::preflight::DownloadGuard;

        let code: crate::ProductCode = "wow".parse().unwrap();
        let mut downloads = Downloads::new();
        downloads.register(code.clone(), Cancel::new());
        downloads.finished(&code);

        assert!(!downloads.pause(&code).unwrap());
        assert!(!downloads.resume(&code).unwrap());
    }

    #[test]
    fn a_small_download_still_checkpoints_several_times() {
        let options = Options::default();

        // The bug this guards: Classic Era is 4.5 GiB against a 4 GiB default,
        // so it would have checkpointed once at the very end, if at all. A
        // restart orphaned 3.2 GB because no index had ever been written.
        let classic_era = 4_900_000_000u64;
        let interval = options.interval_for(classic_era);
        assert!(interval < options.checkpoint_bytes);
        assert!(classic_era / interval >= 6, "only {} checkpoints", classic_era / interval);

        // A big install still honours the cap rather than checkpointing on an
        // eighth of 123 GiB.
        let retail = 132_000_000_000u64;
        assert_eq!(options.interval_for(retail), options.checkpoint_bytes);

        // A tiny plan does not rewrite sixteen indices every few megabytes.
        assert_eq!(options.interval_for(1_000_000), MIN_CHECKPOINT_BYTES);
        assert!(options.interval_for(0) > 0);
    }

    #[test]
    fn a_cancel_token_is_shared_between_clones() {
        let cancel = Cancel::new();
        let clone = cancel.clone();
        assert!(!cancel.is_cancelled());

        clone.cancel();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn progress_is_measured_in_bytes_not_files() {
        let mut state = Progress { bytes_total: 1000, ..Progress::default() };
        assert_eq!(state.fraction(), 0.0);
        state.bytes_stored = 250;
        assert_eq!(state.fraction(), 0.25);

        // A plan with nothing to do is finished, not divided by zero.
        assert_eq!(Progress::default().fraction(), 1.0);
    }

    #[test]
    fn an_outcome_is_complete_only_when_nothing_failed_and_nothing_was_cancelled() {
        let mut outcome = Outcome { files_stored: 10, ..Outcome::default() };
        assert!(outcome.is_complete());

        // Unverified files are expected and do not make a run incomplete.
        outcome.unverified = 3;
        assert!(outcome.is_complete());

        outcome.cancelled = true;
        assert!(!outcome.is_complete());
        outcome.cancelled = false;
        outcome.failed.push(Failure { ekey: [0; 16], reason: "timed out".into() });
        assert!(!outcome.is_complete());
    }
}
