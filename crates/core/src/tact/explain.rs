//! Why a plan is the size it is.
//!
//! A plan's headline numbers — wanted, already present, still to fetch — say
//! *how much* without saying *what*, and when the answer looks wrong there is
//! nothing to argue with. This breaks the same numbers down four ways, each
//! chosen to separate one wrong explanation from another:
//!
//! * **by priority**, because the download manifest's priority is what
//!   separates the files a client needs to start from the ones it can live
//!   without — a shortfall concentrated in the high numbers is optional
//!   content nobody fetched, not a broken install;
//! * **by bucket**, because the store is 16 hash-keyed indices and a single
//!   unreadable one would hide about a sixteenth of the store uniformly;
//! * **by where the missing keys can be fetched from**, which separates "not
//!   in the store" from "in no archive index either", a broken plan;
//! * **against [`Storage::contains`]**, which is the cross-check: the planner
//!   probes a set of truncated keys it built once, and this asks the store
//!   itself about every key that probe called missing. A disagreement means
//!   the probe is wrong and the files were there all along.
//!
//! Read-only. Nothing here writes to the store.

use std::collections::BTreeMap;

use super::archive::FileIndex;
use super::idx::Entry;
use super::indices::Locations;
use super::manifest::{DownloadEntry, DownloadManifest, Tag, TagSet};
use super::storage::{EKey, Storage};

/// Files and bytes, the pair every row of the breakdown carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub files: u64,
    pub bytes: u64,
}

impl Tally {
    fn add(&mut self, size: u64) {
        self.files += 1;
        self.bytes += size;
    }
}

/// One priority level's split between what the store has and what it does not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Split {
    pub present: Tally,
    pub missing: Tally,
}

/// Where the missing keys could be fetched from, if anywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sources {
    /// Locatable in an archive index — the normal case, fetched by range.
    pub archived: u64,
    /// In the file index, fetched whole.
    pub loose: u64,
    /// In neither. Nothing can fetch these; the plan is incomplete.
    pub nowhere: u64,
}

/// What the store and the plan disagree about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Explanation {
    pub wanted: Tally,
    pub present: Tally,
    pub missing: Tally,
    /// Keyed by the download manifest's priority; lower is fetched sooner.
    pub by_priority: BTreeMap<i8, Split>,
    /// Missing keys per store bucket, 0-15. Flat is healthy; one bucket
    /// carrying all of them is an index that did not load.
    pub missing_by_bucket: BTreeMap<u8, u64>,
    /// Entries the store's own indices list, per bucket, for comparison.
    pub store_by_bucket: BTreeMap<u8, u64>,
    pub sources: Sources,
    /// Keys the planner called missing that [`Storage::contains`] finds
    /// anyway. Always zero unless the planner's probe is broken — which is
    /// exactly the thing worth knowing.
    pub false_missing: u64,
    /// A few of those, for a report that has to name names.
    pub examples: Vec<EKey>,
}

/// How many disagreeing keys to keep for the report.
const EXAMPLES: usize = 8;

/// Break `wanted` down against the store that would satisfy it.
///
/// `already_have` is the planner's own probe, passed in rather than
/// reimplemented: the point is to reproduce its answer exactly and then
/// contradict it with the store where the two disagree.
pub fn explain(
    wanted: &[DownloadEntry],
    store: Option<&Storage>,
    locations: &Locations,
    loose: &FileIndex,
    already_have: impl Fn(&EKey) -> bool,
) -> Explanation {
    let mut out = Explanation::default();

    if let Some(store) = store {
        for index in store.indices() {
            *out.store_by_bucket.entry(index.bucket()).or_default() +=
                index.files().count() as u64;
        }
    }

    for entry in wanted {
        out.wanted.add(entry.size);
        let split = out.by_priority.entry(entry.priority).or_default();

        if already_have(&entry.ekey) {
            out.present.add(entry.size);
            split.present.add(entry.size);
            continue;
        }

        out.missing.add(entry.size);
        split.missing.add(entry.size);
        *out.missing_by_bucket.entry(bucket_of(&entry.ekey)).or_default() += 1;

        if locations.get(&entry.ekey).is_some() {
            out.sources.archived += 1;
        } else if loose.lookup(&entry.ekey).is_some() {
            out.sources.loose += 1;
        } else {
            out.sources.nowhere += 1;
        }

        // The cross-check. `already_have` said no; ask the store directly.
        if store.is_some_and(|s| s.contains(&entry.ekey)) {
            out.false_missing += 1;
            if out.examples.len() < EXAMPLES {
                out.examples.push(entry.ekey);
            }
        }
    }

    out
}


/// One tag, and how much of the selection it accounts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    pub name: String,
    /// 1 platform, 2 architecture, 3 locale, 4 region, 5 speech/text.
    pub kind: u16,
    /// Whether the tag expression names this tag, and so constrains on it.
    pub constrained: bool,
    /// Whether this tag now excludes the entries it covers: optional content
    /// the expression is silent about and nobody opted in to.
    pub gated: bool,
    pub selected: Tally,
    pub missing: Tally,
}

/// Which tag categories the selection actually constrains.
///
/// The blind spot this exists to close: [`TagSet`] drops names the manifest
/// does not define, and the selection rule only requires the tags that are
/// left. A category the expression says *nothing* about is therefore not
/// narrowed at all — every value of it passes — and nothing in the plan's
/// numbers shows that happening. A build that introduces a new category is
/// exactly the case where a launcher quietly starts wanting several times
/// what it should.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagReport {
    pub rows: Vec<TagRow>,
    /// Categories present in the manifest that the expression never names.
    pub unconstrained_kinds: Vec<u16>,
}

