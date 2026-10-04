//! Installing, updating and removing addons.
//!
//! An addon package is a zip whose top level is one or more addon folders,
//! which are extracted straight into `Interface/AddOns`. Several folders per
//! package is normal, not exotic — BigWigs ships eleven — so an install is
//! tracked as a *set* of folders, and removing it removes exactly that set.
//!
//! Two rules here are safety rather than bookkeeping:
//!
//! 1. **Nothing is written outside the AddOns directory.** A zip entry may name
//!    `../../../.bashrc` or an absolute path, and extracting it verbatim is the
//!    classic zip-slip. Every path is validated before a byte is written.
//! 2. **Only what the ledger records is ever deleted.** Hand-installed addons
//!    and addons from another manager are never touched, because Lightforge did
//!    not put them there.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::catalog::Resolved;
use crate::product::ProductCode;

/// Refuse a package that expands beyond this. A zip bomb is a 40 KB download
/// that fills a disk; no real addon is anywhere near this size.
pub const MAX_UNCOMPRESSED: u64 = 512 * 1024 * 1024;

/// What Lightforge installed, so it can be updated and removed again.
///
/// Kept in Lightforge's own data directory rather than inside the game, so a
/// reinstall of the game does not silently orphan it and Agent never sees a
/// file it did not write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub addons: Vec<Record>,
}

/// One installed addon.
///
/// `Default` exists for tests and for growing the struct: a new per-addon
/// setting should not mean editing every literal that never cared about it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub provider: String,
    pub id: String,
    pub name: String,
    /// Catalog file id currently installed — what an update compares against.
    pub file_id: Option<String>,
    /// Every folder the package wrote. Removal deletes exactly these.
    pub folders: Vec<String>,
    /// Which builds this addon accepts, overriding the global default.
    ///
    /// `None` means "follow the default", not "stable" — so changing the
    /// default moves every addon that was never given an opinion of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<super::Channel>,
    /// What kind of build is actually installed — a *fact*, where
    /// [`Record::channel`] is a *preference*.
    ///
    /// Without it, moving an addon from alpha back to stable looks exactly
    /// like an update, because the only thing to compare is two opaque file
    /// ids. `None` means it was installed before this was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<super::Channel>,
    /// Left alone by anything that updates in bulk.
    ///
    /// For an addon kept at a version on purpose, or one a parent package
    /// drags along: without it, every "Update all" quietly undoes the choice.
    /// An explicit Update on the addon itself still works — this is about what
    /// happens *without* being asked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ignored: bool,
}

/// `skip_serializing_if` needs a path; `bool::not` is not one.
fn is_false(value: &bool) -> bool {
    !*value
}

impl Ledger {
    /// Where a product's ledger lives.
    ///
    /// Keyed by the AddOns directory as well as the product, because a product
    /// code alone is not an install: a second install of the same product (a
    /// test root, a second copy of the game) would otherwise share one ledger
    /// and each would be told it manages the other's addons.
    pub fn path_for(data_dir: &Path, addons_dir: &Path, code: &ProductCode) -> PathBuf {
        use sha2::{Digest, Sha256};

        let digest = Sha256::digest(addons_dir.as_os_str().as_encoded_bytes());
        let tag: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        data_dir
            .join("addons")
            .join(format!("{}-{tag}.json", code.as_str()))
    }

