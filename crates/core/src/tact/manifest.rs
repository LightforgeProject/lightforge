//! Install and download manifests: *which* files a build wants here.
//!
//! Whether this machine should hold a file at all is decided here, by tag:
//!
//! - **install** (`IN`), the ~266 loose files that must exist on disk as real
//!   files for the client to start: `Wow.exe`, `wow_loader.dll`, CEF and its
//!   `.pak` resources. Listed by path and content key.
//! - **download** (`DL`): every file the CASC store should hold, ~2.87M of
//!   them for 12.1.0, listed by encoding key with a size and a priority.
//!
//! Both end in a tag section: a name, a type, and one bit per entry.
//!
//! **The bit order is MSB-first within each byte**, and getting it backwards
//! fails silently: `tact_parser::install` reads LSB-first and hands `Windows`
//! five macOS `.app` bundles. MSB-first gives `OSX` all 195 `.app` paths.
//!
//! Selection is driven by the `Tags` column of `.build.info`, e.g.
//!
//! ```text
//! Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:...enUS text?
//! ```
//!
//! Colon-separated groups are alternatives; within a group every tag must
//! match. Names the manifest does not define (`acct-AUT`, `geoip-AT`) are
//! ignored. On a real install this selects 1,574,027 files, 123.0 GiB, the
//! store Agent built.

use super::config::CKey;
use super::storage::EKey;

/// One tag and the entries it covers.
#[derive(Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Category: 1 platform, 2 architecture, 3 locale, 4 region, 5 speech/text.
    /// Only compared, never interpreted.
    pub kind: u16,
    mask: Vec<u8>,
}

impl std::fmt::Debug for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tag")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("entries", &self.count())
            .finish()
    }
}

impl Tag {
    /// A tag covering nothing, for tests that only look at names and kinds.
    #[cfg(test)]
    pub(crate) fn named(name: &str, kind: u16) -> Self {
        Self { name: name.to_owned(), kind, mask: Vec::new() }
    }

    /// Whether entry `index` carries this tag. Bit 0 of a byte is its **high**
    /// bit. Out of range is `false`: the mask's padding bits address nothing.
    pub fn covers(&self, index: usize) -> bool {
        match self.mask.get(index / 8) {
            Some(byte) => byte & (0x80 >> (index % 8)) != 0,
            None => false,
        }
    }

    /// How many entries carry this tag, padding bits included.
    fn count(&self) -> u32 {
        self.mask.iter().map(|b| b.count_ones()).sum()
    }
}

/// The tag expression from a `.build.info` `Tags` column.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagSet {
    /// Alternatives. An entry is selected if it satisfies any one of them.
    groups: Vec<Vec<String>>,
    /// Optional content the user has asked for by name, see
    /// [`TagSet::with_optional`].
    allow: Vec<String>,
}

