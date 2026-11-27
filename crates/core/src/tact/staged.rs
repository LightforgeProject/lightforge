//! Builds downloaded ahead of their release: the pre-download.
//!
//! Before a patch Blizzard lists the coming build in `/bgdl`. Its data goes
//! into the store while `.build.info` still names the build being played;
//! release day's update then fetches what is left. Recorded beside
//! `.build.info`, not in Lightforge's own state, so whatever reads the store
//! sees it: prune keeps its data ([`super::reclaim::builds`]), and the record
//! moves with the install.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::product::ProductCode;

const FILE: &str = ".lightforge-staged.toml";

#[derive(Debug, thiserror::Error)]
pub enum StagedError {
    #[error("cannot read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("cannot write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
    #[error("{path} is not a staged-build record: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("cannot encode the staged builds: {0}")]
    Encode(#[from] toml::ser::Error),
}

/// One build whose data is in the store ahead of its release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub product: ProductCode,
    /// The build config's hash, what `.build.info`'s `Build Key` will be.
    pub build_key: String,
    /// e.g. `12.2.0.71000`.
    pub version: String,
    /// Where the CDN serves it, for reading its manifests back.
    pub cdn_path: String,
    pub cdn_hosts: Vec<String>,
    pub cdn_servers: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default, rename = "build")]
    builds: Vec<Staged>,
    /// Builds a download has started on and not finished: their data is in
    /// the store, named by nothing else. Prune keeps it, and a cancelled
    /// download removes exactly it, even after Blizzard moved on.
    #[serde(default, rename = "fetching", skip_serializing_if = "Vec::is_empty")]
    fetching: Vec<Staged>,
}

fn path(install: &Path) -> PathBuf {
    install.join(FILE)
}

/// Every staged build in `install`; none when nothing was ever staged.
pub fn load(install: &Path) -> Result<Vec<Staged>, StagedError> {
    read(install).map(|r| r.builds)
}

/// Every build a download started on in `install` and has not finished.
pub fn fetching(install: &Path) -> Result<Vec<Staged>, StagedError> {
    read(install).map(|r| r.fetching)
}

/// Note that a download of `build` has begun. Kept across pauses and runs,
/// so a run that resumes onto a newer build adds to it.
pub fn begin_fetch(install: &Path, build: Staged) -> Result<(), StagedError> {
    let mut record = read(install)?;
    if record.fetching.iter().any(|b| b.product == build.product && b.build_key == build.build_key) {
        return Ok(());
    }
    record.fetching.push(build);
    write(install, record)
}

/// Forget every build a download of `code` was fetching, returning them:
/// it finished, or it was cancelled and its data is going.
pub fn end_fetch(install: &Path, code: &ProductCode) -> Result<Vec<Staged>, StagedError> {
    let mut record = read(install)?;
    let (ended, kept): (Vec<_>, Vec<_>) = record.fetching.into_iter().partition(|b| &b.product == code);
    if ended.is_empty() {
        return Ok(ended);
    }
    record.fetching = kept;
    write(install, record)?;
    Ok(ended)
}

fn read(install: &Path) -> Result<Record, StagedError> {
    let path = path(install);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Record::default()),
        Err(source) => return Err(StagedError::Read { path, source }),
    };
    toml::from_str::<Record>(&text).map_err(|source| StagedError::Parse { path, source })
}

/// `code`'s staged build, for showing it. An unreadable record is logged and
/// shown as none; [`load`] is what decisions about data go through.
pub fn get(install: &Path, code: &ProductCode) -> Option<Staged> {
    match load(install) {
        Ok(builds) => builds.into_iter().find(|b| &b.product == code),
        Err(e) => {
            tracing::warn!("{e}");
            None
        }
    }
}

/// Record `staged`, replacing whatever was staged for its product before.
pub fn record(install: &Path, staged: Staged) -> Result<(), StagedError> {
    let mut record = read(install)?;
    record.builds.retain(|b| b.product != staged.product);
    record.builds.push(staged);
    write(install, record)
}

