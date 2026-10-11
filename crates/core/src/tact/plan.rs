//! The fetch plan: every request the download will make, decided up front.
//!
//! A build's wanted files are scattered across ~1400 archives, and asking for
//! each one separately would mean 1.57 million range requests for a single
//! retail install. Asking for whole archives instead would mean transferring
//! 174.5 GiB where 123.0 GiB is wanted: every locale and platform, discarded
//! on arrival. Neither is acceptable, so the plan sits between them:
//!
//! 1. locate every wanted key in its archive index,
//! 2. sort by `(archive, offset)`,
//! 3. merge neighbours separated by less than [`Options::max_gap`] into one
//!    **run**, capped at [`Options::max_run`] bytes,
//! 4. fetch each run with one range request and split it locally.
//!
//! The cost of merging is the padding between files, which is transferred and
//! thrown away; the benefit is one request instead of many. The defaults are
//! measured against the real 12.1.0 install rather than guessed, see
//! [`Options`].
//!
//! Keys the file index lists are not in any archive and are fetched whole, by
//! plain GET. Keys in neither are reported as [`Plan::missing`] rather than
//! silently dropped: a wanted file nothing can locate is a broken plan, and
//! finding that out at planning time is much cheaper than at 90% of 123 GiB.

use std::collections::{HashMap, HashSet};

use super::archive::FileIndex;
use super::config::CKey;
use super::encoding::Encoding;
use super::indices::Locations;
use super::manifest::DownloadEntry;
use super::storage::EKey;

/// How aggressively neighbouring files are merged into one request.
///
/// Measured over the whole of 12.1.0, 1,574,027 wanted files, 123.0 GiB
/// across 1406 archives, so the default is a reading of the curve rather
/// than a guess:
///
/// ```text
/// max_gap   requests   transfer   wasted
///       0    102,790    123.0 GiB   0.00 GiB
///     16K     78,402    123.1 GiB   0.13 GiB
///     64K     60,717    123.8 GiB   0.75 GiB   <- default
///    128K     50,940    124.7 GiB   1.67 GiB
///    256K     41,831    126.5 GiB   3.48 GiB
///      1M     27,946    134.0 GiB  10.98 GiB
///      4M     23,710    144.5 GiB  21.47 GiB
/// ```
///
/// 64 KiB is where the curve stops paying: it buys a 41% cut in requests for
/// 0.6% more bytes, while every step past it trades roughly a gigabyte for a
/// tenth of the requests. At 60,717 requests the average is already 2.1 MB of
/// payload each, a comfortable size for HTTP, so the remaining win is small
/// and the bytes are real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Merge two files if the unwanted bytes between them are fewer than this.
    ///
    /// Zero merges only files that are exactly adjacent, already a 15x
    /// reduction, because wanted content sits densely inside an archive.
    pub max_gap: u32,
    /// Never let a run exceed this many bytes.
    ///
    /// Bounds the memory one response needs and keeps progress and
    /// cancellation granular, a paused download should not have to discard a
    /// quarter-gigabyte of transfer.
    pub max_run: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { max_gap: 64 * 1024, max_run: 8 * 1024 * 1024 }
    }
}

/// One file inside a run, and what verifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub ekey: EKey,
    /// Content key, which is what actually verifies the decoded bytes.
    ///
    /// Absent when encoding does not map this key back to a content key. That
    /// is rare but real, 2 of 12.1.0's 1,574,027 wanted files, and both of
    /// them are present in the store Agent built, so the right response is to
    /// store the file unverified rather than refuse it. Refusing would diverge
    /// from a real install over two small files.
    pub ckey: Option<CKey>,
    /// Offset of this file's bytes from the start of its run.
    pub at: u32,
    /// Length of this file's bytes, CASC entry header included.
    pub size: u32,
}

