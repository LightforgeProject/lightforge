//! The primary action button's state machine.
//!
//! Three rules:
//!
//! 1. **`.build.info` is the completion marker**, written last and only on a
//!    complete run. During an update it still names the previous build, the
//!    one safe to launch.
//! 2. **An unreachable version service does not block launching**: a
//!    complete install is `Play`.
//! 3. **A live transfer outranks everything.**

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths::Layout;
use crate::product::ProductCode;
use crate::tact::version::Version;

/// What the primary button currently offers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ActionState {
    /// Local state hasn't been determined yet (startup, or a refresh in flight).
    #[default]
    Unknown,
    /// Game is missing, `[ INSTALL ]`, light blue.
    Install,
    /// Ribbit reports newer CDN hashes than local `.build.info`: `[ UPDATE ]`, violet.
    Update {
        local_build: String,
        remote_build: String,
    },
    /// Waiting for another product's download: one transfer at a time.
    Queued,
    /// A download or patch is running. Carries live progress for the speed/ETA UI.
    Downloading(Progress),
    /// Download is paused, either by the user or by pre-flight before launching
    /// Battle.net (concurrent CASC writes are a real corruption risk).
    Paused(Progress),
    /// Ready to launch, `[ PLAY ]`, the wordmark's gold.
    Play,
    /// Something went wrong; the message is user-facing.
    Error { message: String },
    /// The drive this product lives on is not mounted. Everything that writes
    /// waits; decided outside [`decide`].
    Offline { drive: String },
}

impl ActionState {
    /// Button label for this state.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unknown => "…",
            Self::Install => "INSTALL",
            Self::Update { .. } => "UPDATE",
            Self::Queued => "QUEUED",
            Self::Downloading(_) => "PAUSE",
            Self::Paused(_) => "RESUME",
            Self::Play => "PLAY",
            Self::Error { .. } => "RETRY",
            Self::Offline { .. } => "RETRY",
        }
    }

    /// Whether launching the game is possible right now.
    pub fn is_playable(&self) -> bool {
        matches!(self, Self::Play)
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
    /// Completion in `0.0..=1.0`, 0.0 while the total is unknown.
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
        Some(std::time::Duration::from_secs(
            self.remaining_bytes() / self.bytes_per_second,
        ))
    }
}

/// What one product looks like on this machine, read without any network.
///
/// Cheap: the store is only counted, by index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Local {
    /// `Build Key` from `.build.info`, absent if the file or row is missing.
    pub build_key: Option<String>,
    /// `Version`, e.g. `1.15.9.69722`.
    pub version: Option<String>,
    /// The game binary, if one is actually there.
    pub executable: Option<PathBuf>,
    /// Files the CASC store holds. Zero means no readable store.
    pub store_files: usize,
}

impl Local {
    /// Inspect `code`'s install under `layout`.
    pub fn inspect(layout: &Layout, code: &ProductCode) -> Self {
        let info = crate::buildinfo::BuildInfo::load(layout.build_info(code)).ok();
        let cell = |column: &str| {
            info.as_ref()
                .and_then(|i| i.get(code, column).ok())
                .map(str::to_owned)
                .filter(|v| !v.is_empty())
        };

        let exe = layout.wow_exe(code);
        let store_files = crate::tact::storage::Storage::open(layout.wow_install(code))
            .map(|store| store.stats().files)
            .unwrap_or(0);

        Self {
            build_key: cell(crate::buildinfo::COL_BUILD_KEY),
            version: cell(crate::buildinfo::COL_VERSION),
            executable: exe.is_file().then_some(exe),
            store_files,
        }
    }

    /// Whether this is a finished install, not a partial one.
    ///
    /// A build key, the binary and a non-empty store: any one alone is what
    /// an interrupted run leaves.
    pub fn is_installed(&self) -> bool {
        self.build_key.is_some() && self.executable.is_some() && self.store_files > 0
    }
}

/// A transfer this process is running for the product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// Accepted, but another product is downloading first.
    Queued,
    Running(Progress),
    Paused(Progress),
}