    /// Load a ledger, treating a missing file as an empty one.
    ///
    /// A corrupt ledger is an error rather than a silent reset: it is the only
    /// record of what may be deleted, and guessing would mean either orphaning
    /// addons or deleting someone else's.
    pub fn load(path: &Path) -> Result<Self, InstallError> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| InstallError::Ledger {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(InstallError::Io { path: path.to_path_buf(), source }),
        }
    }

    /// Write atomically — temp file beside it, then rename.
    pub fn save(&self, path: &Path) -> Result<(), InstallError> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(|source| InstallError::Io {
            path: dir.to_path_buf(),
            source,
        })?;

        let json = serde_json::to_vec_pretty(self).expect("ledger serializes");
        let temp = dir.join(".addons.json.tmp");
        std::fs::write(&temp, &json).map_err(|source| InstallError::Io {
            path: temp.clone(),
            source,
        })?;
        std::fs::rename(&temp, path).map_err(|source| InstallError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn find(&self, provider: &str, id: &str) -> Option<&Record> {
        self.addons.iter().find(|r| r.id == id && r.provider == provider)
    }

    pub fn find_mut(&mut self, provider: &str, id: &str) -> Option<&mut Record> {
        self.addons.iter_mut().find(|r| r.id == id && r.provider == provider)
    }

    /// Record an install, replacing any previous record of the same addon.
    pub fn record(&mut self, record: Record) {
        self.addons
            .retain(|r| !(r.id == record.id && r.provider == record.provider));
        self.addons.push(record);
        self.addons.sort_by_key(|r| r.name.to_lowercase());
    }

    /// Drop records whose folders have all vanished from disk.
    ///
    /// Addons deleted by hand, or lost to a reinstall of the game, would
    /// otherwise be offered for update and remove forever. A record keeping
    /// even one of its folders is left alone: a partly deleted package is
    /// still ours to repair.
    ///
    /// Callers must only reconcile against a directory they can actually read
    /// — an AddOns directory that is missing because the library is offline
    /// says nothing about what is installed.
    pub fn prune_missing(&mut self, addons_dir: &Path) -> Vec<String> {
        let mut dropped = Vec::new();
        self.addons.retain(|record| {
            let alive = record
                .folders
                .iter()
                .any(|folder| addons_dir.join(folder).is_dir());
            if !alive {
                dropped.push(record.name.clone());
            }
            alive
        });
        dropped
    }

    pub fn forget(&mut self, provider: &str, id: &str) -> Option<Record> {
        let index = self
            .addons
            .iter()
            .position(|r| r.id == id && r.provider == provider)?;
        Some(self.addons.remove(index))
    }

    /// Whether a resolved addon is newer than what is recorded.
    ///
    /// An addon with no recorded file id is treated as needing an update: it
    /// was installed before we tracked versions, and refreshing it is harmless.
    pub fn needs_update(&self, resolved: &Resolved, provider: &str) -> bool {
        match self.find(provider, &resolved.id) {
            None => true,
            Some(record) => match (&record.file_id, &resolved.file_id) {
                (Some(installed), Some(available)) => installed != available,
                _ => true,
            },
        }
    }
}

/// What an extraction produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// Top-level addon folders written, in the order found.
    pub folders: Vec<String>,
    pub files: usize,
    pub bytes: u64,
}

/// Extract an addon package into `addons_dir`.
///
/// Replaces the folders it contains: an update must not leave a removed file
/// from the previous version behind, since a stale `.lua` can break an addon
/// far more confusingly than a missing one.
pub fn extract(package: &[u8], addons_dir: &Path) -> Result<Extracted, InstallError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(package)).map_err(InstallError::Archive)?;

    // Validate everything *before* writing anything, so a malicious entry
    // halfway through cannot leave a half-installed addon behind.
    let mut planned: Vec<(usize, PathBuf, String)> = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(InstallError::Archive)?;
        if entry.is_dir() {
            continue;
        }
        let raw = entry
            .enclosed_name()
            .ok_or_else(|| InstallError::UnsafePath(entry.name().to_owned()))?;
        let relative = safe_relative(&raw).ok_or_else(|| {
            InstallError::UnsafePath(raw.display().to_string())
        })?;
        let folder = relative
            .components()
            .next()
            .and_then(|c| match c {
                Component::Normal(name) => name.to_str().map(str::to_owned),
                _ => None,
            })
            .ok_or_else(|| InstallError::UnsafePath(raw.display().to_string()))?;

        // A file at the zip's top level belongs to no addon folder; WoW would
        // ignore it and we have nowhere sensible to put it.
        if relative.components().count() < 2 {
            continue;
        }

        total += entry.size();
        if total > MAX_UNCOMPRESSED {
            return Err(InstallError::TooLarge { limit: MAX_UNCOMPRESSED });
        }
        planned.push((index, relative, folder));
    }

    let mut folders: Vec<String> = Vec::new();
    for (_, _, folder) in &planned {
        if !folders.contains(folder) {
            folders.push(folder.clone());
        }
    }
    if folders.is_empty() {
        return Err(InstallError::NoAddonFolders);
    }

    // Clear the previous copy of each folder this package owns.
    for folder in &folders {
        let target = addons_dir.join(folder);
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|source| InstallError::Io {
                path: target,
                source,
            })?;
        }
    }

    let mut files = 0usize;
    let mut bytes = 0u64;
    for (index, relative, _) in &planned {
        let mut entry = archive.by_index(*index).map_err(InstallError::Archive)?;
        let dest = addons_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| InstallError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let mut contents = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut contents).map_err(|source| InstallError::Io {
            path: dest.clone(),
            source,
        })?;
        std::fs::write(&dest, &contents).map_err(|source| InstallError::Io {
            path: dest.clone(),
            source,
        })?;

        files += 1;
        bytes += contents.len() as u64;
    }

    Ok(Extracted { folders, files, bytes })
}

