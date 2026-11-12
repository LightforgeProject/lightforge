//! One GUI per configuration.
//!
//! An `flock` on [`AppDirs::instance_file`]`("lock")`, held for the life of
//! the process. A second launch connects to the socket beside the lock, which
//! brings the first window forward, and exits. The CLI only looks at it, to
//! know whether a GUI is there to look after a background Battle.net; the
//! store lock already keeps the CLI and the GUI from writing at once.
//!
//! [`AppDirs::instance_file`]: lightforge_core::paths::AppDirs::instance_file
//!
//! The update restart starts the new binary before the old one has exited,
//! so a launch with [`AFTER_UPDATE`] waits for the lock instead.

use std::fs::{File, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::Stream;

pub const AFTER_UPDATE: &str = "--after-update";

/// How long a restarted instance waits for the old one to let go. Past it,
/// the restart pokes the old one and exits; if that one was on its way out,
/// no window is left.
const RESTART_WAIT: Duration = Duration::from_secs(15);

/// How long any launch waits for the lock: the CLI takes it for a moment to
/// see whether a GUI runs ([`AppDirs::gui_running`]).
///
/// [`AppDirs::gui_running`]: lightforge_core::paths::AppDirs::gui_running
const PROBE_WAIT: Duration = Duration::from_millis(300);

/// Kept until the process exits, which is what releases the lock.
static INSTANCE: OnceLock<Instance> = OnceLock::new();

struct Instance {
    _lock: File,
    /// `None` when the socket could not be made: no raising, still one instance.
    listener: Option<UnixListener>,
}

/// Take this configuration's lock, or bring forward the instance holding it.
/// `false` means another instance has it, and this one should exit.
///
/// Anything in the way of taking it (no runtime directory, no config
/// directory) is logged and the GUI runs unguarded.
pub fn claim(after_update: bool) -> bool {
    let dirs = match lightforge_core::paths::AppDirs::discover() {
        Ok(dirs) => dirs,
        Err(e) => {
            tracing::warn!("no single-instance lock: {e}");
            return true;
        }
    };
    let (lock, socket) = (dirs.instance_file("lock"), dirs.instance_file("sock"));
    let wait = if after_update { RESTART_WAIT } else { PROBE_WAIT };

    match claim_in(&lock, &socket, wait) {
        Ok(Some(instance)) => {
            let _ = INSTANCE.set(instance);
            true
        }
        Ok(None) => {
            if after_update {
                tracing::warn!("the previous Lightforge still held the lock after {}s", wait.as_secs());
            }
            tracing::info!("Lightforge is already running; asked it to come forward");
            false
        }
        Err(e) => {
            tracing::warn!("no single-instance lock at {}: {e}", lock.display());
            true
        }
    }
}

/// `None`: another instance holds the lock, and has been poked.
fn claim_in(lock: &Path, socket: &Path, wait: Duration) -> io::Result<Option<Instance>> {
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Without `XDG_RUNTIME_DIR` this is in the cache folder, where another
    // user's planted symlink must not make us truncate or lock its target.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock)?;

    let deadline = Instant::now() + wait;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryLockError::WouldBlock) => {
                if let Err(e) = UnixStream::connect(socket) {
                    tracing::warn!("the running Lightforge did not answer at {}: {e}", socket.display());
                }
                return Ok(None);
            }
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }

    // Left by an instance that did not exit cleanly; the lock says it is gone.
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)
        .inspect_err(|e| tracing::warn!("cannot listen at {}: {e}", socket.display()))
        .ok();
    Ok(Some(Instance { _lock: lock, listener }))
}

/// One item each time a second launch asks for the window.
pub fn raises() -> impl Stream<Item = ()> {
    iced::stream::channel(1, async |mut output| {
        let Some(listener) = INSTANCE.get().and_then(|i| i.listener.as_ref()) else {
            return;
        };
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        // `accept` blocks, so it gets a thread of its own, as the pads do.
        let spawned = std::thread::Builder::new().name("instance".to_owned()).spawn(move || {
            for stream in listener.incoming() {
                if stream.is_ok() && send.send(()).is_err() {
                    return;
                }
            }
        });
        if spawned.is_err() {
            return;
        }
        while receive.recv().await.is_some() {
            use futures_util::SinkExt;
            if output.send(()).await.is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        (dir.path().join("lightforge-test.lock"), dir.path().join("lightforge-test.sock"))
    }

    #[test]
    fn a_second_claim_pokes_the_first_instead_of_starting() {
        let dir = tempfile::tempdir().unwrap();
        let (lock, socket) = paths(&dir);
        let first = claim_in(&lock, &socket, Duration::ZERO).unwrap().expect("the lock is free");
        assert!(claim_in(&lock, &socket, Duration::ZERO).unwrap().is_none());

        first.listener.as_ref().unwrap().set_nonblocking(true).unwrap();
        assert!(first.listener.as_ref().unwrap().accept().is_ok(), "the second launch connected");
    }

    #[test]
    fn a_restart_takes_the_lock_once_the_old_instance_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let (lock, socket) = paths(&dir);
        let old = claim_in(&lock, &socket, Duration::ZERO).unwrap().expect("the lock is free");
        let exiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(old);
        });
        assert!(claim_in(&lock, &socket, RESTART_WAIT).unwrap().is_some());
        exiting.join().unwrap();
    }

    #[test]
    fn a_symlink_planted_at_the_lock_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let (lock, socket) = paths(&dir);
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, &lock).unwrap();

        assert!(claim_in(&lock, &socket, PROBE_WAIT).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    }

    #[test]
    fn a_launch_while_the_cli_looks_at_the_lock_still_starts() {
        let dir = tempfile::tempdir().unwrap();
        let (lock, socket) = paths(&dir);
        let probe = File::create(&lock).unwrap();
        probe.try_lock_shared().unwrap();
        let looking = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(probe);
        });
        assert!(claim_in(&lock, &socket, PROBE_WAIT).unwrap().is_some());
        looking.join().unwrap();
    }
}