/// One range request: a contiguous stretch of one archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Hash of the archive to request from.
    pub archive: String,
    /// Offset of the run from the start of the archive.
    pub offset: u32,
    /// Bytes to request.
    pub length: u32,
    /// The wanted files inside it, in offset order.
    pub pieces: Vec<Piece>,
}

impl Run {
    /// The value of an HTTP `Range` header that fetches this run.
    pub fn range(&self) -> String {
        format!("bytes={}-{}", self.offset, u64::from(self.offset) + u64::from(self.length) - 1)
    }

    /// Bytes transferred that no wanted file needs.
    pub fn padding(&self) -> u64 {
        u64::from(self.length) - self.pieces.iter().map(|p| u64::from(p.size)).sum::<u64>()
    }

    /// One piece's bytes out of a fetched run.
    ///
    /// `None` if the response is shorter than the plan says it should be,
    /// which is the shape a truncated or ignored range request arrives in.
    pub fn slice<'a>(&self, body: &'a [u8], piece: &Piece) -> Option<&'a [u8]> {
        let start = piece.at as usize;
        let end = start.checked_add(piece.size as usize)?;
        body.get(start..end)
    }
}

/// A loose file: served whole at its own URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loose {
    pub ekey: EKey,
    pub ckey: Option<CKey>,
    pub size: u32,
}

/// Everything the download will request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub runs: Vec<Run>,
    pub loose: Vec<Loose>,
    /// Wanted, but in no archive index and not loose. Nothing can fetch these.
    pub missing: Vec<EKey>,
    /// Wanted, but already in the local store, the resume path.
    pub already_present: usize,
}

impl Plan {
    /// Build a plan for `wanted`.
    ///
    /// `already_have` is asked about every key, so an interrupted download
    /// re-plans to only what is left; pass `|_| false` for a fresh install.
    pub fn build(
        wanted: &[DownloadEntry],
        locations: &Locations,
        loose_index: &FileIndex,
        ckeys: &HashMap<EKey, CKey>,
        already_have: impl Fn(&EKey) -> bool,
        options: Options,
    ) -> Self {
        let mut plan = Self::default();
        // (archive index into `archives`, offset, size, ekey)
        let mut archived: Vec<(u32, u32, u32, EKey)> = Vec::new();
        let mut archives: Vec<String> = Vec::new();
        let mut archive_id = HashMap::<String, u32>::new();

        for entry in wanted {
            if already_have(&entry.ekey) {
                plan.already_present += 1;
                continue;
            }

            if let Some((archive, located)) = locations.get(&entry.ekey) {
                let id = *archive_id.entry(archive.to_owned()).or_insert_with(|| {
                    archives.push(archive.to_owned());
                    (archives.len() - 1) as u32
                });
                archived.push((id, located.offset, located.size, entry.ekey));
            } else if let Some(entry_loose) = loose_index.lookup(&entry.ekey) {
                plan.loose.push(Loose {
                    ekey: entry.ekey,
                    ckey: ckeys.get(&entry.ekey).copied(),
                    size: entry_loose.size,
                });
            } else {
                plan.missing.push(entry.ekey);
            }
        }

        // Sorting is what makes the merge below a single pass, and it is also
        // what makes each archive read sequentially rather than seeking.
        archived.sort_unstable();

        let mut current: Option<Run> = None;
        for (id, offset, size, ekey) in archived {
            let piece_end = u64::from(offset) + u64::from(size);

            let extends = current.as_ref().is_some_and(|run| {
                let run_end = u64::from(run.offset) + u64::from(run.length);
                archives[id as usize] == run.archive
                    // Sorted, so a start before the run's end is an overlap:
                    // two entries sharing bytes, which merging handles fine.
                    && u64::from(offset) <= run_end + u64::from(options.max_gap)
                    && piece_end - u64::from(run.offset) <= u64::from(options.max_run)
            });

            match (extends, current.as_mut()) {
                (true, Some(run)) => {
                    run.length = (piece_end - u64::from(run.offset)) as u32;
                    run.pieces.push(Piece {
                        ekey,
                        ckey: ckeys.get(&ekey).copied(),
                        at: offset - run.offset,
                        size,
                    });
                }
                _ => {
                    if let Some(done) = current.take() {
                        plan.runs.push(done);
                    }
                    current = Some(Run {
                        archive: archives[id as usize].clone(),
                        offset,
                        length: size,
                        pieces: vec![Piece { ekey, ckey: ckeys.get(&ekey).copied(), at: 0, size }],
                    });
                }
            }
        }
        if let Some(done) = current.take() {
            plan.runs.push(done);
        }

        plan
    }

