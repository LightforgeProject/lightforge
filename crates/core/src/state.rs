//! The primary action button's state machine.
//!
//! The UI renders exactly one of these per product. Transitions are computed
//! from local `.build.info` against Ribbit's reported CDN build (Milestone 5);
//! Milestone 1 defines the shape and the invariants.

use serde::{Deserialize, Serialize};

/// What the primary button currently offers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ActionState {
    /// Local state hasn't been determined yet (startup, or a refresh in flight).
    #[default]
    Unknown,
    /// Game is missing — `[ INSTALL ]`, light blue.
    Install,
    /// Ribbit reports newer CDN hashes than local `.build.info` — `[ UPDATE ]`, orange.
    Update { local_build: String, remote_build: String },
    /// A download or patch is running. Carries live progress for the speed/ETA UI.
    Downloading(Progress),
    /// Download is paused, either by the user or by pre-flight before launching
    /// Battle.net (concurrent CASC writes are a real corruption risk).
    Paused(Progress),
    /// Ready to launch — `[ PLAY ]`, green.
    Play,
    /// Something went wrong; the message is user-facing.
    Error { message: String },
}

impl ActionState {
    /// Button label for this state.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unknown => "…",
            Self::Install => "INSTALL",
            Self::Update { .. } => "UPDATE",
            Self::Downloading(_) => "PAUSE",
            Self::Paused(_) => "RESUME",
            Self::Play => "PLAY",
            Self::Error { .. } => "RETRY",
        }
    }

    /// Whether launching the game is possible right now.
    pub fn is_playable(&self) -> bool {
        matches!(self, Self::Play)
    }

    /// Whether a download is occupying CASC storage. Pre-flight must pause the
    /// downloader before spawning Battle.net when this is true.
    pub fn is_writing_casc(&self) -> bool {
        matches!(self, Self::Downloading(_))
    }
}

/// Live transfer progress, used for the MB/s and ETA readouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// Instantaneous rate, bytes per second.
    pub bytes_per_second: u64,
}

impl Progress {
    /// Completion in `0.0..=1.0`. Returns 0.0 while the total is still unknown
    /// rather than dividing by zero.
    pub fn fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.downloaded_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
    }

    pub fn remaining_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.downloaded_bytes)
    }

    /// Seconds left at the current rate, or `None` while stalled or unknown.
    pub fn eta(&self) -> Option<std::time::Duration> {
        if self.bytes_per_second == 0 || self.total_bytes == 0 {
            return None;
        }
        Some(std::time::Duration::from_secs(self.remaining_bytes() / self.bytes_per_second))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_play_is_playable() {
        assert!(ActionState::Play.is_playable());
        assert!(!ActionState::Install.is_playable());
        assert!(!ActionState::Downloading(Progress::default()).is_playable());
    }

    #[test]
    fn paused_downloads_do_not_block_battle_net() {
        assert!(ActionState::Downloading(Progress::default()).is_writing_casc());
        assert!(!ActionState::Paused(Progress::default()).is_writing_casc());
    }

    #[test]
    fn progress_handles_unknown_total() {
        let p = Progress { downloaded_bytes: 10, total_bytes: 0, bytes_per_second: 5 };
        assert_eq!(p.fraction(), 0.0);
        assert!(p.eta().is_none());
    }

    #[test]
    fn eta_uses_remaining_bytes() {
        let p = Progress { downloaded_bytes: 50, total_bytes: 150, bytes_per_second: 10 };
        assert_eq!(p.eta().unwrap().as_secs(), 10);
        assert!((p.fraction() - 1.0 / 3.0).abs() < f32::EPSILON);
    }
}
