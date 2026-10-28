//! Installing, updating and removing addons.
//!
//! An addon package is a zip whose top level is one or more addon folders,
//! which are extracted straight into `Interface/AddOns`. Several folders per
//! package is normal, not exotic, BigWigs ships eleven, so an install is
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

use std::ffi::OsString;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::catalog::Resolved;
use crate::product::ProductCode;

/// Refuse a package that expands beyond this. A zip bomb is a 40 KB download
/// that fills a disk; no real addon is anywhere near this size.
pub const MAX_UNCOMPRESSED: u64 = 512 * 1024 * 1024;

/// Refuse a package download past this, half of [`MAX_UNCOMPRESSED`].
pub const MAX_PACKAGE_BYTES: usize = 256 * 1024 * 1024;

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
    /// Catalog file id currently installed, what an update compares against.
    pub file_id: Option<String>,
    /// Every folder the package wrote. Removal deletes exactly these.
    pub folders: Vec<String>,
    /// Which builds this addon accepts, overriding the global default.
    ///
    /// `None` means "follow the default", not "stable", so changing the
    /// default moves every addon that was never given an opinion of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<super::Channel>,
    /// What kind of build is actually installed, a *fact*, where
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
    /// An explicit Update on the addon itself still works. This is about what
    /// happens *without* being asked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ignored: bool,
    /// Updated without being asked, when Lightforge makes its own pass.
    ///
    /// `None` means "follow the app-wide default", the way [`Record::channel`]
    /// does, so changing that default moves every addon that was never given
    /// an opinion of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
    /// Held at this catalog file id, whatever is newer.
    ///
    /// A file id rather than a version string, because that is what both
    /// providers and [`Record::file_id`] trade in: a version string would
    /// have to be matched back to a file to be acted on, and two files can
    /// carry the same one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<String>,
    /// The day this build was installed, `YYYY-MM-DD`.
    ///
    /// Ours to record, because no provider knows it: the catalogue can say
    /// when a build was *published*, which is a different question from when
    /// this machine took it. `None` on anything installed before this was
    /// recorded, an absent fact, not a zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    /// The project page at its provider.
    ///
    /// Recorded here rather than read from an update check, because
    /// [`crate::addons::manage::check_updates`] deliberately asks nothing
    /// for a pinned or ignored addon, and those rows want the link as much
    /// as any other. `None` on anything installed before this was recorded,
    /// which only costs the menu entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    /// The author's page at the provider, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_url: Option<String>,
    /// The project's source repository, when the provider publishes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// Today, as `YYYY-MM-DD` in UTC.
///
/// UTC rather than local time because the string is stored and compared, not
/// announced: a ledger written in one timezone and read in another should not
/// disagree with itself about what day it was.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let (year, month, day) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Days since the epoch to a civil date, by Howard Hinnant's `civil_from_days`.
///
/// Written out rather than pulled in: one date, one direction, no timezones
/// and no formatting, a calendar crate for this would be a dependency to
/// audit and update forever in exchange for nine lines.
fn civil_from_unix(secs: u64) -> (i64, u32, u32) {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = match mp < 10 {
        true => (mp + 3) as u32,
        false => (mp - 9) as u32,
    };
    (y + i64::from(m <= 2), m, d)
}

impl Record {
    /// Whether an unattended pass should update this addon.
    ///
    /// Ignoring wins over auto-updating, and is not a contradiction to be
    /// resolved: "leave this alone" is the stronger of the two statements,
    /// and an addon can carry an auto-update choice from before it was
    /// ignored that should come back if it stops being ignored.
    pub fn updates_itself(&self, default: bool) -> bool {
        self.tracks_updates() && self.auto_update.unwrap_or(default)
    }

    /// Whether this addon moves with its channel at all.
    ///
    /// Two different statements, both ending here: ignoring means "not in any
    /// pass", pinning means "not to any other version". Either one makes an
    /// update request for this addon pointless.
    pub fn tracks_updates(&self) -> bool {
        !self.ignored && self.pinned.is_none()
    }
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