/// Decide which button a product shows.
///
/// Pure: [`Local::inspect`] supplies the disk half; `remote` is `None` when
/// the version service could not be reached.
pub fn decide(local: &Local, remote: Option<&Version>, transfer: Option<Transfer>) -> ActionState {
    // What is happening now beats what the disk said a moment ago.
    match transfer {
        Some(Transfer::Queued) => return ActionState::Queued,
        Some(Transfer::Running(progress)) => return ActionState::Downloading(progress),
        Some(Transfer::Paused(progress)) => return ActionState::Paused(progress),
        None => {}
    }

    if !local.is_installed() {
        return ActionState::Install;
    }

    let Some(remote) = remote else {
        // Offline, or the service is down. The install is complete, so it runs.
        return ActionState::Play;
    };

    match crate::tact::version::Status::compare(
        local.build_key.as_deref(),
        local.version.as_deref(),
        remote,
    ) {
        crate::tact::version::Status::Behind { local_build, remote_build, .. } => {
            ActionState::Update { local_build, remote_build }
        }
        // `is_installed` required a build key, so not `NotInstalled`.
        _ => ActionState::Play,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(build: &str) -> Version {
        Version {
            region: "eu".into(),
            build_config: build.into(),
            cdn_config: "cdn".into(),
            key_ring: None,
            build_id: 69722,
            versions_name: "1.15.9.69722".into(),
            product_config: String::new(),
        }
    }

    fn complete() -> Local {
        Local {
            build_key: Some("3645f0fe9dc5215ea90eaf7cdb7379ce".into()),
            version: Some("1.15.9.69722".into()),
            executable: Some("/games/_classic_era_/WowClassic.exe".into()),
            store_files: 146_669,
        }
    }

    #[test]
    fn a_complete_install_on_the_current_build_plays() {
        let state = decide(&complete(), Some(&remote("3645f0fe9dc5215ea90eaf7cdb7379ce")), None);
        assert_eq!(state, ActionState::Play);
    }

    #[test]
    fn a_different_remote_build_offers_an_update_either_direction() {
        let state = decide(&complete(), Some(&remote("ffffffffffffffffffffffffffffffff")), None);
        match state {
            ActionState::Update { local_build, remote_build } => {
                assert_eq!(local_build, "3645f0fe9dc5215ea90eaf7cdb7379ce");
                assert_eq!(remote_build, "ffffffffffffffffffffffffffffffff");
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn an_unreachable_version_service_still_lets_a_finished_install_play() {
        assert_eq!(decide(&complete(), None, None), ActionState::Play);
    }

    #[test]
    fn every_partial_shape_reads_as_install_not_play() {
        // Shapes an interrupted run leaves; none may offer Play.
        let no_marker = Local { build_key: None, ..complete() };
        let no_binary = Local { executable: None, ..complete() };
        let no_content = Local { store_files: 0, ..complete() };

        for local in [no_marker, no_binary, no_content, Local::default()] {
            assert!(!local.is_installed(), "{local:?}");
            assert_eq!(decide(&local, Some(&remote("abc")), None), ActionState::Install);
            // Offline does not turn a partial install into a playable one.
            assert_eq!(decide(&local, None, None), ActionState::Install);
        }
    }

    #[test]
    fn a_live_transfer_outranks_whatever_the_disk_says() {
        let progress = Progress { downloaded_bytes: 50, total_bytes: 100, bytes_per_second: 10 };

        // Even a complete, current install shows the transfer that is running.
        let current = Some(remote("3645f0fe9dc5215ea90eaf7cdb7379ce"));
        assert_eq!(
            decide(&complete(), current.as_ref(), Some(Transfer::Running(progress))),
            ActionState::Downloading(progress)
        );
        assert_eq!(
            decide(&Local::default(), None, Some(Transfer::Paused(progress))),
            ActionState::Paused(progress)
        );
    }

    #[test]
    fn a_transfer_in_progress_is_never_playable() {
        let progress = Progress::default();
        let running = decide(&complete(), None, Some(Transfer::Running(progress)));
        let paused = decide(&complete(), None, Some(Transfer::Paused(progress)));

        assert!(!running.is_playable() && !paused.is_playable());
    }

    #[test]
    fn only_play_is_playable() {
        assert!(ActionState::Play.is_playable());
        assert!(!ActionState::Install.is_playable());
        assert!(!ActionState::Downloading(Progress::default()).is_playable());
    }

    #[test]
    fn progress_handles_unknown_total() {
        let p = Progress {
            downloaded_bytes: 10,
            total_bytes: 0,
            bytes_per_second: 5,
        };
        assert_eq!(p.fraction(), 0.0);
        assert!(p.eta().is_none());
    }

    #[test]
    fn eta_uses_remaining_bytes() {
        let p = Progress {
            downloaded_bytes: 50,
            total_bytes: 150,
            bytes_per_second: 10,
        };
        assert_eq!(p.eta().unwrap().as_secs(), 10);
        assert!((p.fraction() - 1.0 / 3.0).abs() < f32::EPSILON);
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn installed() -> Local {
        Local {
            build_key: Some("abc".into()),
            version: Some("1.0.0".into()),
            executable: Some(PathBuf::from("/games/_retail_/Wow.exe")),
            store_files: 1,
        }
    }

    /// The bug this whole model exists to prevent: one product's transfer
    /// deciding another product's button.
    #[test]
    fn a_transfer_only_speaks_for_the_product_it_belongs_to() {
        let running = Some(Transfer::Running(Progress::default()));
        assert!(matches!(
            decide(&installed(), None, running),
            ActionState::Downloading(_)
        ));
        // The same disk state with no transfer of its own is simply ready.
        assert_eq!(decide(&installed(), None, None), ActionState::Play);
    }

    #[test]
    fn a_queued_product_says_so_rather_than_looking_ready() {
        // Not installed and waiting its turn: neither Install nor Downloading
        // is honest, hence Queued.
        assert_eq!(
            decide(&Local::default(), None, Some(Transfer::Queued)),
            ActionState::Queued
        );
        assert_eq!(ActionState::Queued.label(), "QUEUED");
    }
}
