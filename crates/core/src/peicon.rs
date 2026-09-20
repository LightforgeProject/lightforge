//! Pulling an application icon out of a Windows executable.
//!
//! **Why this exists:** Battle.net's non-WoW games ship no loose icon files —
//! a Hearthstone install has zero `.png`, `.ico` or `.jpg` anywhere in it. The
//! art is inside `Hearthstone.exe`, in the PE resource tree, which is also
//! where Windows itself reads it from. Lightforge shows those games as
//! hand-off tiles, and a tile wants a face.
//!
//! No external binary. `icoutils`/`wrestool` would do this in one line, but a
//! launcher that only draws icons on distributions shipping a particular
//! package is not one anybody wants to debug.
//!
//! The output is a `.ico` file rather than a decoded bitmap: an icon resource
//! is *already* a set of images at several sizes, `.ico` is exactly the
//! container for that set, and image decoders — including the one iced uses —
//! read it and pick the largest. Re-encoding would mean choosing a size here,
//! where there is no window to choose it for.
//!
//! Every read is bounds-checked and every offset is validated. This parses a
//! third-party binary that Blizzard rewrites on its own schedule; it is
//! allowed to find nothing, and not allowed to panic.

use std::path::Path;

/// Resource type ids, from `winuser.h`.
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;

/// Extract an executable's icon as `.ico` bytes.
///
/// Takes the *first* icon group, which is the one Windows shows for the file:
/// the loader picks the lowest-numbered group, and so does this.
pub fn extract(exe: &Path) -> Result<Vec<u8>, IconError> {
    let bytes = std::fs::read(exe).map_err(|source| IconError::Io {
        path: exe.to_path_buf(),
        source,
    })?;
    from_bytes(&bytes).ok_or_else(|| IconError::NoIcon(exe.to_path_buf()))
}

/// The executable an install directory is *for*.
///
/// A Blizzard install holds several: the game, a launcher, an uninstaller, a
/// crash reporter. The one whose name matches the directory is the game —
/// `Hearthstone/Hearthstone.exe` — and failing that, the largest one that is
/// not obviously a helper. Only ever used to find an icon, so a wrong guess
/// costs a wrong picture, never a wrong launch.
pub fn main_executable(dir: &Path) -> Option<std::path::PathBuf> {
    const HELPERS: &[&str] = &[
        "launcher",
        "uninstall",
        "helper",
        "error",
        "setup",
        "crash",
        "repair",
    ];

    let wanted = dir.file_name()?.to_string_lossy().to_lowercase();
    let mut best: Option<(u64, std::path::PathBuf)> = None;

    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("exe")) {
            continue;
        }
        let stem = path.file_stem()?.to_string_lossy().to_lowercase();
        if stem == wanted {
            return Some(path);
        }
        if HELPERS.iter().any(|h| stem.contains(h)) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if best.as_ref().is_none_or(|(best, _)| size > *best) {
            best = Some((size, path));
        }
    }
    best.map(|(_, path)| path)
}

/// The parsing half, separate so it can be tested without a file on disk.
pub fn from_bytes(pe: &[u8]) -> Option<Vec<u8>> {
    let sections = Sections::parse(pe)?;
    let resources = sections.resource_directory(pe)?;

    // The group lists which images belong together and at what sizes; the
    // images themselves live under RT_ICON, addressed by the ids it names.
    let group = first_leaf(pe, &sections, resources, RT_GROUP_ICON)?;
    let icons = type_directory(pe, resources, RT_ICON)?;

    assemble(pe, &sections, resources, icons, group)
}