impl TagSet {
    /// Parse the `Tags` column of `.build.info`: groups split on `:`, names on
    /// spaces, a trailing `?` stripped. Empty selects everything.
    pub fn parse(column: &str) -> Self {
        let groups = column
            .split(':')
            .map(|group| {
                group
                    .split_whitespace()
                    .map(|name| name.trim_end_matches('?').to_string())
                    .filter(|name| !name.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|group: &Vec<String>| !group.is_empty())
            .collect();
        Self { groups, allow: Vec::new() }
    }

    /// Build a set from one explicit group.
    pub fn of(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let group: Vec<String> = names.into_iter().map(Into::into).collect();
        Self {
            groups: if group.is_empty() { Vec::new() } else { vec![group] },
            allow: Vec::new(),
        }
    }

    /// Opt in to optional content by tag name, e.g. `HighRes`. A permission,
    /// not a requirement: requiring it would select only the texture pack.
    pub fn with_optional(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allow = names.into_iter().map(Into::into).collect();
        self
    }

    /// The optional content this set opts in to.
    pub fn optional(&self) -> &[String] {
        &self.allow
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn groups(&self) -> impl Iterator<Item = &[String]> {
        self.groups.iter().map(Vec::as_slice)
    }

    /// Names in this set that `tags` does not define, so selection ignores
    /// them. Surfaced: a typo looks exactly like a geoip fact.
    pub fn unknown_names<'a>(&'a self, tags: &[Tag]) -> Vec<&'a str> {
        let mut unknown: Vec<&str> = self
            .groups
            .iter()
            .flatten()
            .filter(|name| !tags.iter().any(|t| t.name == **name))
            .map(String::as_str)
            .collect();
        unknown.sort_unstable();
        unknown.dedup();
        unknown
    }

    /// Bind this expression to one manifest's tag table of `entries` entries.
    pub fn against<'a>(&self, tags: &'a [Tag], entries: usize) -> Selection<'a> {
        let groups = self
            .groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .filter_map(|name| tags.iter().position(|t| t.name == *name))
                    .collect()
            })
            .collect();

        // Naming one value of a category (`enUS`) constrains it. Tags of a
        // category named nowhere become gates: they must *not* cover an
        // entry unless opted in, or a 37 GiB texture pack slips in.
        //
        // Except a tag covering the whole manifest, which separates nothing:
        // Warcraft I Remastered tags all 746 entries `Debug` and `Release`.
        let named = |name: &str| self.groups.iter().flatten().any(|n| n == name);
        let covers_everything = |tag: &Tag| entries > 0 && (0..entries).all(|i| tag.covers(i));
        let gates = tags
            .iter()
            .enumerate()
            .filter(|(_, tag)| {
                !named(&tag.name)
                    && !self.allow.iter().any(|a| a == &tag.name)
                    && !tags.iter().any(|t| t.kind == tag.kind && named(&t.name))
                    && !covers_everything(tag)
            })
            .map(|(i, _)| i)
            .collect();

        Selection { groups, gates, tags, unconstrained: self.groups.is_empty() }
    }

    /// The optional content a manifest offers that this expression is silent
    /// about, the names a user could opt in to.
    pub fn optional_names(&self, tags: &[Tag], entries: usize) -> Vec<String> {
        let mut names: Vec<String> = self
            .against(tags, entries)
            .gates
            .iter()
            .map(|&i| tags[i].name.clone())
            .collect();
        // Opted-in names stop being gates, but are still what is on offer.
        names.extend(self.allow.iter().filter(|a| tags.iter().any(|t| t.name == **a)).cloned());
        names.sort_unstable();
        names.dedup();
        names
    }
}

/// A tag expression bound to one manifest's tag table. An entry must satisfy
/// one alternative and carry no gate.
pub struct Selection<'a> {
    /// Tag positions per alternative group.
    groups: Vec<Vec<usize>>,
    /// Tag positions that exclude an entry outright.
    gates: Vec<usize>,
    tags: &'a [Tag],
    /// An empty expression selects everything, gates included.
    unconstrained: bool,
}

impl Selection<'_> {
    pub fn selects(&self, index: usize) -> bool {
        if self.unconstrained {
            return true;
        }
        if self.gates.iter().any(|&t| self.tags[t].covers(index)) {
            return false;
        }
        self.groups
            .iter()
            .any(|group| group.iter().all(|&t| self.tags[t].covers(index)))
    }

    /// The tags acting as gates, by name.
    pub fn gated(&self) -> Vec<&str> {
        self.gates.iter().map(|&i| self.tags[i].name.as_str()).collect()
    }
}

/// One file the install manifest wants on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallEntry {
    /// Windows-style path relative to the flavor directory, e.g. `Utils\cef.pak`.
    pub path: String,
    pub ckey: CKey,
    /// Decoded size. The encoded size is whatever the CDN serves.
    pub size: u32,
}

impl InstallEntry {
    /// The path as a relative POSIX path, case as the manifest spells it.
    /// `None` for a path that climbs out with `..`: these are executables.
    /// Empty and `.` segments, a leading separator among them, are dropped.
    pub fn relative_path(&self) -> Option<std::path::PathBuf> {
        let parts: Vec<&str> =
            self.path.split(['\\', '/']).filter(|p| !p.is_empty() && *p != ".").collect();
        let safe = !parts.is_empty() && parts.iter().all(|p| *p != ".." && !p.contains('\0'));
        safe.then(|| parts.into_iter().collect())
    }
}