/// Attribute the selection, and the shortfall, to the tags that produced them.
pub fn by_tag(
    manifest: &DownloadManifest,
    tags: &TagSet,
    already_have: impl Fn(&EKey) -> bool,
) -> TagReport {
    let named: Vec<&str> = tags.groups().flatten().map(String::as_str).collect();
    let defined: Vec<&Tag> = manifest.tags().iter().collect();
    let selection = tags.against(manifest.tags());
    let gated = selection.gated();

    let mut rows: Vec<TagRow> = defined
        .iter()
        .map(|tag| TagRow {
            name: tag.name.clone(),
            kind: tag.kind,
            constrained: named.contains(&tag.name.as_str()),
            gated: gated.contains(&tag.name.as_str()),
            selected: Tally::default(),
            missing: Tally::default(),
        })
        .collect();

    // Walked by index, because a tag's mask addresses manifest positions —
    // the selected list has been sorted by then and cannot be indexed back.
    let selected = manifest.selected_indices(tags);
    for index in selected {
        let entry = &manifest.entries()[index];
        let absent = !already_have(&entry.ekey);
        for (row, tag) in rows.iter_mut().zip(&defined) {
            if tag.covers(index) {
                row.selected.add(entry.size);
                if absent {
                    row.missing.add(entry.size);
                }
            }
        }
    }

    let mut unconstrained_kinds: Vec<u16> = defined
        .iter()
        .filter(|tag| gated.contains(&tag.name.as_str()))
        .map(|tag| tag.kind)
        .collect();
    unconstrained_kinds.sort_unstable();
    unconstrained_kinds.dedup();

    rows.sort_by_key(|row| std::cmp::Reverse(row.missing.bytes));
    TagReport { rows, unconstrained_kinds }
}

/// Which store bucket a key would live in.
fn bucket_of(ekey: &EKey) -> u8 {
    let key: [u8; 9] = ekey[..9].try_into().expect("9 of 16 bytes");
    Entry { key, archive_id: 0, offset: 0, size: 0 }.bucket()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> EKey {
        let mut k = [0u8; 16];
        k[0] = n;
        k[15] = n;
        k
    }

    /// An empty-but-valid file index: nothing here is testing the file index's
    /// own parsing, only that a key it does not list is reported as such.
    fn no_loose() -> FileIndex {
        let (block_size, entry_size) = (4096usize, 20usize);
        let mut out = vec![0u8; block_size];
        out[..16].copy_from_slice(&key(200));
        out[16..20].copy_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&key(200));
        out.extend_from_slice(&[0u8; 8]);
        let mut footer = vec![0u8; 8];
        footer.extend_from_slice(&[1, 0, 0, (block_size / 1024) as u8, 0, 4, 16, 8]);
        footer.extend_from_slice(&1u32.to_le_bytes());
        footer.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&footer);
        let _ = entry_size;
        FileIndex::parse(&out).unwrap()
    }

    fn want(entries: &[(u8, i8, u64)]) -> Vec<DownloadEntry> {
        entries
            .iter()
            .map(|(n, priority, size)| DownloadEntry {
                ekey: key(*n),
                size: *size,
                priority: *priority,
            })
            .collect()
    }

    #[test]
    fn the_split_totals_match_the_wanted_set() {
        let wanted = want(&[(1, 0, 100), (2, 0, 200), (3, 1, 400)]);
        let have = |k: &EKey| *k == key(1);
        let e = explain(&wanted, None, &Locations::default(), &no_loose(), have);

        assert_eq!(e.wanted, Tally { files: 3, bytes: 700 });
        assert_eq!(e.present, Tally { files: 1, bytes: 100 });
        assert_eq!(e.missing, Tally { files: 2, bytes: 600 });
    }

    #[test]
    fn priorities_separate_what_a_client_needs_from_what_it_does_not() {
        // The distinction the whole report exists to make: a shortfall that
        // sits entirely in one priority is a different bug from one spread
        // across all of them.
        let wanted = want(&[(1, 0, 100), (2, 2, 900), (3, 2, 900)]);
        let have = |k: &EKey| *k == key(1);
        let e = explain(&wanted, None, &Locations::default(), &no_loose(), have);

        assert_eq!(e.by_priority[&0].present, Tally { files: 1, bytes: 100 });
        assert_eq!(e.by_priority[&0].missing, Tally::default());
        assert_eq!(e.by_priority[&2].missing, Tally { files: 2, bytes: 1800 });
    }

    #[test]
    fn a_key_nothing_can_fetch_is_counted_apart_from_one_that_is_merely_absent() {
        let wanted = want(&[(1, 0, 100)]);
        let e = explain(
            &wanted,
            None,
            &Locations::default(),
            &no_loose(),
            |_| false,
        );
        assert_eq!(e.sources, Sources { archived: 0, loose: 0, nowhere: 1 });
    }

    #[test]
    fn missing_keys_are_attributed_to_the_bucket_that_should_hold_them() {
        let wanted = want(&[(1, 0, 100), (2, 0, 100)]);
        let e = explain(
            &wanted,
            None,
            &Locations::default(),
            &no_loose(),
            |_| false,
        );
        let total: u64 = e.missing_by_bucket.values().sum();
        assert_eq!(total, 2);
        // Each key lands in the bucket its own bytes decide, not bucket zero.
        assert_eq!(e.missing_by_bucket[&bucket_of(&key(1))], 1);
    }

    #[test]
    fn with_no_store_nothing_is_reported_as_falsely_missing() {
        // Without a store there is nothing to contradict the probe with, so
        // the cross-check must stay silent rather than accusing it.
        let wanted = want(&[(1, 0, 100)]);
        let e = explain(
            &wanted,
            None,
            &Locations::default(),
            &no_loose(),
            |_| false,
        );
        assert_eq!(e.false_missing, 0);
        assert!(e.examples.is_empty());
    }
}
