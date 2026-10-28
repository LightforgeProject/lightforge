//! Downloads in the queue, and how fast they are going.

use crate::*;

/// The parts of a finished run the UI actually shows. `install::Summary` is not
/// `Clone`, and a message has to be.
#[derive(Debug, Clone)]
pub(crate) struct Complete {
    pub(crate) complete: bool,
    pub(crate) cancelled: bool,
    pub(crate) files: usize,
    pub(crate) version: Option<String>,
    /// Why a finished run is not complete, see `Summary::reasons`.
    pub(crate) reasons: Vec<String>,
}

/// Where one product's transfer has got to.
///
/// Keyed to a product: held app-wide, every product would show whichever
/// transfer is running.
pub(crate) enum Stage {
    /// Accepted, waiting for the running one to finish.
    Queued,
    Running {
        rx: tokio::sync::watch::Receiver<TransferState>,
        cancel: Cancel,
    },
    /// Stopped with work outstanding. Keeps the progress it reached so the bar
    /// does not snap back to zero on resume.
    Paused(state::Progress),
}

/// One product's place in the download queue.
pub(crate) struct Download {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    pub(crate) stage: Stage,
    /// Newest sample off the channel, kept after a pause so the manager can
    /// still say how far it got.
    pub(crate) sample: TransferState,
    pub(crate) rate: Rate,
    pub(crate) job: Job,
    /// Into the store ahead of a release, while the installed build plays on.
    pub(crate) pre_download: bool,
}

/// What one transfer set out to fetch, carried across its pauses.
///
/// The bar measures the job, not the product: an update that changes 10 GB of
/// a 90 GB install reads "0 of 10 GB", not "80 of 90 GB". A resumed run plans
/// only what is still missing, so the job's size is fixed by the *first* plan
/// and each later run is placed within it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Job {
    /// Bytes the first run planned. `None` until one has planned.
    pub(crate) total: Option<u64>,
    /// Bytes done when the current run started, shown while a resumed run
    /// is still planning, so the bar holds instead of dropping to zero.
    pub(crate) reached: u64,
}

impl Download {
    /// Progress against the whole product, not against this run.
    ///
    /// A resumed run plans only what is missing, so run-relative numbers read
    /// as "starting over" every time a download is paused and picked up again.
    pub(crate) fn progress(&self) -> state::Progress {
        job_progress(self.job, &self.sample, self.rate.per_second)
    }

    /// This entry as a resumed job: the size its first run planned, and how
    /// far it got before the pause.
    pub(crate) fn resumed(self) -> Job {
        let reached = match self.stage {
            Stage::Paused(p) => p.downloaded_bytes,
            _ => 0,
        };
        Job { reached, ..self.job }
    }

    pub(crate) fn transfer(&self) -> state::Transfer {
        match &self.stage {
            Stage::Queued => state::Transfer::Queued,
            Stage::Running { .. } => state::Transfer::Running(self.progress()),
            Stage::Paused(progress) => state::Transfer::Paused(*progress),
        }
    }
}

/// Transfer rate as a human should read it: an average with a real time
/// constant, published at a pace the eye can follow.
///
/// A raw delta between two samples swings wildly when a large file lands in one
/// tick, so the rate is averaged. Measuring often keeps the ETA honest, but a
/// number that changes three times a second reads as flicker however well it is
/// averaged, so what the view shows is held between refreshes.
#[derive(Debug, Default)]
pub(crate) struct Rate {
    pub(crate) last_bytes: u64,
    pub(crate) last_at: Option<Instant>,
    /// The average, moved by every sample.
    pub(crate) average: f64,
    /// What the view shows: the average as it stood at `shown_at`.
    pub(crate) per_second: u64,
    pub(crate) shown_at: Option<Instant>,
}

impl Rate {
    /// How long the average takes to cover about two thirds of a step change.
    pub(crate) const TAU: f64 = 2.0;
    /// How long a shown value stands before the average replaces it.
    pub(crate) const HOLD: Duration = Duration::from_secs(1);
    /// Shorter windows are mostly chunk-arrival noise, not a rate.
    pub(crate) const MIN_WINDOW: f64 = 0.25;

    #[allow(clippy::cast_possible_truncation, reason = "a rate in bytes per second")]
    pub(crate) fn sample(&mut self, bytes: u64) {
        let now = Instant::now();
        let Some(previous) = self.last_at else {
            self.last_at = Some(now);
            self.last_bytes = bytes;
            return;
        };
        let elapsed = now.duration_since(previous).as_secs_f64();
        if elapsed < Self::MIN_WINDOW {
            return;
        }

        let instant = bytes.saturating_sub(self.last_bytes) as f64 / elapsed;
        // Weighting by the time that passed, not by the tick, keeps the
        // smoothing the same whether ticks arrive on time, late or in a burst.
        self.average = match self.shown_at {
            None => instant,
            Some(_) => {
                let alpha = 1.0 - (-elapsed / Self::TAU).exp();
                self.average + (instant - self.average) * alpha
            }
        };
        self.last_bytes = bytes;
        self.last_at = Some(now);

        // The first real sample is shown at once -- an empty readout while the
        // average warms up would look stalled.
        if self.shown_at.is_none_or(|at| now.duration_since(at) >= Self::HOLD) {
            self.per_second = self.average as u64;
            self.shown_at = Some(now);
        }
    }
}