    /// How many HTTP requests the plan makes.
    pub fn requests(&self) -> usize {
        self.runs.len() + self.loose.len()
    }

    /// How many files the plan fetches.
    pub fn files(&self) -> usize {
        self.runs.iter().map(|r| r.pieces.len()).sum::<usize>() + self.loose.len()
    }

    /// Bytes actually transferred, padding included.
    pub fn bytes(&self) -> u64 {
        self.runs.iter().map(|r| u64::from(r.length)).sum::<u64>()
            + self.loose.iter().map(|l| u64::from(l.size)).sum::<u64>()
    }

    /// Bytes transferred that no wanted file needs.
    pub fn padding(&self) -> u64 {
        self.runs.iter().map(Run::padding).sum()
    }

    /// Whether every wanted key could be located.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Encoding key → content key, for the keys in `wanted` only.
///
/// The encoding manifest is sorted by content key, so the reverse direction
/// has no index and `Encoding::ckey_for_ekey` is a linear scan. Doing that per
/// file would be 1.57 million scans of a 2.87 million entry manifest; one pass
/// building a map for the keys that matter is the difference between a plan
/// that builds in milliseconds and one that never finishes.
pub fn reverse_encoding(encoding: &Encoding, wanted: &HashSet<EKey>) -> HashMap<EKey, CKey> {
    let mut map = HashMap::with_capacity(wanted.len());
    for entry in encoding.entries() {
        if wanted.contains(&entry.ekey) {
            map.entry(entry.ekey).or_insert(entry.ckey);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::archive::ArchiveIndex;
    use super::super::indices::Locations;

    fn key(n: u8) -> EKey {
        let mut k = [0u8; 16];
        k[0] = n;
        k
    }

    fn index_bytes(entries: &[(EKey, u32, u32)]) -> Vec<u8> {
        let (block_size, entry_size) = (4096usize, 24usize);
        let per_block = block_size / entry_size;
        let blocks = entries.len().div_ceil(per_block).max(1);

        let mut out = vec![0u8; blocks * block_size];
        for (i, (ekey, size, offset)) in entries.iter().enumerate() {
            let at = (i / per_block) * block_size + (i % per_block) * entry_size;
            out[at..at + 16].copy_from_slice(ekey);
            out[at + 16..at + 20].copy_from_slice(&size.to_be_bytes());
            out[at + 20..at + 24].copy_from_slice(&offset.to_be_bytes());
        }
        for b in 0..blocks {
            let last = ((b + 1) * per_block - 1).min(entries.len().saturating_sub(1));
            out.extend_from_slice(&entries.get(last).map_or([0u8; 16], |e| e.0));
        }
        out.extend(std::iter::repeat_n(0u8, blocks * 8));
        let mut footer = vec![0u8; 8];
        footer.extend_from_slice(&[1, 0, 0, (block_size / 1024) as u8, 4, 4, 16, 8]);
        footer.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        footer.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&footer);
        out
    }

    fn file_index_bytes(entries: &[(EKey, u32)]) -> Vec<u8> {
        let (block_size, entry_size) = (4096usize, 20usize);
        let per_block = block_size / entry_size;
        let blocks = entries.len().div_ceil(per_block).max(1);

        let mut out = vec![0u8; blocks * block_size];
        for (i, (ekey, size)) in entries.iter().enumerate() {
            let at = (i / per_block) * block_size + (i % per_block) * entry_size;
            out[at..at + 16].copy_from_slice(ekey);
            out[at + 16..at + 20].copy_from_slice(&size.to_be_bytes());
        }
        for b in 0..blocks {
            let last = ((b + 1) * per_block - 1).min(entries.len().saturating_sub(1));
            out.extend_from_slice(&entries.get(last).map_or([0u8; 16], |e| e.0));
        }
        out.extend(std::iter::repeat_n(0u8, blocks * 8));
        let mut footer = vec![0u8; 8];
        footer.extend_from_slice(&[1, 0, 0, (block_size / 1024) as u8, 0, 4, 16, 8]);
        footer.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        footer.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&footer);
        out
    }

    fn located(archive: &str, entries: &[(EKey, u32, u32)], wanted: &[EKey]) -> Locations {
        let index = ArchiveIndex::parse(&index_bytes(entries), archive).unwrap();
        let mut locations = Locations::default();
        locations.absorb(&index, &wanted.iter().copied().collect());
        locations
    }

    fn want(keys: &[EKey]) -> Vec<DownloadEntry> {
        keys.iter().map(|k| DownloadEntry { ekey: *k, size: 0, priority: 0 }).collect()
    }

    fn no_loose() -> FileIndex {
        FileIndex::parse(&file_index_bytes(&[(key(200), 1)])).unwrap()
    }

    #[test]
    fn adjacent_files_become_one_request() {
        let keys = [key(1), key(2), key(3)];
        let locations = located("a", &[(key(1), 100, 0), (key(2), 100, 100), (key(3), 100, 200)], &keys);

        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &HashMap::new(), |_| false, Options::default());

        assert_eq!(plan.requests(), 1);
        assert_eq!(plan.files(), 3);
        assert_eq!(plan.runs[0].offset, 0);
        assert_eq!(plan.runs[0].length, 300);
        assert_eq!(plan.runs[0].padding(), 0);
        assert_eq!(plan.runs[0].range(), "bytes=0-299");
        assert_eq!(
            plan.runs[0].pieces.iter().map(|p| p.at).collect::<Vec<_>>(),
            [0, 100, 200]
        );
    }

