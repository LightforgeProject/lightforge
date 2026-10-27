//! Pre-flight: everything that has to be true before Battle.net is spawned.
//!
//! The problem this solves is the "Initializing Data…" deadlock. A previous
//! session leaves `Agent.exe`, or the whole client, alive inside the prefix;
//! the next launch finds Agent's directory cache describing a world that no
//! longer exists, and Agent responds by scanning 126 GB of CASC storage.
//!
//! The pass, in order:
//!
//! 1. [`scan`] the process table for anything belonging to *this* prefix,
//! 2. `wineserver -k` for a graceful, prefix-scoped shutdown,
//! 3. re-scan and signal the stragglers, `SIGTERM` then `SIGKILL`,
//! 4. with nothing alive: delete Agent's directory *cache* and the installer's
//!    staging directory, never Agent's own directory, which holds the
//!    `product.db` registry of installed games,
//! 5. re-apply the Battle.net config defaults,
//! 6. pause any download writing to CASC storage.
//!
//! A process is only ever signalled when its own environment names
//! Lightforge's prefix (`WINEPREFIX` or `STEAM_COMPAT_DATA_PATH`). Someone
//! running Battle.net out of Lutris or Steam has processes with identical
//! names and a different prefix, so killing on name alone would be a
//! data-loss bug. A process whose environment cannot be read is left alone.
//!
//! Steps 4 and 5 are gated on step 3 succeeding: the client rewrites its
//! configs on exit and Agent memory-maps its cache, so writing into a live
//! prefix loses the write at best.
//!
//! The same process table answers the question at the *other* end of a
//! launch; see [`wait_for_game_start`]. Handing a game to Battle.net gives
//! nothing to wait on, because the client exits as soon as the game is up.

mod games;
mod gate;
mod scan;
mod shutdown;
#[cfg(test)]
mod tests;

pub use games::{
    client_running, games_running, stop_games, wait_for_game_exit, wait_for_game_start,
    GameProcess, Stopped,
};
pub use gate::{DownloadGuard, NoDownloads, Preflight, Report, Timing};
pub use scan::{scan, PrefixProcess, ProcKind};
pub use shutdown::{shutdown, Disposition, Outcome};

use std::path::PathBuf;
use std::time::Duration;

use crate::battlenet::BattleNetError;
use crate::prefix::PrefixError;
use crate::product::ProductCode;
// The tests reach the classifiers and the signalling directly.
#[cfg(test)]
use {games::*, scan::*, shutdown::*};

/// How long a process gets to exit on `SIGTERM` before `SIGKILL` follows.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);

/// Polling interval while waiting for signalled processes to disappear.
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error(
        "{} in the prefix survived SIGKILL: {}",
        if .0.len() == 1 { "a process" } else { "processes" },
        .0.iter().map(|p| format!("{} (pid {})", p.name, p.pid)).collect::<Vec<_>>().join(", ")
    )]
    StillRunning(Vec<PrefixProcess>),

    #[error(
        "a game is running in the prefix ({}); close it first, cleaning the prefix would end it",
        .0.iter().map(|g| format!("{} (pid {})", g.name, g.pid)).collect::<Vec<_>>().join(", ")
    )]
    GameRunning(Vec<GameProcess>),

    #[error("a download for `{0}` is writing to CASC storage and could not be paused")]
    DownloadBusy(ProductCode),

    #[error(transparent)]
    Prefix(#[from] PrefixError),

    #[error(transparent)]
    BattleNet(#[from] BattleNetError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