/// Forget `code`'s staged build, or only the one keyed `build_key` when given.
/// Whether anything was forgotten.
pub fn clear(install: &Path, code: &ProductCode, build_key: Option<&str>) -> Result<bool, StagedError> {
    let mut record = read(install)?;
    let before = record.builds.len();
    record.builds.retain(|b| !(&b.product == code && build_key.is_none_or(|key| b.build_key == key)));
    if record.builds.len() == before {
        return Ok(false);
    }
    write(install, record)?;
    Ok(true)
}

/// Write atomically; nothing staged or fetching removes the file.
fn write(install: &Path, record: Record) -> Result<(), StagedError> {
    let path = path(install);
    let failed = |source| StagedError::Write { path: path.clone(), source };
    if record.builds.is_empty() && record.fetching.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(failed(e)),
            _ => Ok(()),
        };
    }
    let text = toml::to_string_pretty(&record)?;
    crate::fsutil::write_atomic(&path, text.as_bytes()).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(product: ProductCode, build_key: &str) -> Staged {
        Staged {
            product,
            build_key: build_key.to_owned(),
            version: "12.2.0.71000".to_owned(),
            cdn_path: "tpr/wow".to_owned(),
            cdn_hosts: vec!["level3.blizzard.com".to_owned()],
            cdn_servers: vec!["https://level3.ssl.blizzard.com/?maxhosts=8&fallback=1".to_owned()],
        }
    }

    #[test]
    fn a_resumed_download_onto_a_newer_build_remembers_both() {
        let dir = tempfile::tempdir().unwrap();
        begin_fetch(dir.path(), staged(ProductCode::ClassicEra, "aaaa")).unwrap();
        begin_fetch(dir.path(), staged(ProductCode::ClassicEra, "aaaa")).unwrap();
        begin_fetch(dir.path(), staged(ProductCode::ClassicEra, "bbbb")).unwrap();
        let keys: Vec<String> = fetching(dir.path()).unwrap().into_iter().map(|b| b.build_key).collect();
        assert_eq!(keys, ["aaaa", "bbbb"]);
    }

    #[test]
    fn ending_a_fetch_keeps_the_staged_build_and_other_products() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), staged(ProductCode::Retail, "cccc")).unwrap();
        begin_fetch(dir.path(), staged(ProductCode::Retail, "dddd")).unwrap();
        begin_fetch(dir.path(), staged(ProductCode::ClassicEra, "eeee")).unwrap();

        let ended = end_fetch(dir.path(), &ProductCode::Retail).unwrap();

        assert_eq!(ended.len(), 1);
        assert_eq!(load(dir.path()).unwrap().len(), 1);
        assert_eq!(fetching(dir.path()).unwrap()[0].product, ProductCode::ClassicEra);
    }

    #[test]
    fn a_newer_staged_build_replaces_the_older_one() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), staged(ProductCode::Retail, "aaaa")).unwrap();
        record(dir.path(), staged(ProductCode::Classic, "cccc")).unwrap();
        record(dir.path(), staged(ProductCode::Retail, "bbbb")).unwrap();

        let builds = load(dir.path()).unwrap();
        assert_eq!(builds.len(), 2);
        assert_eq!(get(dir.path(), &ProductCode::Retail).unwrap().build_key, "bbbb");
    }

    #[test]
    fn clearing_another_build_key_keeps_the_staged_one() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), staged(ProductCode::Retail, "bbbb")).unwrap();
        assert!(!clear(dir.path(), &ProductCode::Retail, Some("aaaa")).unwrap());
        assert!(clear(dir.path(), &ProductCode::Retail, Some("bbbb")).unwrap());
        assert!(!dir.path().join(FILE).exists(), "nothing staged leaves no file");
    }
}