    #[test]
    fn a_gap_wider_than_the_threshold_splits_the_run() {
        let keys = [key(1), key(2)];
        let locations = located("a", &[(key(1), 100, 0), (key(2), 100, 1_000_000)], &keys);

        let narrow = Options { max_gap: 1024, max_run: 8 << 20 };
        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &HashMap::new(), |_| false, narrow);
        assert_eq!(plan.requests(), 2);
        assert_eq!(plan.padding(), 0);

        // Widen the threshold past the gap and the two merge, paying for the
        // bytes between them.
        let wide = Options { max_gap: 2 << 20, max_run: 8 << 20 };
        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &HashMap::new(), |_| false, wide);
        assert_eq!(plan.requests(), 1);
        assert_eq!(plan.runs[0].length, 1_000_100);
        assert_eq!(plan.runs[0].padding(), 999_900);
    }

    #[test]
    fn a_run_never_exceeds_its_cap() {
        // Keys start at 1: an all-zero key is block padding, not an entry.
        let entries: Vec<(EKey, u32, u32)> =
            (1..=8u8).map(|n| (key(n), 1000, u32::from(n - 1) * 1000)).collect();
        let keys: Vec<EKey> = entries.iter().map(|e| e.0).collect();
        let locations = located("a", &entries, &keys);

        let capped = Options { max_gap: 1 << 20, max_run: 2500 };
        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &HashMap::new(), |_| false, capped);

        assert!(plan.runs.iter().all(|r| r.length <= 2500));
        assert_eq!(plan.files(), 8);
        assert_eq!(plan.bytes(), 8000);
    }

    #[test]
    fn files_in_different_archives_never_share_a_request() {
        let keys = [key(1), key(2)];
        let mut locations = located("a", &[(key(1), 100, 0)], &keys);
        let second = ArchiveIndex::parse(&index_bytes(&[(key(2), 100, 100)]), "b").unwrap();
        locations.absorb(&second, &keys.iter().copied().collect());

        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &HashMap::new(), |_| false, Options::default());

        assert_eq!(plan.requests(), 2);
        let mut archives: Vec<&str> = plan.runs.iter().map(|r| r.archive.as_str()).collect();
        archives.sort_unstable();
        assert_eq!(archives, ["a", "b"]);
    }

    #[test]
    fn a_loose_key_is_fetched_whole_and_an_unlocatable_one_is_reported() {
        let keys = [key(1), key(50), key(99)];
        let locations = located("a", &[(key(1), 100, 0)], &keys);
        let loose = FileIndex::parse(&file_index_bytes(&[(key(50), 4096)])).unwrap();

        let plan = Plan::build(&want(&keys), &locations, &loose, &HashMap::new(), |_| false, Options::default());

        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.loose, [Loose { ekey: key(50), ckey: None, size: 4096 }]);
        assert_eq!(plan.missing, [key(99)]);
        assert!(!plan.is_complete());
        assert_eq!(plan.bytes(), 100 + 4096);
    }

    #[test]
    fn keys_already_in_the_store_are_not_planned_again() {
        let keys = [key(1), key(2), key(3)];
        let locations = located("a", &[(key(1), 100, 0), (key(2), 100, 100), (key(3), 100, 200)], &keys);

        let plan = Plan::build(
            &want(&keys),
            &locations,
            &no_loose(),
            &HashMap::new(),
            |k| *k == key(2),
            Options::default(),
        );

        assert_eq!(plan.already_present, 1);
        assert_eq!(plan.files(), 2);
        // The hole left behind is bridged rather than split, since the bytes
        // in between are below the gap threshold.
        assert_eq!(plan.requests(), 1);
        assert_eq!(plan.runs[0].padding(), 100);
    }

    #[test]
    fn a_piece_slices_out_of_its_run_and_a_short_response_does_not_panic() {
        let keys = [key(1), key(2)];
        let locations = located("a", &[(key(1), 4, 0), (key(2), 4, 4)], &keys);
        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &HashMap::new(), |_| false, Options::default());

        let run = &plan.runs[0];
        let body = b"AAAABBBB";
        assert_eq!(run.slice(body, &run.pieces[0]), Some(&body[0..4]));
        assert_eq!(run.slice(body, &run.pieces[1]), Some(&body[4..8]));
        assert_eq!(run.slice(b"AAAA", &run.pieces[1]), None);
    }

    #[test]
    fn content_keys_are_attached_where_encoding_knows_them() {
        let keys = [key(1), key(2)];
        let locations = located("a", &[(key(1), 100, 0), (key(2), 100, 100)], &keys);
        let ckeys: HashMap<EKey, CKey> = [(key(1), [0xAA; 16])].into_iter().collect();

        let plan = Plan::build(&want(&keys), &locations, &no_loose(), &ckeys, |_| false, Options::default());

        let pieces = &plan.runs[0].pieces;
        assert_eq!(pieces[0].ckey, Some([0xAA; 16]));
        // Storable but not verifiable, and the plan says so rather than
        // pretending an unverified file is fine.
        assert_eq!(pieces[1].ckey, None);
    }

    #[test]
    fn an_empty_wanted_set_is_an_empty_plan() {
        let plan = Plan::build(&[], &Locations::default(), &no_loose(), &HashMap::new(), |_| false, Options::default());

        assert_eq!(plan.requests(), 0);
        assert_eq!(plan.bytes(), 0);
        assert!(plan.is_complete());
    }
}