/// Build a `.ico` from a group descriptor and the images it names.
fn assemble(
    pe: &[u8],
    sections: &Sections,
    resources: usize,
    icons: usize,
    group: &[u8],
) -> Option<Vec<u8>> {
    // GRPICONDIR: reserved, type, count.
    let count = u16_at(group, 4)? as usize;
    if count == 0 {
        return None;
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        // GRPICONDIRENTRY is 14 bytes and its ICONDIRENTRY twin is 16. They
        // agree for the first 8 — width, height, colours, reserved, planes,
        // bit count — and then diverge: the group says `bytesInRes` + a
        // 2-byte resource id, the file says `bytesInRes` + a 4-byte offset.
        // Only those first 8 bytes can be copied across.
        let at = 6 + i * 14;
        let head = group.get(at..at + 14)?;
        let id = u32::from(u16_at(head, 12)?);
        let image = leaf_by_id(pe, sections, resources, icons, id)?;
        entries.push((head[..8].to_vec(), image));
    }

    // ICONDIR, then one 16-byte ICONDIRENTRY each, then the images.
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    let mut offset = 6 + entries.len() * 16;
    for (head, image) in &entries {
        out.extend_from_slice(head);
        // The resource's real length, not the length the group claims: the
        // bytes about to be written are the authority on how many there are.
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for (_, image) in &entries {
        out.extend_from_slice(image);
    }
    Some(out)
}

// ------------------------------------------------------------ PE headers --

/// The section table, which is all that is needed to turn an RVA into a file
/// offset.
struct Sections {
    /// `(virtual_address, virtual_size, raw_pointer, raw_size)`.
    entries: Vec<(u32, u32, u32, u32)>,
    resource_rva: u32,
}

impl Sections {
    fn parse(pe: &[u8]) -> Option<Self> {
        // MZ, then e_lfanew at 0x3C points at the PE signature.
        if pe.get(..2)? != b"MZ" {
            return None;
        }
        let at = u32_at(pe, 0x3c)? as usize;
        if pe.get(at..at + 4)? != b"PE\0\0" {
            return None;
        }

        let coff = at + 4;
        let section_count = u16_at(pe, coff + 2)? as usize;
        let optional_size = u16_at(pe, coff + 16)? as usize;
        let optional = coff + 20;

        // The data directory sits after the optional header's fixed part,
        // which differs between PE32 and PE32+.
        let directories = match u16_at(pe, optional)? {
            0x10b => optional + 96,
            0x20b => optional + 112,
            _ => return None,
        };
        // Entry 2 is the resource table.
        let resource_rva = u32_at(pe, directories + 2 * 8)?;
        if resource_rva == 0 {
            return None;
        }

        let table = optional + optional_size;
        let mut entries = Vec::with_capacity(section_count);
        for i in 0..section_count {
            let at = table + i * 40;
            entries.push((
                u32_at(pe, at + 12)?,
                u32_at(pe, at + 8)?,
                u32_at(pe, at + 20)?,
                u32_at(pe, at + 16)?,
            ));
        }

        Some(Self { entries, resource_rva })
    }

    /// Where the resource tree starts in the file.
    fn resource_directory(&self, pe: &[u8]) -> Option<usize> {
        let at = self.offset(self.resource_rva)?;
        // A directory header is 16 bytes; anything shorter is not one.
        (at + 16 <= pe.len()).then_some(at)
    }

    fn offset(&self, rva: u32) -> Option<usize> {
        for &(virtual_address, virtual_size, raw_pointer, raw_size) in &self.entries {
            let end = virtual_address.checked_add(virtual_size.max(raw_size))?;
            if rva >= virtual_address && rva < end {
                let delta = rva - virtual_address;
                // A section can be larger in memory than on disk; an RVA
                // pointing into that gap has no bytes behind it.
                if delta >= raw_size {
                    return None;
                }
                return usize::try_from(raw_pointer.checked_add(delta)?).ok();
            }
        }
        None
    }
}

// --------------------------------------------------------- resource tree --

/// Iterate one directory's entries as `(id_or_name, offset, is_subdirectory)`.
fn entries(pe: &[u8], at: usize) -> impl Iterator<Item = (u32, u32, bool)> + '_ {
    let named = u16_at(pe, at + 12).unwrap_or(0) as usize;
    let ids = u16_at(pe, at + 14).unwrap_or(0) as usize;
    (0..named + ids).filter_map(move |i| {
        let entry = at + 16 + i * 8;
        let name = u32_at(pe, entry)?;
        let offset = u32_at(pe, entry + 4)?;
        Some((name, offset & 0x7fff_ffff, offset & 0x8000_0000 != 0))
    })
}

/// The subdirectory for one resource type.
fn type_directory(pe: &[u8], resources: usize, want: u32) -> Option<usize> {
    entries(pe, resources)
        .find(|&(id, _, sub)| sub && id == want)
        .map(|(_, offset, _)| resources + offset as usize)
}

