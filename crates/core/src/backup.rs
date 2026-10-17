//! Snapshots of a game's `WTF/Account` folder.
//!
//! Everything a player has set up inside the client lives there: every
//! addon's SavedVariables, key bindings, macros, the account-synced cvars. An
//! addon update that migrates its saved data, or a client patch that rewrites
//! it, can lose that in one login, and none of it is anywhere else.
//!
//! One backup is one `.tar.zst` in the app data directory, outside the game
//! library, so uninstalling a game or losing its drive does not take the
//! backups with it. The archive carries its own description as its first
//! member, so a backup is a single file: it is written to a temp file and
//! renamed into place, and there is no index that could disagree with it.
//!
//! A backup is skipped when nothing changed since the newest one on the same
//! build. The unattended addon pass runs on every start, and without that
//! check it would rotate the real history out with identical copies.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::buildinfo::{BuildInfo, COL_VERSION};
use crate::paths::{AppDirs, Layout};
use crate::product::ProductCode;

/// How many backups each game keeps unless the config says otherwise.
pub const DEFAULT_KEEP: usize = 5;

/// The archive's first member, describing the rest.
const META: &str = "lightforge-backup.toml";
/// Where the folder's contents sit inside the archive.
const ROOT: &str = "Account";
const EXTENSION: &str = ".tar.zst";
/// zstd's own default: well past the point where a higher level stops paying
/// for itself on Lua text, and fast enough not to be noticed before a launch.
const LEVEL: i32 = 3;
/// Beside `Account` in `WTF`, so both renames stay on one filesystem.
const STAGING: &str = ".lightforge-restore";
const REPLACED: &str = ".lightforge-replaced";

/// Why a backup was taken, shown beside it so a player can tell which one
/// predates the thing that went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// Asked for.
    Manual,
    /// Before addons that were already installed got replaced.
    AddonUpdate,
    /// Before the first launch on a build no backup was taken on.
    NewBuild,
    /// The state a restore replaced, so a restore can itself be undone.
    BeforeRestore,
}

impl Reason {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::AddonUpdate => "Before addon update",
            Self::NewBuild => "Before first launch on a new build",
            Self::BeforeRestore => "Before restore",
        }
    }

    fn slug(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AddonUpdate => "addon-update",
            Self::NewBuild => "new-build",
            Self::BeforeRestore => "before-restore",
        }
    }
}

/// One backup on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub path: PathBuf,
    /// Milliseconds since the epoch. Finer than the clock a person reads, so
    /// two backups a second apart still sort in the order they were taken.
    pub created: u64,
    pub reason: Reason,
    /// The client build installed when it was taken.
    pub build: Option<String>,
    /// Compressed size in bytes.
    pub size: u64,
    fingerprint: String,
}

impl Backup {
    /// When it was taken, in local time: `2026-10-14 22:52`.
    pub fn when(&self) -> String {
        let secs = (self.created / 1000) as libc::time_t;
        // SAFETY: `localtime_r` is the reentrant form; it reads `secs` and
        // writes only into `tm`, which outlives the call.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return secs.to_string();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
        )
    }

    /// The archive's file name, which is how a backup is named from the CLI.
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// What the archive says about itself.
#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    created: u64,
    reason: Reason,
    build: Option<String>,
    fingerprint: String,
}

/// Where one game's settings are and where its backups go.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The `WTF/Account` folder being protected.
    pub account_dir: PathBuf,
    /// This game's backup directory.
    pub dir: PathBuf,
    /// The installed client build, from `.build.info`.
    pub build: Option<String>,
    /// How many backups to keep. A restore never prunes, so its safety copy
    /// can briefly make it one more.
    pub keep: usize,
}

impl Plan {
    pub fn for_product(layout: &Layout, dirs: &AppDirs, code: &ProductCode, keep: usize) -> Self {
        Self {
            account_dir: layout.wtf_dir(code).join("Account"),
            dir: dirs.data_dir().join("backups").join(code.as_str()),
            build: installed_build(layout, code),
            keep,
        }
    }