/// Delete the folders an install wrote, and forget it.
///
/// Only folders the ledger records are removed, and each is checked to be
/// inside `addons_dir` before anything is deleted.
pub fn remove(
    ledger: &mut Ledger,
    addons_dir: &Path,
    provider: &str,
    id: &str,
) -> Result<Vec<String>, InstallError> {
    let record = ledger
        .forget(provider, id)
        .ok_or_else(|| InstallError::NotTracked(id.to_owned()))?;

    let mut removed = Vec::new();
    for folder in &record.folders {
        if safe_relative(Path::new(folder)).is_none() {
            return Err(InstallError::UnsafePath(folder.clone()));
        }
        let target = addons_dir.join(folder);
        match std::fs::remove_dir_all(&target) {
            Ok(()) => removed.push(folder.clone()),
            // Already gone by hand: nothing to do, and not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(InstallError::Io { path: target, source }),
        }
    }
    Ok(removed)
}

/// Normalise a zip path and refuse anything that escapes the destination.
///
/// Rejects absolute paths, `..`, Windows drive prefixes, and empty results;
/// accepts a plain relative path, dropping any `./` segments.
fn safe_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str()?;
                // A backslash survives as a literal on Unix, so a Windows-style
                // path would otherwise become one giant file name.
                if text.contains('\\') || text.contains('/') {
                    return None;
                }
                out.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Group installed folders by the addon that owns them, from the ledger.
