//! Rewriting archives that are mostly dead space.
//!
//! Versions share archives, so dropping one version's entries empties almost
//! none: on a real store, retail's 72 GiB sat in 129 archives the Classic
//! beta also used, and reclaiming it freed 0.19 GiB.
//!
//! Compaction copies each worthwhile archive's live files into a fresh one,
//! publishes the indices, then deletes the old archive. A crash at any point
//! leaves a readable store with unreachable bytes the next pass deletes.
//!
//! What is never rewritten:
//!
//! * An archive holding a key indexed twice (a headerless alias, see
//!   [`super::storage`]): which copy to move would be a guess.
//! * The rest of an archive once a header fails to verify; that is repair's job.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::idx::{Entry, ARCHIVE_RESERVED};
use super::storage::{archive_name, Storage, StorageError};
use super::verify::{Snapshot, VerifyError};
use super::writer::{StoreLock, StoreWriter, WriteError};

/// Publish the indices and delete what has been emptied at least this often,
/// which is also roughly the extra disk space a pass needs at its peak.
const CHECKPOINT_BYTES: u64 = 1 << 30;

#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("cannot remove {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}


/// How one archive's bytes are spent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// The file's length on disk.
    pub size: u64,
    /// Bytes of files the indices still point at.
    pub live: u64,
    /// Holds a key indexed more than once, so it is never rewritten.
    pub tangled: bool,
}

impl Usage {
    pub fn dead(&self) -> u64 {
        self.size.saturating_sub(self.live + ARCHIVE_RESERVED as u64)
    }

    /// Whether the space it would free is worth the copy: a quarter or more.
    pub fn worth_rewriting(&self) -> bool {
        let files = self.size.saturating_sub(ARCHIVE_RESERVED as u64);
        !self.tangled && (self.live == 0 || self.dead() * 4 >= files)
    }
}

/// What a compaction would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compaction {
    /// Archives to rewrite, or delete outright when nothing lives in them.
    pub archives: Vec<u16>,
    /// Bytes of live files that would be copied.
    pub moved: u64,
    /// Bytes of disk it would free.
    pub freed: u64,
    /// Dead bytes left in archives not worth rewriting, or tangled.
    pub left: u64,
}

impl Compaction {
    /// Choose from `usage`, considering only the archives `consider` allows.
    pub fn choose(usage: &BTreeMap<u16, Usage>, mut consider: impl FnMut(u16) -> bool) -> Self {
        let mut out = Self::default();
        for (&archive, use_) in usage.iter().filter(|(a, _)| consider(**a)) {
            match use_.worth_rewriting() {
                true => {
                    out.archives.push(archive);
                    out.moved += use_.live;
                    out.freed += use_.size.saturating_sub(use_.live);
                }
                false => out.left += use_.dead(),
            }
        }
        out
    }
}

/// How every archive in `data_dir` is used, counting only `live`; `gone` are
/// entries about to leave. An archive nothing points into (a crash leftover)
/// is included; one missing from disk is sized from its entries.
pub fn usage<'a>(
    data_dir: &Path,
    live: impl Iterator<Item = &'a Entry>,
    gone: impl Iterator<Item = &'a Entry>,
) -> BTreeMap<u16, Usage> {
    let mut map: BTreeMap<u16, Usage> = BTreeMap::new();
    let mut gone_bytes: BTreeMap<u16, u64> = BTreeMap::new();
    for entry in gone {
        *gone_bytes.entry(entry.archive_id).or_default() += u64::from(entry.size);
        map.entry(entry.archive_id).or_default();
    }
    let mut seen: HashMap<[u8; 9], u16> = HashMap::new();
    let mut tangled = BTreeSet::new();
    for entry in live {
        map.entry(entry.archive_id).or_default().live += u64::from(entry.size);
        if let Some(first) = seen.insert(entry.key, entry.archive_id) {
            tangled.extend([first, entry.archive_id]);
        }
    }
    for archive in on_disk(data_dir) {
        map.entry(archive).or_default();
    }
    for (&archive, use_) in map.iter_mut() {
        use_.tangled = tangled.contains(&archive);
        use_.size = std::fs::metadata(data_dir.join(archive_name(archive)))
            .map(|m| m.len())
            .unwrap_or(use_.live + gone_bytes.get(&archive).unwrap_or(&0) + ARCHIVE_RESERVED as u64);
    }
    map
}