/// The first leaf under a resource type — its lowest-numbered entry.
fn first_leaf<'a>(
    pe: &'a [u8],
    sections: &Sections,
    resources: usize,
    kind: u32,
) -> Option<&'a [u8]> {
    let directory = type_directory(pe, resources, kind)?;
    let (_, offset, sub) = entries(pe, directory).min_by_key(|&(id, _, _)| id)?;
    let data = match sub {
        // Language level.
        true => {
            let languages = resources + offset as usize;
            let (_, leaf, _) = entries(pe, languages).next()?;
            resources + leaf as usize
        }
        false => resources + offset as usize,
    };
    leaf(pe, sections, data)
}

/// One RT_ICON image by the id its group names.
fn leaf_by_id<'a>(
    pe: &'a [u8],
    sections: &Sections,
    resources: usize,
    icons: usize,
    id: u32,
) -> Option<&'a [u8]> {
    let (_, offset, sub) = entries(pe, icons).find(|&(entry, _, _)| entry == id)?;
    let data = match sub {
        true => {
            let languages = resources + offset as usize;
            let (_, leaf, _) = entries(pe, languages).next()?;
            resources + leaf as usize
        }
        false => resources + offset as usize,
    };
    leaf(pe, sections, data)
}

/// Resolve an IMAGE_RESOURCE_DATA_ENTRY to the bytes it points at.
fn leaf<'a>(pe: &'a [u8], sections: &Sections, at: usize) -> Option<&'a [u8]> {
    let rva = u32_at(pe, at)?;
    let size = u32_at(pe, at + 4)? as usize;
    let start = sections.offset(rva)?;
    pe.get(start..start.checked_add(size)?)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[derive(Debug, thiserror::Error)]
