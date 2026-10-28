//! The download queue as it stood, kept across restarts.
//!
//! Only the queue's shape is kept, what is in it, in what order, and how far
//! each job had got. The bytes themselves are the store's business: it is
//! checkpointed as it goes, and a restored transfer re-plans against it like
//! any resume.
//!
//! Beside the config rather than in it: this is state the app writes on every
//! pause and resume, not settings anyone edits.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::product::ProductCode;

#[derive(Debug, thiserror::Error)]
pub enum TransfersError {
    #[error("cannot read {path}: {source}")]
    Read { path: std::path::PathBuf, source: std::io::Error },
    #[error("cannot write {path}: {source}")]
    Write { path: std::path::PathBuf, source: std::io::Error },
    #[error("{path} is not a saved queue: {source}")]
    Parse { path: std::path::PathBuf, source: toml::de::Error },
    #[error("cannot encode the queue: {0}")]
    Encode(#[from] toml::ser::Error),
}

/// Where a transfer stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Standing {
    /// Downloading when the app closed; resumed on the next start.
    Running,
    Queued,
    Paused,
}

/// One entry in the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub code: ProductCode,
    pub standing: Standing,
    /// Bytes the job's first plan asked for, if it had planned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_bytes: Option<u64>,
    /// Bytes done when it was paused. Not tracked while running, the store
    /// knows that better, and the next plan reads it back.
    #[serde(default)]
    pub reached: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default, rename = "transfer")]
    pub entries: Vec<Saved>,
}

impl Queue {
    /// The saved queue, or an empty one when none was saved.
    pub fn load(path: &Path) -> Result<Self, TransfersError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(TransfersError::Read { path: path.to_path_buf(), source }),
        };
        toml::from_str(&text)
            .map_err(|source| TransfersError::Parse { path: path.to_path_buf(), source })
    }

    /// Write via a temporary file, flushed before the rename, so a crash
    /// mid-save leaves the last good queue rather than half of one or an
    /// empty file. An empty queue removes the file.
    pub fn save(&self, path: &Path) -> Result<(), TransfersError> {
        let write = |source| TransfersError::Write { path: path.to_path_buf(), source };
        if self.entries.is_empty() {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(write(e)),
                _ => Ok(()),
            };
        }
        let text = toml::to_string_pretty(self)?;
        crate::fsutil::write_atomic(path, text.as_bytes()).map_err(write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_round_trips_and_an_empty_one_leaves_no_file() {
        let dir = std::env::temp_dir()
            .join(format!("lightforge-transfers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("queue.toml");

        assert_eq!(Queue::load(&path).unwrap(), Queue::default());

        let queue = Queue {
            entries: vec![
                Saved {
                    code: ProductCode::Classic,
                    standing: Standing::Running,
                    job_bytes: Some(12 << 30),
                    reached: 0,
                },
                Saved {
                    code: ProductCode::ClassicEra,
                    standing: Standing::Paused,
                    job_bytes: Some(4 << 30),
                    reached: 1 << 30,
                },
            ],
        };
        queue.save(&path).unwrap();
        assert_eq!(Queue::load(&path).unwrap(), queue);

        Queue::default().save(&path).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_saves_all_succeed_and_leave_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.toml");
        let queue = Queue {
            entries: vec![Saved {
                code: ProductCode::Classic,
                standing: Standing::Queued,
                job_bytes: None,
                reached: 0,
            }],
        };

        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..25 {
                        queue.save(&path).unwrap();
                    }
                });
            }
        });

        assert_eq!(Queue::load(&path).unwrap(), queue);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