/// Plan a compaction of the whole store under `install`. Changes nothing.
pub fn plan(install: &Path) -> Result<(Compaction, Snapshot), CompactError> {
    plan_where(install, |_| true)
}

/// [`plan`], considering only the archives `consider` allows.
pub fn plan_where(
    install: &Path,
    consider: impl FnMut(u16) -> bool,
) -> Result<(Compaction, Snapshot), CompactError> {
    let store = Storage::open(install)?;
    let snapshot = Snapshot::take(store.data_dir())?;
    let usage = usage(store.data_dir(), store.files(), std::iter::empty());
    Ok((Compaction::choose(&usage, consider), snapshot))
}

/// Carry out `compaction` against the store under `install`, if it is still
/// the store `snapshot` describes. Returns the bytes freed.
///
/// `progress` hears the bytes copied. Never against a live client.
pub fn execute(
    install: &Path,
    compaction: &Compaction,
    snapshot: &Snapshot,
    progress: impl FnMut(u64),
) -> Result<u64, CompactError> {
    let data_dir = super::storage::resolve_data_dir(install);
    // Taken before the comparison, so no writer slips in after it.
    let lock = StoreLock::acquire(&data_dir)?;
    if Snapshot::take(&data_dir)? != *snapshot {
        return Err(VerifyError::StoreChanged.into());
    }
    rewrite(install, &compaction.archives, progress, lock)
}

/// The unchecked half of [`execute`], for a caller that just changed the
/// store itself. Works from the indices as they are now. Returns the bytes
/// freed net of the copies.
pub(crate) fn rewrite(
    install: &Path,
    archives: &[u16],
    mut progress: impl FnMut(u64),
    lock: StoreLock,
) -> Result<u64, CompactError> {
    if archives.is_empty() {
        return Ok(0);
    }
    let mut store = Storage::open(install)?;
    let data_dir = store.data_dir().to_path_buf();
    let wanted: BTreeSet<u16> = archives.iter().copied().collect();

    let current = usage(&data_dir, store.files(), std::iter::empty());
    let mut files: BTreeMap<u16, Vec<Entry>> = BTreeMap::new();
    for entry in store.files().filter(|e| wanted.contains(&e.archive_id)) {
        files.entry(entry.archive_id).or_default().push(*entry);
    }

    let mut writer = StoreWriter::create_locked(&data_dir, lock)?.filling_gaps();
    let mut pending = Vec::new();
    let (mut copied, mut since_checkpoint, mut deleted) = (0u64, 0u64, 0u64);

    for &archive in &wanted {
        if current.get(&archive).is_none_or(|u| u.tangled) {
            continue;
        }
        let mut entries = files.remove(&archive).unwrap_or_default();
        // In file order, so the old archive is read front to back.
        entries.sort_by_key(|e| e.offset);

        let mut whole = true;
        for entry in &entries {
            let raw = store.read_raw(entry)?;
            match writer.relocate(entry, &raw) {
                Ok(_) => {}
                Err(WriteError::BadHeader { .. }) => {
                    whole = false;
                    break;
                }
                Err(e) => return Err(e.into()),
            }
            copied += u64::from(entry.size);
            since_checkpoint += u64::from(entry.size);
            progress(copied);
        }
        store.close_archive(archive);
        if whole {
            writer.forget_archive(archive);
            pending.push(archive);
        }

        if since_checkpoint >= CHECKPOINT_BYTES {
            deleted += publish_and_delete(&mut writer, &data_dir, &mut pending)?;
            since_checkpoint = 0;
        }
    }
    writer.finish()?;
    deleted += delete(&data_dir, &mut pending)?;
    Ok(deleted.saturating_sub(copied))
}