    /// Write atomically: a temp file of its own beside it, fsynced, then
    /// renamed. Every product's ledger lives in one directory, so a shared
    /// temp name let two saves rename each other's contents into place.
    pub fn save(&self, path: &Path) -> Result<(), InstallError> {
        let json = serde_json::to_vec_pretty(self).expect("ledger serializes");
        crate::fsutil::write_atomic(path, &json)
            .map_err(|source| InstallError::Io { path: path.to_path_buf(), source })
    }

    pub fn find(&self, provider: &str, id: &str) -> Option<&Record> {
        self.addons.iter().find(|r| r.id == id && r.provider == provider)
    }

    pub fn find_mut(&mut self, provider: &str, id: &str) -> Option<&mut Record> {
        self.addons.iter_mut().find(|r| r.id == id && r.provider == provider)
    }

    /// Record an install, replacing any previous record of the same addon.
    ///
    /// A folder belongs to one record. The package just extracted replaced
    /// these folders on disk, so another record claiming them, the same addon
    /// from a different provider most often, loses them, and goes once it
    /// has none left. Two owners meant removing either deleted folders the
    /// other was still counting on. Returns the names of records dropped.
    pub fn record(&mut self, record: Record) -> Vec<String> {
        self.addons
            .retain(|r| !(r.id == record.id && r.provider == record.provider));
        let mut dropped = Vec::new();
        for other in &mut self.addons {
            other.folders.retain(|f| !record.folders.contains(f));
        }
        self.addons.retain(|r| {
            let keep = !r.folders.is_empty();
            if !keep {
                dropped.push(r.name.clone());
            }
            keep
        });
        self.addons.push(record);
        self.addons.sort_by_key(|r| r.name.to_lowercase());
        dropped
    }

    /// Drop records whose folders have all vanished from disk.
    ///
    /// Addons deleted by hand, or lost to a reinstall of the game, would
    /// otherwise be offered for update and remove forever. A record keeping
    /// even one of its folders is left alone: a partly deleted package is
    /// still ours to repair.
    ///
    /// Callers must only reconcile against a directory they can actually read,
    /// an AddOns directory that is missing because the library is offline
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
/// Replaces whole folders, since a stale `.lua` left from the last version
/// breaks an addon more confusingly than a missing one; `retire` names extra
/// folders to remove in the same step. The package is written to a staging
/// folder beside `AddOns` first, then swapped in by rename, so a full disk
/// leaves the old addon intact and a failed rename puts back what it swapped.
pub fn extract(package: &[u8], addons_dir: &Path, retire: &[String]) -> Result<Extracted, InstallError> {
    extract_within(package, addons_dir, retire, MAX_UNCOMPRESSED)
}

/// [`extract`] with the size limit as a parameter, so a test can trip it
/// without writing half a gigabyte.
fn extract_within(
    package: &[u8],
    addons_dir: &Path,
    retire: &[String],
    limit: u64,
) -> Result<Extracted, InstallError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(package)).map_err(InstallError::Archive)?;

    // Validate everything *before* writing anything, so a malicious entry
    // halfway through cannot leave a half-installed addon behind.
    let mut planned: Vec<(usize, PathBuf, String)> = Vec::new();
    // In first-seen order. The client sees `BigWigs` and `bigwigs` as one
    // folder, so they are one here too, under the spelling met first, and
    // every entry is filed under it.
    let mut folders: Vec<String> = Vec::new();
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
        let (folder, relative) = match folders.iter().find(|f| f.eq_ignore_ascii_case(&folder)) {
            Some(first) if *first != folder => {
                (first.clone(), Path::new(first).join(relative.components().skip(1).collect::<PathBuf>()))
            }
            Some(_) => (folder, relative),
            None => {
                folders.push(folder.clone());
                (folder, relative)
            }
        };

        // What the archive claims, a cheap early refusal. The sizes are the
        // packager's word, so the limit is enforced again below on the bytes
        // that actually come out.
        total = total.saturating_add(entry.size());
        if total > limit {
            return Err(InstallError::TooLarge { limit });
        }
        planned.push((index, relative, folder));
    }

    if folders.is_empty() {
        return Err(InstallError::NoAddonFolders);
    }

    // A retired folder is replaced wholesale like the others, so it has to be
    // one plain name inside `AddOns`, the same rule the ledger's are held to.
    for folder in retire {
        if Path::new(folder).components().count() != 1 || safe_relative(Path::new(folder)).is_none() {
            return Err(InstallError::UnsafePath(folder.clone()));
        }
    }

    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| InstallError::Io { path, source }
    };
    // `AddOns` sits two levels below the client's own folder. Creating the
    // whole chain would conjure a `_retail_` for a game that is not installed,
    // and the library would then look like it holds one.
    if let Some(client) = addons_dir.ancestors().nth(2) {
        if !client.is_dir() {
            return Err(InstallError::GameNotInstalled(client.to_path_buf()));
        }
    }
    std::fs::create_dir_all(addons_dir).map_err(io(addons_dir))?;
    // `Interface`, not `AddOns`: neither the client nor `scan` ever looks
    // at a folder there that is not an addon.
    let interface = addons_dir.parent().unwrap_or(addons_dir);
    // Dropped at the end either way, taking the replaced copies with it.
    let staging = crate::staging::create_in(interface).map_err(io(interface))?;
    let fresh = staging.path().join("new");
    let old = staging.path().join("old");

    let mut files = 0usize;
    let mut bytes = 0u64;
    for (index, relative, _) in &planned {
        let mut entry = archive.by_index(*index).map_err(InstallError::Archive)?;
        let dest = fresh.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| InstallError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        // Streamed, and capped at what is left of the budget plus one byte so
        // going over is detectable. A header that says 1 byte in front of a
        // deflate stream that inflates to gigabytes is otherwise read to the
        // end: the zip reader bounds the compressed bytes, not the output.
        let remaining = limit - bytes;
        let io = |source| InstallError::Io { path: dest.clone(), source };
        let mut file = std::fs::File::create(&dest).map_err(io)?;
        let written = std::io::copy(&mut (&mut entry).take(remaining + 1), &mut file).map_err(io)?;
        if written > remaining {
            return Err(InstallError::TooLarge { limit });
        }

        files += 1;
        bytes += written;
    }

    let retired: Vec<&String> =
        retire.iter().filter(|f| !folders.iter().any(|g| g.eq_ignore_ascii_case(f))).collect();
    swap(addons_dir, &fresh, &old, &folders, &retired)?;
    Ok(Extracted { folders, files, bytes })
}