/// The install manifest: files that live on disk, not in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallManifest {
    tags: Vec<Tag>,
    entries: Vec<InstallEntry>,
}

impl InstallManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let mut r = Reader::new(bytes, "install manifest");
        if r.take(2)? != b"IN" {
            return Err(ManifestError::BadMagic { what: "install manifest" });
        }
        let version = r.u8()?;
        if version != 1 {
            return Err(ManifestError::UnsupportedVersion { what: "install manifest", version });
        }
        let hash_size = usize::from(r.u8()?);
        if hash_size != 16 {
            return Err(ManifestError::UnsupportedHashSize(hash_size));
        }
        let tag_count = usize::from(r.u16()?);
        let entry_count = r.u32()? as usize;

        let tags = r.tags(tag_count, entry_count)?;

        // Smallest entry: an empty path's terminator, a key and a size.
        let mut entries = Vec::with_capacity(entry_count.min(r.remaining() / (1 + 16 + 4)));
        for _ in 0..entry_count {
            let path = r.cstring()?;
            let ckey: CKey = r.take(16)?.try_into().expect("16 bytes");
            let size = r.u32()?;
            entries.push(InstallEntry { path, ckey, size });
        }

        Ok(Self { tags, entries })
    }

    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    pub fn entries(&self) -> &[InstallEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries `tags` selects, in manifest order.
    pub fn select(&self, tags: &TagSet) -> Vec<&InstallEntry> {
        let selection = tags.against(&self.tags, self.entries.len());
        self.entries
            .iter()
            .enumerate()
            .filter(|(i, _)| selection.selects(*i))
            .map(|(_, e)| e)
            .collect()
    }
}

/// One file the download manifest wants in the CASC store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadEntry {
    pub ekey: EKey,
    /// Encoded size, which is what gets transferred and stored.
    pub size: u64,
    /// Lower is fetched sooner. Already adjusted by the header's base.
    pub priority: i8,
}

/// The download manifest: everything the store should hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadManifest {
    tags: Vec<Tag>,
    entries: Vec<DownloadEntry>,
}

