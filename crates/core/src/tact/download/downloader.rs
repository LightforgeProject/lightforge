use std::pin::pin;

use futures_util::future::{self, Either};
use futures_util::stream::{self, StreamExt};
use ngdp_crypto::KeyService;

use super::{cancelled, Cancel, DownloadError, Failure, Options, Outcome, Progress, Source};
use crate::tact::config::{to_hex, CKey};
use crate::tact::plan::Plan;
use crate::tact::storage::EKey;
use crate::tact::writer::StoreWriter;

/// Drives a plan into a store.
#[derive(Debug, Clone, Default)]
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
        let limit = self.options.limit.clone();

        // Runs first: they are the bulk, and the loose files are large enough
        // that leaving them last keeps the tail of the download responsive.
        let work = plan
            .runs
            .iter()
            .map(Work::Run)
            .chain(plan.loose.iter().map(Work::Loose));

        let mut fetches = stream::iter(work)
            .map(|item| {
                let limit = limit.clone();
                async move {
                if cancel.is_cancelled() {
                    return Fetched::Skipped;
                }
                // Paid before the request goes out, so the limit shapes what
                // is asked for rather than what has already arrived.
                if let Some(limit) = &limit {
                    limit.acquire(item.bytes()).await;
                }
                match &item {
                    Work::Run(run) => {
                        let ekeys = || run.pieces.iter().map(|p| p.ekey).collect();
                        let Some(range) = run.range() else {
                            return Fetched::Failed {
                                ekeys: ekeys(),
                                reason: format!("empty run in archive {}", run.archive),
                            };
                        };
                        match client.fetch_range(http, &run.archive, &range).await {
                            Ok(body) => Fetched::Run { run, body },
                            Err(source) => Fetched::Failed { ekeys: ekeys(), reason: source.to_string() },
                        }
                    }
                    Work::Loose(loose) => {
                        match client.fetch(http, crate::tact::cdn::Kind::Data, &to_hex(&loose.ekey)).await {
                            Ok(body) => Fetched::Loose { ekey: loose.ekey, ckey: loose.ckey, body },
                            Err(source) => Fetched::Failed {
                                ekeys: vec![loose.ekey],
                                reason: source.to_string(),
                            },
                        }
                    }
                }
                }
            })
            .buffer_unordered(self.options.concurrency.max(1));

        // Raced against the pause, rather than checked between fetches: up to
        // `concurrency` range requests are in flight at once, each up to
        // hundreds of megabytes, and a pause that waited for them all read as
        // a hang. What is dropped here was never stored, so the next run's
        // plan simply asks for it again.
        while let Either::Left((Some(fetched), _)) =
            future::select(fetches.next(), pin!(cancelled(cancel))).await
        {
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

        // Whatever happened, finished, failed or cancelled, leave the store
        // readable, so the next run re-plans against real bytes.
        sink.writer.checkpoint()?;

        Ok(outcome)
    }
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
        match verify(encoded, ekey, ckey, self.keys) {
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

pub(super) enum Verified {
    Ok,
    /// Encrypted without a key, or absent from encoding: store it anyway.
    Unverifiable,
    Wrong(String),
}

/// Decode and check against the content key.
pub(super) fn verify(encoded: &[u8], ekey: &EKey, ckey: Option<&CKey>, keys: Option<&KeyService>) -> Verified {
    let Some(ckey) = ckey else {
        return unverifiable(encoded, ekey);
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
                unverifiable(encoded, ekey)
            } else {
                Verified::Wrong(e.to_string())
            }
        }
    }
}

/// A file whose content key cannot be checked, held to what can be: the
/// same test a store scan applies ([`crate::tact::verify`]), the encoding key over
/// the chunk table and each chunk's own checksum, no decoding needed. Anything
/// this refuses, a later `lightforge verify` would have called damaged.
fn unverifiable(encoded: &[u8], ekey: &EKey) -> Verified {
    use crate::tact::idx::KEY_SIZE;

    let fault = match encoded.starts_with(b"BLTE") {
        false => Some(crate::tact::verify::Fault::NotBlte),
        true => {
            let key = ekey[..KEY_SIZE].try_into().expect("an encoding key is longer than an index key");
            crate::tact::verify::blte_fault(encoded, key)
        }
    };
    match fault {
        None => Verified::Unverifiable,
        Some(fault) => Verified::Wrong(format!("does not match its encoding key: {}", fault.describe())),
    }
}

/// Whether any chunk of a BLTE payload is encrypted, read from the chunk
/// table rather than from a decode error: `blte` 0.4 either fails untyped
/// without a key service, or misreads the key-name size as a u64 where the
/// format has one byte ("Invalid key name size: 9560733477763426568", low byte
/// `0x08`, the real size). Unreleased content ships encrypted and Agent
/// stores it anyway, so these files are stored unverified; refusing them cost
/// a real Classic Era download 7 files.
///
/// ```text
/// "BLTE"            4 bytes
/// header_size       u32 BE   0 means one chunk, data immediately after
/// flags             u8
/// chunk_count       u24 BE
/// chunk_count x     compressed u32 BE, decompressed u32 BE, md5 16 bytes
/// <data>            each chunk starts with its mode byte
/// ```
pub(super) fn has_encrypted_chunk(blte: &[u8]) -> bool {
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
    Run(&'a crate::tact::plan::Run),
    Loose(&'a crate::tact::plan::Loose),
}

impl Work<'_> {
    /// Bytes this piece of work will pull off the wire.
    fn bytes(&self) -> u64 {
        match self {
            Self::Run(run) => u64::from(run.length),
            Self::Loose(loose) => u64::from(loose.size),
        }
    }
}

enum Fetched<'a> {
    Run { run: &'a crate::tact::plan::Run, body: Vec<u8> },
    Loose { ekey: EKey, ckey: Option<CKey>, body: Vec<u8> },
    Failed { ekeys: Vec<EKey>, reason: String },
    Skipped,
}
