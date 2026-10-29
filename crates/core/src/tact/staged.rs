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
}

fn path(install: &Path) -> PathBuf {
    install.join(FILE)
}

/// Every staged build in `install`; none when nothing was ever staged.
pub fn load(install: &Path) -> Result<Vec<Staged>, StagedError> {
    let path = path(install);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(StagedError::Read { path, source }),
    };
    toml::from_str::<Record>(&text).map(|r| r.builds).map_err(|source| StagedError::Parse { path, source })
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
    let mut builds = load(install)?;
    builds.retain(|b| b.product != staged.product);
    builds.push(staged);
    save(install, builds)
}

/// Forget `code`'s staged build, or only the one keyed `build_key` when given.
/// Whether anything was forgotten.
pub fn clear(install: &Path, code: &ProductCode, build_key: Option<&str>) -> Result<bool, StagedError> {
    let mut builds = load(install)?;
    let before = builds.len();
    builds.retain(|b| !(&b.product == code && build_key.is_none_or(|key| b.build_key == key)));
    if builds.len() == before {
        return Ok(false);
    }
    save(install, builds)?;
    Ok(true)
}

/// Write atomically; nothing staged removes the file.
fn save(install: &Path, builds: Vec<Staged>) -> Result<(), StagedError> {
    let path = path(install);
    let write = |source| StagedError::Write { path: path.clone(), source };
    if builds.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(write(e)),
            _ => Ok(()),
        };
    }
    let text = toml::to_string_pretty(&Record { builds })?;
    crate::fsutil::write_atomic(&path, text.as_bytes()).map_err(write)
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