impl DownloadManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let mut r = Reader::new(bytes, "download manifest");
        if r.take(2)? != b"DL" {
            return Err(ManifestError::BadMagic { what: "download manifest" });
        }
        let version = r.u8()?;
        if !(1..=3).contains(&version) {
            return Err(ManifestError::UnsupportedVersion { what: "download manifest", version });
        }
        let ekey_size = usize::from(r.u8()?);
        if ekey_size != 16 {
            return Err(ManifestError::UnsupportedHashSize(ekey_size));
        }
        let has_checksum = r.u8()? != 0;
        let entry_count = r.u32()? as usize;
        let tag_count = usize::from(r.u16()?);

        // v2 adds per-entry flags, v3 a priority base and three reserved bytes.
        let flag_size = if version >= 2 { usize::from(r.u8()?) } else { 0 };
        let base_priority = if version >= 3 {
            let base = r.u8()? as i8;
            r.take(3)?;
            base
        } else {
            0
        };

        // Smallest entry: a key, a 40-bit size and a priority.
        let mut entries = Vec::with_capacity(entry_count.min(r.remaining() / (16 + 5 + 1)));
        for _ in 0..entry_count {
            let ekey: EKey = r.take(16)?.try_into().expect("16 bytes");
            let size = r.u40()?;
            // Saturating: a malformed base must not panic.
            let priority = (r.u8()? as i8).saturating_sub(base_priority);
            if has_checksum {
                r.take(4)?;
            }
            if flag_size > 0 {
                r.take(flag_size)?;
            }
            entries.push(DownloadEntry { ekey, size, priority });
        }

        // Tags come after the entries here, unlike the install manifest.
        let tags = r.tags(tag_count, entry_count)?;

        Ok(Self { tags, entries })
    }

    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    pub fn entries(&self) -> &[DownloadEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The manifest positions `tags` selects, in manifest order, which is what
    /// a tag's mask is addressed by.
    pub fn selected_indices(&self, tags: &TagSet) -> Vec<usize> {
        let selection = tags.against(&self.tags, self.entries.len());
        (0..self.entries.len()).filter(|i| selection.selects(*i)).collect()
    }

    /// The entries `tags` selects, by priority then key: deterministic, which
    /// resume depends on.
    pub fn select(&self, tags: &TagSet) -> Vec<DownloadEntry> {
        let selection = tags.against(&self.tags, self.entries.len());
        let mut selected: Vec<DownloadEntry> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(i, _)| selection.selects(*i))
            .map(|(_, e)| *e)
            .collect();
        selected.sort_unstable_by_key(|e| (e.priority, e.ekey));
        selected
    }

    /// Total encoded bytes `tags` selects, without materialising the list.
    #[cfg(test)]
    pub fn selected_size(&self, tags: &TagSet) -> u64 {
        let selection = tags.against(&self.tags, self.entries.len());
        self.entries
            .iter()
            .enumerate()
            .filter(|(i, _)| selection.selects(*i))
            .map(|(_, e)| e.size)
            .sum()
    }

    /// Optional content this build offers that `tags` does not ask for.
    pub fn optional_names(&self, tags: &TagSet) -> Vec<String> {
        tags.optional_names(&self.tags, self.entries.len())
    }

    /// The entries present only because `name` was opted in to, with the rest
    /// of `base` held fixed: what switching the option off leaves unneeded.
    pub fn only_with(&self, base: &TagSet, name: &str) -> Vec<DownloadEntry> {
        let with = base
            .clone()
            .with_optional(base.optional().iter().cloned().chain([name.to_owned()]));
        let without = base
            .clone()
            .with_optional(base.optional().iter().filter(|n| *n != name).cloned());
        let kept: std::collections::HashSet<_> =
            self.selected_indices(&without).into_iter().collect();
        self.selected_indices(&with)
            .into_iter()
            .filter(|i| !kept.contains(i))
            .map(|i| self.entries[i])
            .collect()
    }
}