    /// Every readable backup, newest first. Files that are not one are left
    /// alone rather than reported: the directory is ours, but a stray file in
    /// it should not hide the backups beside it.
    pub fn list(&self) -> Result<Vec<Backup>, BackupError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(BackupError::io(&self.dir, source)),
        };
        let mut backups: Vec<Backup> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_str().is_some_and(|p| p.ends_with(EXTENSION)))
            .filter_map(|path| read_backup(&path).ok())
            .collect();
        backups.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.path.cmp(&a.path)));
        Ok(backups)
    }

    /// Take a backup unless one on this build already holds exactly this.
    /// `None` means there was nothing to do: no `Account` folder yet, or no
    /// change.
    pub fn create(&self, reason: Reason) -> Result<Option<Backup>, BackupError> {
        let backup = self.snapshot(reason)?;
        if backup.is_some() {
            self.prune()?;
        }
        Ok(backup)
    }

    /// The backup before a launch: taken only when no backup exists on the
    /// installed build, which is the first launch after a patch.
    pub fn before_launch(&self) -> Result<Option<Backup>, BackupError> {
        let newest = self.list()?.into_iter().next();
        if newest.is_some_and(|b| b.build == self.build) {
            return Ok(None);
        }
        self.create(Reason::NewBuild)
    }

    /// Replace `Account` with what `backup` holds. The client must be closed.
    ///
    /// The archive is unpacked before anything is touched, so a damaged one
    /// fails with the current folder intact. The current state is then backed
    /// up, and the swap is two renames on one filesystem. Returns that safety
    /// backup, or `None` when it was already identical to an existing one.
    pub fn restore(&self, backup: &Backup) -> Result<Option<Backup>, BackupError> {
        let wtf = self.account_dir.parent().unwrap_or(Path::new("."));
        let staging = wtf.join(STAGING);
        let replaced = wtf.join(REPLACED);
        // Leftovers from a restore that was interrupted. Neither is ever the
        // live folder: that one is only ever called `Account`.
        remove_if_present(&staging)?;
        remove_if_present(&replaced)?;

        unpack(&backup.path, &staging)?;
        let fresh = staging.join(ROOT);
        // A backup of an empty folder has no members under it.
        fs::create_dir_all(&fresh).map_err(|e| BackupError::io(&fresh, e))?;

        let safety = self.snapshot(Reason::BeforeRestore)?;

        let had_current = self.account_dir.exists();
        if had_current {
            fs::rename(&self.account_dir, &replaced)
                .map_err(|e| BackupError::io(&self.account_dir, e))?;
        }
        if let Err(e) = fs::rename(&fresh, &self.account_dir) {
            if had_current {
                let _ = fs::rename(&replaced, &self.account_dir);
            }
            return Err(BackupError::io(&self.account_dir, e));
        }
        // The restore has happened; failing to tidy up after it is not worth
        // reporting as a failed restore, and the next one clears both.
        let _ = fs::remove_dir_all(&replaced);
        let _ = fs::remove_dir_all(&staging);
        Ok(safety)
    }

    /// Delete one backup, the player's own decision.
    ///
    /// Only a file this game's backup directory lists is removed: `backup`
    /// is matched against [`Plan::list`] rather than trusted, so a stale or
    /// foreign path is refused instead of deleted.
    pub fn delete(&self, backup: &Backup) -> Result<(), BackupError> {
        if !self.list()?.iter().any(|b| b.path == backup.path) {
            return Err(BackupError::NotABackup(backup.path.clone()));
        }
        fs::remove_file(&backup.path).map_err(|e| BackupError::io(&backup.path, e))
    }

    /// Write a backup without pruning, so a restore cannot rotate out the
    /// backup it is restoring.
    fn snapshot(&self, reason: Reason) -> Result<Option<Backup>, BackupError> {
        // Never launched, so nothing to lose yet.
        if !self.account_dir.is_dir() {
            return Ok(None);
        }
        let files = walk(&self.account_dir)?;
        let fingerprint = fingerprint(&files);

        // Any backup on this build, not only the newest: after a restore the
        // newest is the safety copy of what was replaced, and the restored
        // folder is the older one. Matching only the newest wrote it again,
        // and rotation then took out a real backup to make room.
        let unchanged = self
            .list()?
            .into_iter()
            .any(|b| b.build == self.build && b.fingerprint == fingerprint);
        if unchanged {
            return Ok(None);
        }

        let meta = Meta { created: now(), reason, build: self.build.clone(), fingerprint };
        let path = self.write(&files, &meta)?;
        read_backup(&path).map(Some)
    }

    fn write(&self, files: &[File], meta: &Meta) -> Result<PathBuf, BackupError> {
        let dir = &self.dir;
        fs::create_dir_all(dir).map_err(|e| BackupError::io(dir, e))?;
        // Created 0600 and left that way: these are one player's settings.
        let tmp = tempfile::Builder::new()
            .prefix(".partial-")
            .tempfile_in(dir)
            .map_err(|e| BackupError::io(dir, e))?;
        let io = |e| BackupError::io(tmp.path(), e);

        let encoder = zstd::Encoder::new(tmp.as_file(), LEVEL).map_err(io)?;
        let mut tar = tar::Builder::new(encoder);

        let text = toml::to_string(meta).expect("backup metadata serializes");
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(meta.created / 1000);
        header.set_cksum();
        tar.append_data(&mut header, META, text.as_bytes()).map_err(io)?;

        for file in files {
            let source = self.account_dir.join(&file.rel);
            tar.append_path_with_name(&source, Path::new(ROOT).join(&file.rel))
                .map_err(|e| BackupError::io(&source, e))?;
        }
        tar.into_inner().map_err(io)?.finish().map_err(io)?;
        tmp.as_file().sync_all().map_err(io)?;

        // The fingerprint keeps two backups taken in one millisecond from
        // landing on one name; the rename would silently replace the first.
        let short = &meta.fingerprint[..8];
        let path = dir.join(format!("{}-{}-{short}{EXTENSION}", meta.created, meta.reason.slug()));
        tmp.persist(&path).map_err(|e| BackupError::io(&path, e.error))?;
        // Durability of the rename, as `BuildInfo::save` does it.
        if let Ok(handle) = fs::File::open(dir) {
            let _ = handle.sync_all();
        }
        Ok(path)
    }

    /// Keep the newest `keep`, and clear anything a crash left half written.
    fn prune(&self) -> Result<(), BackupError> {
        for old in self.list()?.iter().skip(self.keep.max(1)) {
            fs::remove_file(&old.path).map_err(|e| BackupError::io(&old.path, e))?;
        }
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.filter_map(Result::ok) {
                if entry.file_name().to_string_lossy().starts_with(".partial-") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(())
    }
}