pub fn owners(ledger: &Ledger) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for record in &ledger.addons {
        for folder in &record.folders {
            map.insert(folder.clone(), record.name.clone());
        }
    }
    map
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the package is not a readable zip")]
    Archive(#[source] zip::result::ZipError),

    #[error("the package contains an unsafe path: {0}")]
    UnsafePath(String),

    #[error("the package expands past the {limit} byte limit")]
    TooLarge { limit: u64 },

    #[error("the package contains no addon folders")]
    NoAddonFolders,

    #[error("{0} was not installed by Lightforge, so it will not be removed")]
    NotTracked(String),

    #[error("the addon ledger at {path} is unreadable")]
    Ledger {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            for (name, body) in entries {
                writer.start_file(*name, SimpleFileOptions::default()).unwrap();
                writer.write_all(body).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer
    }

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-addon-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_multi_folder_package_installs_every_folder() {
        // BigWigs ships eleven folders in one zip; one folder is the exception.
        let package = zip_with(&[
            ("BigWigs/BigWigs.toc", b"## Title: BigWigs"),
            ("BigWigs/core.lua", b"-- core"),
            ("BigWigs_Options/BigWigs_Options.toc", b"## Title: Options"),
        ]);
        let dir = tmpdir("multi");

        let extracted = extract(&package, &dir).unwrap();
        assert_eq!(extracted.folders, vec!["BigWigs", "BigWigs_Options"]);
        assert_eq!(extracted.files, 3);
        assert!(dir.join("BigWigs/BigWigs.toc").is_file());
        assert!(dir.join("BigWigs_Options/BigWigs_Options.toc").is_file());
    }

    #[test]
    fn a_path_escaping_the_addons_directory_is_refused_before_anything_is_written() {
        let dir = tmpdir("traversal");
        let package = zip_with(&[
            ("Good/Good.toc", b"## Title: Good"),
            ("../../../evil.txt", b"pwned"),
        ]);

        let err = extract(&package, &dir).unwrap_err();
        assert!(
            matches!(err, InstallError::UnsafePath(_)),
            "expected a path refusal, got {err:?}"
        );
        // Nothing was written, not even the entry that was fine.
        assert!(!dir.join("Good").exists());
        assert!(!dir.parent().unwrap().join("evil.txt").exists());
    }

    #[test]
    fn an_update_replaces_a_folder_rather_than_merging_into_it() {
        let dir = tmpdir("replace");
        extract(&zip_with(&[("Addon/old.lua", b"old"), ("Addon/keep.toc", b"v1")]), &dir).unwrap();
        assert!(dir.join("Addon/old.lua").is_file());

        // The new version dropped old.lua. A stale file left behind is worse
        // than a missing one: it still loads.
        extract(&zip_with(&[("Addon/keep.toc", b"v2")]), &dir).unwrap();
        assert!(!dir.join("Addon/old.lua").exists());
        assert_eq!(std::fs::read(dir.join("Addon/keep.toc")).unwrap(), b"v2");
    }

    #[test]
    fn a_package_with_only_top_level_files_has_no_addon_to_install() {
        let dir = tmpdir("flat");
        let package = zip_with(&[("readme.txt", b"hello")]);
        assert!(matches!(extract(&package, &dir), Err(InstallError::NoAddonFolders)));
    }

    #[test]
    fn removal_deletes_only_what_the_ledger_recorded() {
        let dir = tmpdir("remove");
        extract(&zip_with(&[("Mine/Mine.toc", b"x"), ("Mine_Extra/Mine_Extra.toc", b"y")]), &dir)
            .unwrap();
        std::fs::create_dir_all(dir.join("HandInstalled")).unwrap();
        std::fs::write(dir.join("HandInstalled/thing.toc"), b"not ours").unwrap();

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "1".into(),
            name: "Mine".into(),
            file_id: Some("100".into()),
            folders: vec!["Mine".into(), "Mine_Extra".into()],
            channel: None,
            ..Default::default()
        });

        let removed = remove(&mut ledger, &dir, "curse", "1").unwrap();
        assert_eq!(removed, vec!["Mine", "Mine_Extra"]);
        assert!(!dir.join("Mine").exists());
        // The addon we did not install is untouched.
        assert!(dir.join("HandInstalled/thing.toc").is_file());
        assert!(ledger.find("curse", "1").is_none());
    }

    #[test]
    fn an_untracked_addon_is_refused_rather_than_guessed_at() {
        let dir = tmpdir("untracked");
        let mut ledger = Ledger::default();
        assert!(matches!(
            remove(&mut ledger, &dir, "curse", "999"),
            Err(InstallError::NotTracked(_))
        ));
    }

    #[test]
    fn updates_are_decided_by_file_id() {
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: Some("100".into()),
            folders: vec!["BigWigs".into()],
            channel: None,
            ..Default::default()
        });

        let mut resolved = Resolved {
            id: "2382".into(),
            status: super::super::Status::Ok,
            name: Some("BigWigs".into()),
            file_id: Some("100".into()),
            file_name: None,
            file_length: None,
            md5: None,
            download_url: Some("https://x/y.zip".into()),
            folders: vec![],
            website_url: None,
            note: None,
            channel: None,
        };
        assert!(!ledger.needs_update(&resolved, "curse"));

        resolved.file_id = Some("101".into());
        assert!(ledger.needs_update(&resolved, "curse"));

        // Something we never installed always needs installing.
        resolved.id = "9999".into();
        assert!(ledger.needs_update(&resolved, "curse"));
    }

    #[test]
    fn two_installs_of_one_product_do_not_share_a_ledger() {
        // The symptom this prevents, seen for real: a test install's ledger
        // reported "3 managed" against a different, empty install.
        let data = PathBuf::from("/data");
        let a = Ledger::path_for(&data, Path::new("/games/a/Interface/AddOns"), &ProductCode::Retail);
        let b = Ledger::path_for(&data, Path::new("/games/b/Interface/AddOns"), &ProductCode::Retail);

        assert_ne!(a, b);
        // Still readable at a glance, and stable for the same input.
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("wow-"));
        assert_eq!(a, Ledger::path_for(&data, Path::new("/games/a/Interface/AddOns"), &ProductCode::Retail));
    }

    #[test]
    fn a_ledger_round_trips_and_a_missing_one_is_empty() {
        let dir = tmpdir("ledger");
        let path = Ledger::path_for(&dir, Path::new("/games/x/AddOns"), &ProductCode::Retail);
        assert_eq!(Ledger::load(&path).unwrap(), Ledger::default());

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: Some("8866508".into()),
            folders: vec!["BigWigs".into()],
            channel: None,
            ..Default::default()
        });
        ledger.save(&path).unwrap();

        assert_eq!(Ledger::load(&path).unwrap(), ledger);
    }

    #[test]
    fn pruning_drops_only_the_records_whose_folders_are_all_gone() {
        let addons = tmpdir("prune");
        std::fs::create_dir_all(addons.join("BigWigs")).unwrap();

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: None,
            // Half the package deleted by hand: still ours to repair.
            folders: vec!["BigWigs".into(), "BigWigs_Options".into()],
            channel: None,
            ..Default::default()
        });
        ledger.record(Record {
            provider: "tukui".into(),
            id: "elvui".into(),
            name: "ElvUI".into(),
            file_id: None,
            folders: vec!["ElvUI".into(), "ElvUI_Options".into()],
            channel: None,
            ..Default::default()
        });

        assert_eq!(ledger.prune_missing(&addons), vec!["ElvUI".to_owned()]);
        assert_eq!(ledger.addons.len(), 1);
        assert_eq!(ledger.addons[0].name, "BigWigs");

        // Idempotent, and a file where a folder belongs does not count.
        std::fs::write(addons.join("ElvUI"), b"not a folder").unwrap();
        assert!(ledger.prune_missing(&addons).is_empty());
    }
}