/// Move the installed folders out to `old` and the staged ones in, putting
/// everything back if any rename fails.
fn swap(
    addons_dir: &Path,
    fresh: &Path,
    old: &Path,
    folders: &[String],
    retired: &[&String],
) -> Result<(), InstallError> {
    std::fs::create_dir_all(old).map_err(|source| InstallError::Io { path: old.to_path_buf(), source })?;
    let mut moved_out: Vec<OsString> = Vec::new();
    let mut moved_in: Vec<&str> = Vec::new();

    let result = swap_steps(addons_dir, fresh, old, folders, retired, &mut moved_out, &mut moved_in);
    if result.is_err() {
        for name in moved_in.iter().rev() {
            let _ = std::fs::rename(addons_dir.join(name), fresh.join(name));
        }
        for name in moved_out.iter().rev() {
            let _ = std::fs::rename(old.join(name), addons_dir.join(name));
        }
    }
    result
}

fn swap_steps<'a>(
    addons_dir: &Path,
    fresh: &Path,
    old: &Path,
    folders: &'a [String],
    retired: &[&'a String],
    moved_out: &mut Vec<OsString>,
    moved_in: &mut Vec<&'a str>,
) -> Result<(), InstallError> {
    for name in folders.iter().chain(retired.iter().copied()) {
        // Found ignoring case: an update that ships `bigwigs` replaces the
        // installed `BigWigs` rather than landing beside it.
        let Some(existing) = on_disk(addons_dir, name) else {
            // Not installed yet, or already removed by hand.
            continue;
        };
        let target = addons_dir.join(&existing);
        std::fs::rename(&target, old.join(&existing))
            .map_err(|source| InstallError::Io { path: target, source })?;
        moved_out.push(existing);
    }
    for name in folders {
        let target = addons_dir.join(name);
        std::fs::rename(fresh.join(name), &target)
            .map_err(|source| InstallError::Io { path: target, source })?;
        moved_in.push(name);
    }
    Ok(())
}