pub enum IconError {
    #[error("reading {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} carries no icon resource")]
    NoIcon(std::path::PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PE32+ with one `.rsrc` section holding one icon group and two
    /// images. Small enough to reason about, complete enough that the real
    /// parser walks it exactly as it walks Hearthstone's.
    fn synthetic_pe(images: &[(u16, Vec<u8>)]) -> Vec<u8> {
        const BASE: u32 = 0x1000;
        let mut rsrc = Vec::new();

        // Layout is built in two passes: the tree needs offsets into data
        // that follows it, so lay the data out first and fill in after.
        let dir = |named: u16, ids: u16| {
            let mut d = vec![0u8; 12];
            d.extend_from_slice(&named.to_le_bytes());
            d.extend_from_slice(&ids.to_le_bytes());
            d
        };
        let entry = |id: u32, offset: u32, sub: bool| {
            let mut e = id.to_le_bytes().to_vec();
            e.extend_from_slice(&(offset | if sub { 0x8000_0000 } else { 0 }).to_le_bytes());
            e
        };

        // Root: RT_ICON then RT_GROUP_ICON, each with one type directory.
        let root_len = 16 + 2 * 8;
        let icon_dir_at = root_len as u32;
        let icon_dir_len = 16 + images.len() * 8;
        let group_dir_at = icon_dir_at + icon_dir_len as u32;
        let group_dir_len = 16 + 8;
        let leaves_at = group_dir_at + group_dir_len as u32;
        // One IMAGE_RESOURCE_DATA_ENTRY (16 bytes) per image, plus the group.
        let data_at = leaves_at + (images.len() as u32 + 1) * 16;

        rsrc.extend(dir(0, 2));
        rsrc.extend(entry(RT_ICON, icon_dir_at, true));
        rsrc.extend(entry(RT_GROUP_ICON, group_dir_at, true));

        rsrc.extend(dir(0, images.len() as u16));
        for (i, (id, _)) in images.iter().enumerate() {
            rsrc.extend(entry(u32::from(*id), leaves_at + i as u32 * 16, false));
        }

        rsrc.extend(dir(0, 1));
        rsrc.extend(entry(1, leaves_at + images.len() as u32 * 16, false));

        // Leaves, then the bytes they point at.
        let mut payload = Vec::new();
        let mut group = vec![0u8, 0, 1, 0];
        group.extend_from_slice(&(images.len() as u16).to_le_bytes());
        for (id, bytes) in images {
            group.extend_from_slice(&[32, 32, 0, 0]);
            group.extend_from_slice(&1u16.to_le_bytes());
            group.extend_from_slice(&32u16.to_le_bytes());
            group.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            group.extend_from_slice(&id.to_le_bytes());
        }

        for (_, bytes) in images {
            rsrc.extend_from_slice(&(BASE + data_at + payload.len() as u32).to_le_bytes());
            rsrc.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            rsrc.extend_from_slice(&[0u8; 8]);
            payload.extend_from_slice(bytes);
        }
        rsrc.extend_from_slice(&(BASE + data_at + payload.len() as u32).to_le_bytes());
        rsrc.extend_from_slice(&(group.len() as u32).to_le_bytes());
        rsrc.extend_from_slice(&[0u8; 8]);
        payload.extend_from_slice(&group);
        rsrc.extend_from_slice(&payload);

        // Headers.
        let pe_at = 0x80usize;
        let optional_size = 112 + 16 * 8;
        let raw_at = 0x400u32;

        let mut out = vec![0u8; pe_at];
        out[..2].copy_from_slice(b"MZ");
        out[0x3c..0x40].copy_from_slice(&(pe_at as u32).to_le_bytes());

        out.extend_from_slice(b"PE\0\0");
        out.extend_from_slice(&0x8664u16.to_le_bytes()); // machine
        out.extend_from_slice(&1u16.to_le_bytes()); // one section
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(&(optional_size as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());

        let optional_at = out.len();
        out.extend_from_slice(&0x20bu16.to_le_bytes()); // PE32+
        out.resize(optional_at + optional_size, 0);
        // Data directory entry 2 is the resource table; entries are 8 bytes
        // and the directory starts 112 bytes into a PE32+ optional header.
        let resource_entry = optional_at + 112 + 2 * 8;
        out[resource_entry..resource_entry + 4].copy_from_slice(&BASE.to_le_bytes());
        out[resource_entry + 4..resource_entry + 8]
            .copy_from_slice(&(rsrc.len() as u32).to_le_bytes());

        let mut section = b".rsrc\0\0\0".to_vec();
        section.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // virtual size
        section.extend_from_slice(&BASE.to_le_bytes()); // virtual address
        section.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // raw size
        section.extend_from_slice(&raw_at.to_le_bytes());
        section.resize(40, 0);
        out.extend_from_slice(&section);

        out.resize(raw_at as usize, 0);
        out.extend_from_slice(&rsrc);
        out
    }

    #[test]
    fn an_icon_group_becomes_an_ico_of_every_image_it_names() {
        let images = vec![(1u16, vec![0xAAu8; 40]), (2u16, vec![0xBBu8; 80])];
        let ico = from_bytes(&synthetic_pe(&images)).expect("an icon");

        // ICONDIR, then one 16-byte entry each. Getting this length wrong is
        // exactly the bug that made real icons unreadable.
        assert_eq!(&ico[..2], &[0, 0]);
        assert_eq!(u16_at(&ico, 2), Some(1));
        assert_eq!(u16_at(&ico, 4), Some(2));
        assert_eq!(ico.len(), 6 + 2 * 16 + 40 + 80);

        // Each entry's size and offset must actually address its image.
        let mut expected = 6 + 2 * 16;
        for (i, (_, bytes)) in images.iter().enumerate() {
            let at = 6 + i * 16;
            assert_eq!(u32_at(&ico, at + 8), Some(bytes.len() as u32));
            assert_eq!(u32_at(&ico, at + 12), Some(expected as u32));
            assert_eq!(&ico[expected..expected + bytes.len()], bytes.as_slice());
            expected += bytes.len();
        }
    }

    #[test]
    fn a_file_that_is_not_a_pe_is_not_an_icon() {
        assert!(from_bytes(b"").is_none());
        assert!(from_bytes(b"#!/bin/sh\necho hello\n").is_none());
        assert!(from_bytes(&[0u8; 4096]).is_none());
    }

    /// Third-party binaries, rewritten on someone else's schedule. Every
    /// truncation and every single-byte corruption must answer "no icon"
    /// rather than panic.
    #[test]
    fn damaged_executables_are_refused_rather_than_trusted() {
        let whole = synthetic_pe(&[(1u16, vec![0xCC; 64])]);

        for cut in 0..whole.len() {
            let _ = from_bytes(&whole[..cut]);
        }
        for at in (0..whole.len()).step_by(7) {
            let mut damaged = whole.clone();
            damaged[at] = damaged[at].wrapping_add(0x5b);
            let _ = from_bytes(&damaged);
        }
    }
}