/// The build `.build.info` records for this product.
pub fn installed_build(layout: &Layout, code: &ProductCode) -> Option<String> {
    let info = BuildInfo::load(layout.build_info(code)).ok()?;
    info.get(code, COL_VERSION).ok().filter(|v| !v.is_empty()).map(str::to_owned)
}

/// One regular file under the folder being backed up.
struct File {
    rel: PathBuf,
    len: u64,
    mtime: u128,
}

/// Every regular file under `root`, sorted. Symlinks are not followed and not
/// kept: the client never writes one there, and following one could archive
/// something far outside the game.
fn walk(root: &Path) -> Result<Vec<File>, BackupError> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        let dir = root.join(&rel);
        for entry in fs::read_dir(&dir).map_err(|e| BackupError::io(&dir, e))? {
            let entry = entry.map_err(|e| BackupError::io(&dir, e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| BackupError::io(&path, e))?;
            let rel = rel.join(entry.file_name());
            if meta.is_dir() {
                pending.push(rel);
            } else if meta.is_file() {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    // Whole seconds, what a tar header keeps: at nanoseconds
                    // a restored folder never matched the backup it came from.
                    .map_or(0, |d| u128::from(d.as_secs()));
                files.push(File { rel, len: meta.len(), mtime });
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

/// What the folder looks like, without reading it: names, sizes and
/// modification times. The client rewrites a SavedVariables file whenever it
/// saves one, so a change that leaves all three alone is not a change that
/// happened. Restoring keeps modification times to the second, and that is
/// the resolution kept here, so a restored folder fingerprints the same as
/// the backup it came from. The cost is that two writes of one file within a
/// second, at the same size, look like one; the client writes SavedVariables
/// once, on exit, so that is not a case a real session produces.
fn fingerprint(files: &[File]) -> String {
    let mut hash = Sha256::new();
    for file in files {
        hash.update(file.rel.as_os_str().as_encoded_bytes());
        hash.update([0]);
        hash.update(file.len.to_le_bytes());
        hash.update(file.mtime.to_le_bytes());
    }
    hash.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn read_backup(path: &Path) -> Result<Backup, BackupError> {
    let not_ours = || BackupError::NotABackup(path.to_path_buf());
    let file = fs::File::open(path).map_err(|e| BackupError::io(path, e))?;
    let size = file.metadata().map_err(|e| BackupError::io(path, e))?.len();

    let mut archive = tar::Archive::new(zstd::Decoder::new(file).map_err(|e| BackupError::io(path, e))?);
    let mut entries = archive.entries().map_err(|_| not_ours())?;
    let mut first = entries.next().ok_or_else(not_ours)?.map_err(|_| not_ours())?;
    if first.path().ok().as_deref() != Some(Path::new(META)) {
        return Err(not_ours());
    }
    let mut text = String::new();
    first.read_to_string(&mut text).map_err(|_| not_ours())?;
    let meta: Meta = toml::from_str(&text).map_err(|_| not_ours())?;

    Ok(Backup {
        path: path.to_path_buf(),
        created: meta.created,
        reason: meta.reason,
        build: meta.build,
        size,
        fingerprint: meta.fingerprint,
    })
}

/// Extract under `dest`, refusing anything that is not a plain file or folder
/// inside `Account`. Our own archives never hold anything else, so one that
/// does was not written by us and is not trusted.
fn unpack(archive_path: &Path, dest: &Path) -> Result<(), BackupError> {
    let io = |e| BackupError::io(archive_path, e);
    let file = fs::File::open(archive_path).map_err(io)?;
    let mut archive = tar::Archive::new(zstd::Decoder::new(file).map_err(io)?);
    fs::create_dir_all(dest).map_err(|e| BackupError::io(dest, e))?;

    for entry in archive.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        let path = entry.path().map_err(io)?.into_owned();
        if path == Path::new(META) {
            continue;
        }
        let kind = entry.header().entry_type();
        let inside = path.starts_with(ROOT)
            && path.components().all(|c| matches!(c, Component::Normal(_)));
        if !inside || !(kind.is_file() || kind.is_dir()) {
            return Err(BackupError::UnsafeEntry { archive: archive_path.to_path_buf(), entry: path });
        }
        entry.unpack_in(dest).map_err(|e| BackupError::io(dest, e))?;
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), BackupError> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(BackupError::io(path, e)),
        _ => Ok(()),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{0} is not a Lightforge backup")]
    NotABackup(PathBuf),

    #[error("{archive} holds {entry}, which a backup never contains")]
    UnsafeEntry { archive: PathBuf, entry: PathBuf },
}

impl BackupError {
    fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(root: &Path) -> Plan {
        Plan {
            account_dir: root.join("WTF").join("Account"),
            dir: root.join("backups"),
            build: Some("12.0.0.1".to_owned()),
            keep: 3,
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn nothing_to_back_up_without_an_account_folder() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(plan(root.path()).create(Reason::Manual).unwrap(), None);
    }

    #[test]
    fn an_unchanged_folder_is_backed_up_once_per_build() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = plan(root.path());
        write(&plan.account_dir.join("A/SavedVariables/X.lua"), "X = 1");

        assert!(plan.create(Reason::Manual).unwrap().is_some());
        assert_eq!(plan.create(Reason::AddonUpdate).unwrap(), None);

        // The same bytes on a new build are still worth a backup: that is the
        // snapshot from before the new client first touched them.
        plan.build = Some("12.0.0.2".to_owned());
        assert!(plan.before_launch().unwrap().is_some());
        assert_eq!(plan.before_launch().unwrap(), None);
    }

    #[test]
    fn keeps_only_the_newest() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = plan(root.path());
        let file = plan.account_dir.join("A/SavedVariables/X.lua");
        for i in 0..5 {
            write(&file, &"x".repeat(i + 1));
            // Distinct builds give distinct names within one second.
            plan.build = Some(format!("12.0.0.{i}"));
            plan.create(Reason::Manual).unwrap().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let left = plan.list().unwrap();
        assert_eq!(left.len(), 3);
        assert_eq!(left[0].build.as_deref(), Some("12.0.0.4"));
    }

    #[test]
    fn delete_removes_only_a_listed_backup() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        write(&plan.account_dir.join("A/SavedVariables/X.lua"), "X = 1");
        let taken = plan.create(Reason::Manual).unwrap().unwrap();

        // A file outside the backup directory is never removed, whatever it
        // is called.
        let elsewhere = root.path().join("elsewhere.tar.zst");
        write(&elsewhere, "not ours");
        let foreign = Backup { path: elsewhere.clone(), ..taken.clone() };
        assert!(plan.delete(&foreign).is_err());
        assert!(elsewhere.exists());

        plan.delete(&taken).unwrap();
        assert!(plan.list().unwrap().is_empty());
        // The settings themselves are untouched.
        assert!(plan.account_dir.join("A/SavedVariables/X.lua").exists());
    }

    #[test]
    fn restore_brings_the_folder_back_and_keeps_what_it_replaced() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        let saved = plan.account_dir.join("A/SavedVariables/X.lua");
        let extra = plan.account_dir.join("A/macros-cache.txt");
        write(&saved, "X = 1");
        let before = plan.create(Reason::Manual).unwrap().unwrap();

        write(&saved, "X = 2, migrated");
        write(&extra, "new");
        let safety = plan.restore(&before).unwrap().expect("the changed state is kept");

        assert_eq!(fs::read_to_string(&saved).unwrap(), "X = 1");
        assert!(!extra.exists());
        assert_eq!(safety.reason, Reason::BeforeRestore);
        assert!(!plan.account_dir.with_file_name(STAGING).exists());
        assert!(!plan.account_dir.with_file_name(REPLACED).exists());

        // And the restore is itself undoable.
        plan.restore(&safety).unwrap();
        assert_eq!(fs::read_to_string(&saved).unwrap(), "X = 2, migrated");
    }

    #[test]
    fn a_restored_folder_is_not_backed_up_again() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        let saved = plan.account_dir.join("A/SavedVariables/X.lua");
        write(&saved, "X = 1");
        let before = plan.create(Reason::Manual).unwrap().unwrap();
        // A different size: within the same second, size is all that tells
        // two versions apart, see `fingerprint`.
        write(&saved, "X = 22");
        plan.restore(&before).unwrap();

        // The pass after every start would otherwise write a copy of
        // `before`, and with `keep` reached, rotate a real backup out for it.
        assert_eq!(plan.create(Reason::AddonUpdate).unwrap(), None);
        assert_eq!(plan.list().unwrap().len(), 2);
    }

    #[test]
    fn a_damaged_backup_leaves_the_folder_alone() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        let saved = plan.account_dir.join("A/SavedVariables/X.lua");
        write(&saved, "X = 1");
        let backup = plan.create(Reason::Manual).unwrap().unwrap();

        let bytes = fs::read(&backup.path).unwrap();
        fs::write(&backup.path, &bytes[..bytes.len() / 2]).unwrap();
        write(&saved, "X = 2");

        assert!(plan.restore(&backup).is_err());
        assert_eq!(fs::read_to_string(&saved).unwrap(), "X = 2");
    }

    #[test]
    fn stray_files_are_not_listed() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        write(&plan.dir.join("notes.tar.zst"), "not an archive");
        assert!(plan.list().unwrap().is_empty());
    }
}