/// How `name` is spelled in `dir`, matching ASCII case-insensitively as Wine
/// does. An exact match is taken without listing the directory.
fn on_disk(dir: &Path, name: &str) -> Option<OsString> {
    if dir.join(name).symlink_metadata().is_ok() {
        return Some(name.into());
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name())
        .find(|existing| existing.as_encoded_bytes().eq_ignore_ascii_case(name.as_bytes()))
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

    // Never a folder another record still names. `record` keeps that from
    // arising now, but ledgers written before it can hold one folder twice.
    let own: Vec<String> = record
        .folders
        .into_iter()
        .filter(|f| !ledger.addons.iter().any(|r| r.folders.contains(f)))
        .collect();
    delete_folders(addons_dir, &own)
}

/// Delete addon folders, and report which were actually there.
///
/// Only ever called with folders a [`Record`] names, the rule that nothing
/// Lightforge did not install is ever deleted lives in the callers, and the
/// path check here is the second lock on it.
pub fn delete_folders(addons_dir: &Path, folders: &[String]) -> Result<Vec<String>, InstallError> {
    let mut removed = Vec::new();
    for folder in folders {
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

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the package is not a readable zip")]
    Archive(#[source] zip::result::ZipError),

    #[error("the package contains an unsafe path: {0}")]
    UnsafePath(String),

    #[error("the package expands past the {limit} byte limit")]
    TooLarge { limit: u64 },

    #[error("the game is not installed ({0} does not exist)")]
    GameNotInstalled(PathBuf),

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

    /// Rewrite every declared uncompressed size in `zip` to 1 byte, in the
    /// local headers and the central directory both, the way a hostile
    /// package would lie about it.
    fn understated(mut zip: Vec<u8>) -> Vec<u8> {
        let mut at = 0;
        while at + 4 <= zip.len() {
            let field = match &zip[at..at + 4] {
                b"PK\x03\x04" => Some(at + 22),
                b"PK\x01\x02" => Some(at + 24),
                _ => None,
            };
            if let Some(field) = field {
                zip[field..field + 4].copy_from_slice(&1u32.to_le_bytes());
            }
            at += 1;
        }
        zip
    }

    #[test]
    fn an_entry_that_inflates_past_its_declared_size_is_stopped() {
        let body = vec![0u8; 64 * 1024];
        let package = understated(zip_with(&[("Bomb/Bomb.toc", b"## Title: Bomb"), ("Bomb/big.lua", &body)]));
        let dir = tmpdir("understated");
        let addons = dir.join("Interface/AddOns");

        // The declared total is 2 bytes, well under the limit; only the
        // bytes actually written can catch this.
        let result = extract_within(&package, &addons, &[], 4 * 1024);
        assert!(matches!(result, Err(InstallError::TooLarge { limit: 4096 })), "{result:?}");
        assert!(!addons.join("Bomb").exists(), "nothing half-written is left installed");
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

        let extracted = extract(&package, &dir, &[]).unwrap();
        assert_eq!(extracted.folders, vec!["BigWigs", "BigWigs_Options"]);
        assert_eq!(extracted.files, 3);
        assert!(dir.join("BigWigs/BigWigs.toc").is_file());
        assert!(dir.join("BigWigs_Options/BigWigs_Options.toc").is_file());
    }

    #[test]
    fn an_update_spelled_in_another_case_replaces_the_installed_folder() {
        let dir = tmpdir("case-update");
        std::fs::create_dir_all(dir.join("BigWigs")).unwrap();
        std::fs::write(dir.join("BigWigs/old.lua"), b"-- old").unwrap();
        let package = zip_with(&[("bigwigs/bigwigs.toc", b"## Title: BigWigs")]);

        let extracted = extract(&package, &dir, &[]).unwrap();

        assert_eq!(extracted.folders, ["bigwigs"]);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["bigwigs"], "the old spelling is not left beside the new one");
        assert!(!dir.join("bigwigs/old.lua").exists());
    }

    #[test]
    fn one_folder_spelled_two_ways_in_a_package_is_installed_once() {
        let dir = tmpdir("case-package");
        let package = zip_with(&[
            ("BigWigs/BigWigs.toc", b"## Title: BigWigs"),
            ("bigwigs/core.lua", b"-- core"),
        ]);

        let extracted = extract(&package, &dir, &[]).unwrap();

        assert_eq!(extracted.folders, ["BigWigs"]);
        assert!(dir.join("BigWigs/BigWigs.toc").is_file());
        assert!(dir.join("BigWigs/core.lua").is_file(), "filed under the first spelling");
        assert!(!dir.join("bigwigs").exists());
    }

    #[test]
    fn a_path_escaping_the_addons_directory_is_refused_before_anything_is_written() {
        let dir = tmpdir("traversal");
        let package = zip_with(&[
            ("Good/Good.toc", b"## Title: Good"),
            ("../../../evil.txt", b"pwned"),
        ]);

        let err = extract(&package, &dir, &[]).unwrap_err();
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
        extract(&zip_with(&[("Addon/old.lua", b"old"), ("Addon/keep.toc", b"v1")]), &dir, &[]).unwrap();
        assert!(dir.join("Addon/old.lua").is_file());

        // The new version dropped old.lua. A stale file left behind is worse
        // than a missing one: it still loads.
        extract(&zip_with(&[("Addon/keep.toc", b"v2")]), &dir, &[]).unwrap();
        assert!(!dir.join("Addon/old.lua").exists());
        assert_eq!(std::fs::read(dir.join("Addon/keep.toc")).unwrap(), b"v2");
    }

    /// An `AddOns` inside its own `Interface`, so the staging folder an
    /// extract makes is this test's alone to look for.
    fn addons_dir(name: &str) -> PathBuf {
        let dir = tmpdir(name).join("AddOns");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn staging_left(addons: &Path) -> bool {
        std::fs::read_dir(addons.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(crate::staging::PREFIX))
    }

    #[test]
    fn a_failed_write_leaves_the_installed_addon_untouched() {
        let dir = addons_dir("failed-write");
        extract(&zip_with(&[("Addon/Addon.toc", b"v1"), ("Addon/core.lua", b"v1")]), &dir, &[]).unwrap();

        // `x` as a file and then as a folder: the second write fails halfway
        // through the package, the way a full disk would.
        let broken = zip_with(&[("Addon/Addon.toc", b"v2"), ("Addon/x", b"file"), ("Addon/x/y", b"dir")]);
        assert!(matches!(extract(&broken, &dir, &[]), Err(InstallError::Io { .. })));

        assert_eq!(std::fs::read(dir.join("Addon/Addon.toc")).unwrap(), b"v1");
        assert_eq!(std::fs::read(dir.join("Addon/core.lua")).unwrap(), b"v1");
        assert!(!staging_left(&dir));
    }

    #[test]
    fn nothing_is_created_for_a_game_that_is_not_installed() {
        let client = tmpdir("no-game").join("_retail_");
        let dir = client.join("Interface").join("AddOns");

        let result = extract(&zip_with(&[("Addon/Addon.toc", b"v1")]), &dir, &[]);

        assert!(matches!(result, Err(InstallError::GameNotInstalled(_))));
        assert!(!client.exists());
    }

    #[test]
    fn a_retired_folder_goes_in_the_same_swap() {
        let dir = addons_dir("retire");
        extract(&zip_with(&[("Main/Main.toc", b"v1"), ("Main_Old/Main_Old.toc", b"v1")]), &dir, &[])
            .unwrap();
        std::fs::create_dir_all(dir.join("Someone_Else")).unwrap();

        let retire = ["Main".to_owned(), "Main_Old".to_owned()];
        extract(&zip_with(&[("Main/Main.toc", b"v2")]), &dir, &retire).unwrap();

        assert_eq!(std::fs::read(dir.join("Main/Main.toc")).unwrap(), b"v2");
        assert!(!dir.join("Main_Old").exists());
        assert!(dir.join("Someone_Else").exists());
        assert!(!staging_left(&dir));
    }

    #[test]
    fn a_retired_folder_must_be_one_plain_name() {
        let dir = addons_dir("retire-unsafe");
        let package = zip_with(&[("Main/Main.toc", b"v1")]);
        for bad in ["../Elsewhere", "Main/Sub"] {
            let err = extract(&package, &dir, &[bad.to_owned()]).unwrap_err();
            assert!(matches!(err, InstallError::UnsafePath(_)), "{bad}: {err:?}");
        }
        assert!(!dir.join("Main").exists());
    }

    #[test]
    fn a_package_with_only_top_level_files_has_no_addon_to_install() {
        let dir = tmpdir("flat");
        let package = zip_with(&[("readme.txt", b"hello")]);
        assert!(matches!(extract(&package, &dir, &[]), Err(InstallError::NoAddonFolders)));
    }

    #[test]
    fn removal_deletes_only_what_the_ledger_recorded() {
        let dir = tmpdir("remove");
        extract(&zip_with(&[("Mine/Mine.toc", b"x"), ("Mine_Extra/Mine_Extra.toc", b"y")]), &dir, &[])
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

    fn questie(provider: &str, id: &str, folders: &[&str]) -> Record {
        Record {
            provider: provider.into(),
            id: id.into(),
            name: "Questie".into(),
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn installing_from_a_second_provider_takes_the_folders_over() {
        // Seen live: Questie from WoWInterface, then from CurseForge, left
        // both records claiming `Questie`.
        let mut ledger = Ledger::default();
        ledger.record(questie("wowinterface", "24994", &["Questie"]));
        let dropped = ledger.record(questie("curse", "334372", &["Questie", "QuestieDB"]));

        assert_eq!(dropped, ["Questie"]);
        assert!(ledger.find("wowinterface", "24994").is_none());
        assert_eq!(ledger.addons.len(), 1);
    }

    #[test]
    fn a_record_keeps_the_folders_nobody_else_took() {
        let mut ledger = Ledger::default();
        ledger.record(questie("wowinterface", "1", &["Shared", "OnlyMine"]));
        ledger.record(questie("curse", "2", &["Shared"]));
        assert_eq!(ledger.find("wowinterface", "1").unwrap().folders, ["OnlyMine"]);
    }

    #[test]
    fn removal_leaves_a_folder_another_record_still_names() {
        // A ledger written before ownership was enforced.
        let dir = tmpdir("shared-folder");
        std::fs::create_dir_all(dir.join("Questie")).unwrap();
        std::fs::create_dir_all(dir.join("QuestieDB")).unwrap();
        let mut ledger = Ledger::default();
        ledger.addons.push(questie("wowinterface", "24994", &["Questie"]));
        ledger.addons.push(questie("curse", "334372", &["Questie", "QuestieDB"]));

        let removed = remove(&mut ledger, &dir, "wowinterface", "24994").unwrap();
        assert!(removed.is_empty());
        assert!(dir.join("Questie").is_dir(), "the CurseForge record still owns it");
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
    fn the_civil_date_matches_known_epochs() {
        // Dates nobody can argue with, including the two the algorithm is
        // easiest to get wrong on: a leap day, and the century that is not a
        // leap year's exception.
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29), "a leap day");
        assert_eq!(civil_from_unix(1_709_164_800), (2024, 2, 29));
        assert_eq!(civil_from_unix(1_767_225_600), (2026, 1, 1));
        assert_eq!(civil_from_unix(1_791_331_200), (2026, 10, 7));
        // And the string the ledger stores, zero-padded so it sorts.
        assert_eq!(today().len(), 10);
        assert_eq!(today().matches('-').count(), 2);
    }

    #[test]
    fn deleting_folders_reports_what_was_there_and_refuses_an_escape() {
        let dir = tmpdir("delete-folders");
        std::fs::create_dir_all(dir.join("BigWigs")).unwrap();
        std::fs::write(dir.join("BigWigs/BigWigs.toc"), b"x").unwrap();

        // A folder that is gone already is not a failure: half-deleted is
        // exactly the state a repair exists to fix.
        let removed = delete_folders(
            &dir,
            &["BigWigs".to_owned(), "BigWigs_Core".to_owned()],
        )
        .unwrap();
        assert_eq!(removed, vec!["BigWigs"]);
        assert!(!dir.join("BigWigs").exists());

        // The second lock on "only what the ledger recorded is ever deleted".
        assert!(matches!(
            delete_folders(&dir, &["../elsewhere".to_owned()]),
            Err(InstallError::UnsafePath(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
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
            download_url: Some("https://x/y.zip".into()),
            ..Resolved::default()
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
