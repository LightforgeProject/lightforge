use std::pin::pin;

use futures_util::future::{self, Either};
use futures_util::stream::{self, StreamExt};

use super::{cancelled, Cancel, DownloadError, Failure, Options, Outcome, Progress, Source};
use crate::tact::blte::BlteError;
use crate::tact::config::CKey;
use crate::text::hex;
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

    /// Fetch everything in `plan` and write it through `writer`, which is left
    /// open for the caller to finish.
    pub async fn run(
        &self,
        source: Source<'_>,
        plan: &Plan,
        writer: &mut StoreWriter,
        cancel: &Cancel,
        mut progress: impl FnMut(Progress),
    ) -> Result<Outcome, DownloadError> {
        let Source { http, client } = source;
        let mut sink = Sink {
            writer,
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

        // Runs first: they are the bulk.
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
                // Paid before the request goes out.
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
                        match client.fetch(http, crate::tact::cdn::Kind::Data, &hex(&loose.ekey)).await {
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

        // Raced against the pause: waiting out `concurrency` requests of
        // hundreds of megabytes read as a hang. Dropped bytes were never stored.
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

        // Finished, failed or cancelled: leave the store readable.
        sink.writer.checkpoint()?;

        Ok(outcome)
    }
}

/// The write side: one store, and the tally of what went into it.
struct Sink<'a> {
    writer: &'a mut StoreWriter,
    outcome: Outcome,
    state: Progress,
    since_checkpoint: u64,
}

impl Sink<'_> {
    /// Verify one file and write it, counting what happened.
    fn store(&mut self, ekey: &EKey, ckey: Option<&CKey>, encoded: &[u8]) -> Result<(), DownloadError> {
        match verify(encoded, ekey, ckey) {
            Verified::Ok => {}
            Verified::Unverifiable => self.outcome.unverified += 1,
            Verified::Wrong(reason) => {
                // A file that failed its content key is never stored.
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
    /// Encrypted, or absent from encoding: store it anyway.
    Unverifiable,
    Wrong(String),
}

/// Decode and check against the content key.
pub(super) fn verify(encoded: &[u8], ekey: &EKey, ckey: Option<&CKey>) -> Verified {
    let Some(ckey) = ckey else {
        return unverifiable(encoded, ekey);
    };

    match crate::tact::blte::decode(encoded) {
        Ok(decoded) => {
            let actual = *md5::compute(&decoded);
            if &actual == ckey {
                Verified::Ok
            } else {
                Verified::Wrong(format!(
                    "decoded to {} but the build says {}",
                    hex(&actual),
                    hex(ckey)
                ))
            }
        }
        Err(BlteError::Encrypted { .. }) => unverifiable(encoded, ekey),
        Err(e) => Verified::Wrong(e.to_string()),
    }
}

/// A file whose content key cannot be checked, held to what a store scan
/// ([`crate::tact::verify`]) checks: the encoding key over the chunk table and
/// each chunk's checksum.
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