/// Bounds-checked sequential reader: a truncated manifest is an error naming
/// the shortfall, not a panic.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, at: 0, what }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ManifestError> {
        let end = self.at.checked_add(n).ok_or(ManifestError::Truncated {
            what: self.what,
            need: usize::MAX,
            got: self.bytes.len(),
        })?;
        if end > self.bytes.len() {
            return Err(ManifestError::Truncated { what: self.what, need: end, got: self.bytes.len() });
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn u8(&mut self) -> Result<u8, ManifestError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ManifestError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }

    fn u32(&mut self) -> Result<u32, ManifestError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    /// 40-bit big-endian, the width TACT uses for encoded sizes.
    fn u40(&mut self) -> Result<u64, ManifestError> {
        Ok(self.take(5)?.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }

    fn cstring(&mut self) -> Result<String, ManifestError> {
        let rest = &self.bytes[self.at.min(self.bytes.len())..];
        let len = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(ManifestError::UnterminatedString { what: self.what, at: self.at })?;
        let text = String::from_utf8(rest[..len].to_vec())
            .map_err(|_| ManifestError::NonUtf8Path { what: self.what, at: self.at })?;
        self.at += len + 1;
        Ok(text)
    }

    /// The tag section both manifests share: name, type, then one bit per
    /// entry rounded up to a byte.
    fn tags(&mut self, count: usize, entries: usize) -> Result<Vec<Tag>, ManifestError> {
        let mask_len = entries.div_ceil(8);
        let mut tags = Vec::with_capacity(count);
        for _ in 0..count {
            let name = self.cstring()?;
            let kind = self.u16()?;
            let mask = self.take(mask_len)?.to_vec();
            tags.push(Tag { name, kind, mask });
        }
        Ok(tags)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("{what} is truncated: needed {need} bytes, file has {got}")]
    Truncated { what: &'static str, need: usize, got: usize },

    #[error("{what} does not start with its magic")]
    BadMagic { what: &'static str },

    #[error("{what} is version {version}, which is not supported")]
    UnsupportedVersion { what: &'static str, version: u8 },

    #[error("unsupported hash size {0}, expected 16")]
    UnsupportedHashSize(usize),

    #[error("{what} has an unterminated string at byte {at}")]
    UnterminatedString { what: &'static str, at: usize },

    #[error("{what} has a non-UTF-8 path at byte {at}")]
    NonUtf8Path { what: &'static str, at: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an install manifest with MSB-first masks, matching the format.
    fn install(tags: &[(&str, u16, &[usize])], entries: &[(&str, u8, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"IN");
        out.push(1);
        out.push(16);
        out.extend_from_slice(&(tags.len() as u16).to_be_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        let mask_len = entries.len().div_ceil(8);
        for (name, kind, covered) in tags {
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            out.extend_from_slice(&kind.to_be_bytes());
            let mut mask = vec![0u8; mask_len];
            for i in *covered {
                mask[i / 8] |= 0x80 >> (i % 8);
            }
            out.extend_from_slice(&mask);
        }
        for (path, key, size) in entries {
            out.extend_from_slice(path.as_bytes());
            out.push(0);
            out.extend_from_slice(&[*key; 16]);
            out.extend_from_slice(&size.to_be_bytes());
        }
        out
    }

    /// Build a v1 download manifest: no checksum, no flags, no priority base.
    fn download(tags: &[(&str, u16, &[usize])], entries: &[(u8, u64, i8)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"DL");
        out.push(1);
        out.push(16);
        out.push(0);
        out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        out.extend_from_slice(&(tags.len() as u16).to_be_bytes());
        for (key, size, priority) in entries {
            out.extend_from_slice(&[*key; 16]);
            out.extend_from_slice(&size.to_be_bytes()[3..]);
            out.push(*priority as u8);
        }
        let mask_len = entries.len().div_ceil(8);
        for (name, kind, covered) in tags {
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            out.extend_from_slice(&kind.to_be_bytes());
            let mut mask = vec![0u8; mask_len];
            for i in *covered {
                mask[i / 8] |= 0x80 >> (i % 8);
            }
            out.extend_from_slice(&mask);
        }
        out
    }

    #[test]
    fn an_install_manifest_round_trips_paths_keys_and_sizes() {
        let bytes = install(
            &[("Windows", 1, &[0, 2]), ("OSX", 1, &[1])],
            &[("Wow.exe", 1, 74778320), ("Blizzard Error.app", 2, 100), ("Utils\\cef.pak", 3, 2206428)],
        );
        let m = InstallManifest::parse(&bytes).unwrap();

        assert_eq!(m.len(), 3);
        assert_eq!(m.entries()[0].path, "Wow.exe");
        assert_eq!(m.entries()[0].ckey, [1u8; 16]);
        assert_eq!(m.entries()[0].size, 74778320);
        assert_eq!(m.tags().len(), 2);
    }

    /// The bug in `tact_parser::install`, pinned: reading these masks
    /// LSB-first hands the Windows tag a macOS bundle.
    #[test]
    fn tag_bits_are_read_high_bit_first() {
        let bytes = install(
            &[("Windows", 1, &[0, 2]), ("OSX", 1, &[1])],
            &[("Wow.exe", 1, 1), ("Blizzard Error.app", 2, 1), ("Utils\\cef.pak", 3, 1)],
        );
        let m = InstallManifest::parse(&bytes).unwrap();

        let windows = m.select(&TagSet::of(["Windows"]));
        let paths: Vec<&str> = windows.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["Wow.exe", "Utils\\cef.pak"]);
        assert!(!paths.iter().any(|p| p.contains(".app")));

        let osx = m.select(&TagSet::of(["OSX"]));
        assert_eq!(osx.len(), 1);
        assert_eq!(osx[0].path, "Blizzard Error.app");
    }

    #[test]
    fn a_group_requires_every_tag_and_groups_are_alternatives() {
        let bytes = install(
            &[
                ("Windows", 1, &[0, 1, 2]),
                ("x86_64", 2, &[0, 2]),
                ("speech", 5, &[0]),
                ("text", 5, &[2]),
            ],
            &[("a", 1, 1), ("b", 2, 1), ("c", 3, 1)],
        );
        let m = InstallManifest::parse(&bytes).unwrap();

        // Windows+x86_64+speech matches only a; adding the text alternative
        // brings in c. b fails on x86_64 in both.
        let one = m.select(&TagSet::parse("Windows x86_64 speech"));
        assert_eq!(one.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a"]);

        let both = m.select(&TagSet::parse("Windows x86_64 speech:Windows x86_64 text"));
        assert_eq!(both.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    }

    #[test]
    fn names_the_manifest_does_not_define_are_ignored_not_required() {
        let bytes = install(&[("Windows", 1, &[0])], &[("a", 1, 1), ("b", 2, 1)]);
        let m = InstallManifest::parse(&bytes).unwrap();

        // `geoip-AT` and `acct-AUT` are facts about the account, not file tags.
        // Requiring them would select nothing at all.
        let tags = TagSet::parse("Windows geoip-AT? acct-AUT?");
        assert_eq!(m.select(&tags).len(), 1);
        assert_eq!(tags.unknown_names(m.tags()), ["acct-AUT", "geoip-AT"]);
    }

    #[test]
    fn an_empty_tag_column_constrains_nothing() {
        let bytes = install(&[("Windows", 1, &[0])], &[("a", 1, 1), ("b", 2, 1)]);
        let m = InstallManifest::parse(&bytes).unwrap();

        assert!(TagSet::parse("").is_empty());
        assert_eq!(m.select(&TagSet::parse("")).len(), 2);
    }

    #[test]
    fn padding_bits_past_the_last_entry_are_not_entries() {
        // Three entries, so the mask's last five bits address nothing. A tag
        // covering every bit must still select exactly three.
        let bytes = install(&[("All", 1, &[0, 1, 2, 3, 4, 5, 6, 7])], &[("a", 1, 1), ("b", 2, 1), ("c", 3, 1)]);
        let m = InstallManifest::parse(&bytes).unwrap();

        assert_eq!(m.select(&TagSet::of(["All"])).len(), 3);
    }

    #[test]
    fn a_download_manifest_reads_sizes_as_40_bit_and_orders_by_priority() {
        let bytes = download(
            &[("enUS", 3, &[0, 1, 2])],
            &[(1, 0x00_1F_FF_FF_FF_FF, 2), (2, 100, 0), (3, 500, 1)],
        );
        let m = DownloadManifest::parse(&bytes).unwrap();

        assert_eq!(m.len(), 3);
        assert_eq!(m.entries()[0].size, 0x1F_FF_FF_FF_FF);

        let selected = m.select(&TagSet::of(["enUS"]));
        assert_eq!(
            selected.iter().map(|e| e.priority).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(selected[0].ekey, [2u8; 16]);
    }

    #[test]
    fn a_selection_and_its_size_agree() {
        let bytes = download(
            &[("enUS", 3, &[0, 2]), ("deDE", 3, &[1])],
            &[(1, 1000, 0), (2, 2000, 0), (3, 3000, 0)],
        );
        let m = DownloadManifest::parse(&bytes).unwrap();

        let tags = TagSet::of(["enUS"]);
        let selected = m.select(&tags);
        assert_eq!(selected.len(), 2);
        assert_eq!(
            m.selected_size(&tags),
            selected.iter().map(|e| e.size).sum::<u64>()
        );
        assert_eq!(m.selected_size(&tags), 4000);
    }

    #[test]
    fn a_category_that_separates_nothing_cannot_withhold_anything() {
        // Warcraft I Remastered, reduced: every entry is tagged `Debug` and
        // `Release`, and a WoW-shaped expression names neither. Gating them
        // emptied the plan of all 746 files.
        let bytes = download(
            &[
                ("Windows", 2, &[0, 1, 2]),
                ("enUS", 1, &[0, 1, 2]),
                ("Debug", 3, &[0, 1, 2]),
                ("Release", 3, &[0, 1, 2]),
            ],
            &[(1, 100, 0), (2, 200, 0), (3, 300, 0)],
        );
        let manifest = DownloadManifest::parse(&bytes).unwrap();

        // Nothing names Debug or Release, exactly as `.build.info` would not.
        let tags = TagSet::parse("Windows x86_64 EU? enUS speech?:Windows x86_64 EU? enUS text?");
        assert_eq!(manifest.select(&tags).len(), 3, "the whole manifest, not none of it");
        assert_eq!(manifest.selected_size(&tags), 600);
        assert!(
            manifest.optional_names(&tags).is_empty(),
            "a category that covers everything is not on offer either — there is \
             nothing to offer, and listing it invites opting in to the install itself"
        );
    }

    #[test]
    fn optional_content_is_excluded_unless_it_is_asked_for_by_name() {
        // The 2026-09 defect: `HighRes` is its own category, never in a Tags
        // column, and leaving it unconstrained put a 37 GiB texture pack into
        // the Forever beta's plan.
        let bytes = download(
            &[
                ("Windows", 1, &[0, 1, 2]),
                ("enUS", 3, &[0, 1, 2]),
                ("HighRes", 16384, &[2]),
            ],
            &[(1, 100, 0), (2, 200, 0), (3, 9000, 0)],
        );
        let manifest = DownloadManifest::parse(&bytes).unwrap();

        let tags = TagSet::parse("Windows enUS");
        assert_eq!(manifest.select(&tags).len(), 2);
        assert_eq!(manifest.selected_size(&tags), 300);

        // And it is on offer, not hidden: the user can find it.
        assert_eq!(manifest.optional_names(&tags), ["HighRes"]);
    }

    #[test]
    fn opting_in_admits_the_optional_content_without_requiring_it() {
        // A permission, not a requirement. Requiring `HighRes` would select
        // only the texture pack and drop the game.
        let bytes = download(
            &[
                ("Windows", 1, &[0, 1, 2]),
                ("enUS", 3, &[0, 1, 2]),
                ("HighRes", 16384, &[2]),
            ],
            &[(1, 100, 0), (2, 200, 0), (3, 9000, 0)],
        );
        let manifest = DownloadManifest::parse(&bytes).unwrap();

        let tags = TagSet::parse("Windows enUS").with_optional(["HighRes"]);
        assert_eq!(manifest.select(&tags).len(), 3);
        assert_eq!(manifest.selected_size(&tags), 9300);
    }

    #[test]
    fn what_an_option_alone_brings_in_is_what_turning_it_off_frees() {
        // File 1 is the game, file 2 is tagged HighRes but is not part of this
        // install's base selection (another locale's, say), file 3 is the
        // texture pack the base install selects once the option is on.
        let bytes = download(
            &[
                ("Windows", 1, &[0, 1, 2]),
                ("Base", 5, &[0, 2]),
                ("HighRes", 16384, &[1, 2]),
            ],
            &[(1, 100, 0), (2, 200, 0), (3, 400, 0)],
        );
        let manifest = DownloadManifest::parse(&bytes).unwrap();

        let base = TagSet::parse("Windows Base");
        let only: Vec<u64> =
            manifest.only_with(&base, "HighRes").iter().map(|e| e.size).collect();
        assert_eq!(only, [400]);

        // Already opted in: the same answer, and another opt-in is untouched.
        let opted = base.with_optional(["HighRes"]);
        assert_eq!(manifest.only_with(&opted, "HighRes").len(), 1);
        assert!(manifest.only_with(&opted, "Missing").is_empty());
    }

    #[test]
    fn opting_in_to_one_option_does_not_admit_another() {
        let bytes = download(
            &[
                ("Windows", 1, &[0, 1, 2]),
                ("HighRes", 16384, &[1]),
                ("Alternate", 16384, &[2]),
            ],
            &[(1, 100, 0), (2, 200, 0), (3, 400, 0)],
        );
        let manifest = DownloadManifest::parse(&bytes).unwrap();

        let tags = TagSet::parse("Windows").with_optional(["HighRes"]);
        assert_eq!(manifest.selected_size(&tags), 300);
    }

    #[test]
    fn a_category_the_column_names_any_value_of_is_still_narrowed_not_gated() {
        // Naming `enUS` constrains locale; the other locales must be excluded
        // by the group rule as before, not treated as optional extras.
        let bytes = download(
            &[
                ("Windows", 1, &[0, 1]),
                ("enUS", 3, &[0]),
                ("deDE", 3, &[1]),
            ],
            &[(1, 100, 0), (2, 200, 0)],
        );
        let manifest = DownloadManifest::parse(&bytes).unwrap();

        let tags = TagSet::parse("Windows enUS");
        assert_eq!(manifest.selected_size(&tags), 100);
        assert!(manifest.optional_names(&tags).is_empty());
    }

    #[test]
    fn install_paths_become_relative_posix_paths() {
        let entry = InstallEntry {
            path: "Utils\\swiftshader\\libEGL.dll".into(),
            ckey: [0; 16],
            size: 1,
        };
        assert_eq!(
            entry.relative_path().unwrap(),
            std::path::Path::new("Utils/swiftshader/libEGL.dll")
        );
    }

    #[test]
    fn install_paths_never_leave_the_install() {
        let path = |path: &str| InstallEntry { path: path.into(), ckey: [0; 16], size: 1 }.relative_path();

        assert_eq!(path("..\\..\\.bashrc"), None);
        assert_eq!(path("Utils\\..\\..\\Wow.exe"), None);
        assert_eq!(path("Utils/../../Wow.exe"), None);
        assert_eq!(path(""), None);
        // A leading separator would have made the join absolute.
        assert_eq!(path("\\etc\\passwd").unwrap(), std::path::Path::new("etc/passwd"));
        assert_eq!(path("/etc/passwd").unwrap(), std::path::Path::new("etc/passwd"));
        assert_eq!(path(".\\Wow.exe").unwrap(), std::path::Path::new("Wow.exe"));
    }

    #[test]
    fn truncation_and_bad_magic_are_reported_rather_than_panicking() {
        assert!(matches!(
            InstallManifest::parse(b"XX\x01\x10"),
            Err(ManifestError::BadMagic { .. })
        ));
        assert!(matches!(
            InstallManifest::parse(b"IN"),
            Err(ManifestError::Truncated { .. })
        ));
        assert!(matches!(
            DownloadManifest::parse(b"DL\x09\x10\x00\x00\x00\x00\x01\x00\x00"),
            Err(ManifestError::UnsupportedVersion { .. })
        ));

        // A header promising more entries than the file holds.
        let mut bytes = download(&[("enUS", 3, &[0])], &[(1, 10, 0)]);
        bytes[5..9].copy_from_slice(&9000u32.to_be_bytes());
        assert!(matches!(
            DownloadManifest::parse(&bytes),
            Err(ManifestError::Truncated { .. })
        ));
    }

    #[test]
    fn a_huge_entry_count_is_truncation_not_an_allocation() {
        assert!(matches!(
            InstallManifest::parse(b"IN\x01\x10\x00\x00\xff\xff\xff\xff"),
            Err(ManifestError::Truncated { .. } | ManifestError::UnterminatedString { .. })
        ));
        assert!(matches!(
            DownloadManifest::parse(b"DL\x01\x10\x00\xff\xff\xff\xff\x00\x00"),
            Err(ManifestError::Truncated { .. })
        ));
    }

    #[test]
    fn an_unterminated_name_does_not_run_off_the_end() {
        let mut bytes = install(&[("Windows", 1, &[0])], &[("a", 1, 1)]);
        // Strip the terminator off the single entry path and everything after.
        let cut = bytes.len() - 21;
        bytes.truncate(cut);
        assert!(matches!(
            InstallManifest::parse(&bytes),
            Err(ManifestError::UnterminatedString { .. } | ManifestError::Truncated { .. })
        ));
    }
}