/// Publish the copies, then delete the archives they came from; the other
/// order loses files on a crash.
fn publish_and_delete(
    writer: &mut StoreWriter,
    data_dir: &Path,
    pending: &mut Vec<u16>,
) -> Result<u64, CompactError> {
    writer.checkpoint()?;
    delete(data_dir, pending)
}

/// Delete archives the published indices no longer point into.
fn delete(data_dir: &Path, pending: &mut Vec<u16>) -> Result<u64, CompactError> {
    let mut freed = 0;
    for archive in pending.drain(..) {
        let path = data_dir.join(archive_name(archive));
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => freed += bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(CompactError::Io { path, source }),
        }
    }
    Ok(freed)
}

/// Every `data.NNN` number present in `data_dir`.
fn on_disk(data_dir: &Path) -> Vec<u16> {
    std::fs::read_dir(data_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("data.").filter(|n| n.len() == 3).and_then(|n| n.parse().ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tact::verify;

    fn blte(payload: &[u8]) -> Vec<u8> {
        blte::compress_data_single(payload.to_vec(), blte::CompressionMode::ZLib, None).unwrap()
    }

    fn ekey(n: u8) -> [u8; 16] {
        let mut k = [0u8; 16];
        k[0] = n;
        k[5] = n.wrapping_mul(13);
        k
    }

    /// 300 bytes that do not compress, so archive sizes are predictable.
    fn payload(k: u8) -> Vec<u8> {
        let mut x = u32::from(k).wrapping_mul(2_654_435_761) | 1;
        (0..300)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    /// A store of `n` files spread over several small archives.
    fn store(root: &Path, n: u8) -> PathBuf {
        let data = root.join("Data").join("data");
        let mut writer = StoreWriter::create(&data).unwrap().with_max_archive_size(4096);
        for k in 0..n {
            writer.write(&ekey(k), &blte(&payload(k))).unwrap();
        }
        writer.finish().unwrap();
        data
    }

    fn drop_keys(data: &Path, keys: impl Fn(u8) -> bool) {
        let store = Storage::open(data).unwrap();
        let gone: Vec<Entry> = store.files().filter(|e| keys(e.key[0])).copied().collect();
        verify::remove_entries(data, gone).unwrap();
    }

    #[test]
    fn mostly_dead_archives_are_rewritten_and_every_file_still_reads() {
        let root = tempfile::tempdir().unwrap();
        let data = store(root.path(), 60);
        let before = on_disk(&data).len();
        drop_keys(&data, |k| k % 3 != 0);

        let (compaction, snapshot) = plan(root.path()).unwrap();
        assert_eq!(compaction.archives.len(), before, "two thirds dead: all worth it");
        let freed = execute(root.path(), &compaction, &snapshot, |_| {}).unwrap();
        assert!(freed > 0);
        assert!(on_disk(&data).len() < before);

        let mut store = Storage::open(root.path()).unwrap();
        assert_eq!(store.files().count(), 20);
        for k in (0..60u8).filter(|k| k % 3 == 0) {
            assert_eq!(store.read(&ekey(k)).unwrap(), payload(k), "file {k}");
        }
        for entry in store.files().copied().collect::<Vec<_>>() {
            let raw = store.read_raw(&entry).unwrap();
            assert!(crate::tact::entry::Header::verify(&raw, entry.archive_id, entry.offset));
        }
        // No marker is left for an archive that is gone.
        let present: BTreeSet<u16> = on_disk(&data).into_iter().collect();
        for index in store.indices() {
            assert!(index.entries().iter().all(|e| present.contains(&e.archive_id)));
        }

        // And there is nothing left worth doing.
        let (again, _) = plan(root.path()).unwrap();
        assert!(again.archives.is_empty(), "{again:?}");
    }

    #[test]
    fn archive_numbers_freed_in_the_middle_are_reused() {
        let root = tempfile::tempdir().unwrap();
        let data = store(root.path(), 60);
        let highest = *on_disk(&data).iter().max().unwrap();
        assert!(highest >= 2, "the test needs several archives");

        // Empty archive 0 outright: deleted, nothing copied.
        let first: BTreeSet<u8> = Storage::open(root.path())
            .unwrap()
            .files()
            .filter(|e| e.archive_id == 0)
            .map(|e| e.key[0])
            .collect();
        drop_keys(&data, |k| first.contains(&k));
        let (compaction, snapshot) = plan(root.path()).unwrap();
        assert_eq!(compaction.archives, vec![0]);
        execute(root.path(), &compaction, &snapshot, |_| {}).unwrap();
        assert!(!data.join(archive_name(0)).exists());

        // The next pass copies into the hole, not past the top.
        drop_keys(&data, |k| k % 2 == 0);
        let (compaction, snapshot) = plan(root.path()).unwrap();
        execute(root.path(), &compaction, &snapshot, |_| {}).unwrap();
        assert!(data.join(archive_name(0)).exists(), "{:?}", on_disk(&data));
        assert!(on_disk(&data).iter().all(|&a| a <= highest), "{:?}", on_disk(&data));
    }

    #[test]
    fn a_tangled_archive_is_left_alone() {
        let files = [
            Entry { key: [1; 9], archive_id: 0, offset: 480, size: 100 },
            Entry { key: [1; 9], archive_id: 0, offset: 510, size: 70 },
            Entry { key: [2; 9], archive_id: 1, offset: 480, size: 10 },
        ];
        let dir = tempfile::tempdir().unwrap();
        let mut usage = usage(dir.path(), files.iter(), std::iter::empty());
        usage.get_mut(&0).unwrap().size = 10_000;
        usage.get_mut(&1).unwrap().size = 10_000;
        let compaction = Compaction::choose(&usage, |_| true);
        assert_eq!(compaction.archives, vec![1]);
        assert!(compaction.left > 0);
    }

    #[test]
    fn an_archive_nothing_points_into_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let data = store(root.path(), 10);
        std::fs::write(data.join(archive_name(400)), vec![0u8; 5000]).unwrap();

        let (compaction, snapshot) = plan(root.path()).unwrap();
        assert!(compaction.archives.contains(&400));
        execute(root.path(), &compaction, &snapshot, |_| {}).unwrap();
        assert!(!data.join(archive_name(400)).exists());
        assert_eq!(Storage::open(root.path()).unwrap().files().count(), 10);
    }

    #[test]
    fn a_damaged_header_keeps_its_archive() {
        let root = tempfile::tempdir().unwrap();
        let data = store(root.path(), 12);
        drop_keys(&data, |k| k % 2 == 0);

        // Break the header of one surviving file in place.
        let store = Storage::open(root.path()).unwrap();
        let victim = *store.files().next().unwrap();
        drop(store);
        let path = data.join(archive_name(victim.archive_id));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[victim.offset as usize + 0x1b] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();

        let (compaction, snapshot) = plan(root.path()).unwrap();
        execute(root.path(), &compaction, &snapshot, |_| {}).unwrap();
        assert!(path.exists());
        let store = Storage::open(root.path()).unwrap();
        assert_eq!(store.files().count(), 6);
        assert_eq!(store.locate(&ekey(victim.key[0])).unwrap().archive_id, victim.archive_id);
    }

    #[test]
    fn a_store_that_changed_since_the_plan_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let data = store(root.path(), 10);
        drop_keys(&data, |k| k < 8);
        let (compaction, snapshot) = plan(root.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        drop_keys(&data, |k| k == 8);
        assert!(matches!(
            execute(root.path(), &compaction, &snapshot, |_| {}),
            Err(CompactError::Verify(VerifyError::StoreChanged))
        ));
    }
}
