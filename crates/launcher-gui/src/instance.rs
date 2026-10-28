//! One GUI per configuration.
//!
//! An `flock` on `lightforge-<id>.lock` in `$XDG_RUNTIME_DIR`, held for the
//! life of the process. `<id>` comes from the config directory, so a run with
//! its own `XDG_CONFIG_HOME` (a sandbox) is an instance of its own. A second
//! launch connects to the socket beside the lock, which brings the first
//! window forward, and exits. The CLI never takes it: the store lock already
//! keeps the CLI and the GUI from writing at once.
//!
//! The update restart starts the new binary before the old one has exited,
//! so a launch with [`AFTER_UPDATE`] waits for the lock instead.

use std::fs::{File, TryLockError};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::Stream;

pub const AFTER_UPDATE: &str = "--after-update";

/// How long a restarted instance waits for the old one to let go. Past it,
/// the restart pokes the old one and exits; if that one was on its way out,
/// no window is left.
const RESTART_WAIT: Duration = Duration::from_secs(15);

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
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| dirs.cache_dir().to_path_buf(), PathBuf::from);
    let id = lightforge_core::text::hex(&md5::compute(dirs.config_dir().as_os_str().as_bytes()).0[..4]);
    let wait = if after_update { RESTART_WAIT } else { Duration::ZERO };

    match claim_in(&dir, &id, wait) {
        Ok(Some(instance)) => {
            let _ = INSTANCE.set(instance);
            true
        }
        Ok(None) => {
            tracing::info!("Lightforge is already running; asked it to come forward");
            false
        }
        Err(e) => {
            tracing::warn!("no single-instance lock in {}: {e}", dir.display());
            true
        }
    }
}

/// `None`: another instance holds the lock, and has been poked.
fn claim_in(dir: &Path, id: &str, wait: Duration) -> io::Result<Option<Instance>> {
    std::fs::create_dir_all(dir)?;
    let lock = File::create(dir.join(format!("lightforge-{id}.lock")))?;
    let socket = dir.join(format!("lightforge-{id}.sock"));

    let deadline = Instant::now() + wait;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryLockError::WouldBlock) => {
                if !wait.is_zero() {
                    tracing::warn!("the previous Lightforge still held the lock after {}s", wait.as_secs());
                }
                if let Err(e) = UnixStream::connect(&socket) {
                    tracing::warn!("the running Lightforge did not answer at {}: {e}", socket.display());
                }
                return Ok(None);
            }
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }

    // Left by an instance that did not exit cleanly; the lock says it is gone.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
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

    #[test]
    fn a_second_claim_pokes_the_first_instead_of_starting() {
        let dir = tempfile::tempdir().unwrap();
        let first = claim_in(dir.path(), "test", Duration::ZERO).unwrap().expect("the lock is free");
        assert!(claim_in(dir.path(), "test", Duration::ZERO).unwrap().is_none());

        first.listener.as_ref().unwrap().set_nonblocking(true).unwrap();
        assert!(first.listener.as_ref().unwrap().accept().is_ok(), "the second launch connected");
    }

    #[test]
    fn a_restart_takes_the_lock_once_the_old_instance_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let old = claim_in(dir.path(), "test", Duration::ZERO).unwrap().expect("the lock is free");
        let exiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(old);
        });
        assert!(claim_in(dir.path(), "test", RESTART_WAIT).unwrap().is_some());
        exiting.join().unwrap();
    }

    #[test]
    fn another_configuration_is_an_instance_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let _real = claim_in(dir.path(), "real", Duration::ZERO).unwrap().expect("the lock is free");
        assert!(claim_in(dir.path(), "sandbox", Duration::ZERO).unwrap().is_some());
    }
}
