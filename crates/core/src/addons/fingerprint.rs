//! Recognising an addon by its bytes, so a hand-installed one can be adopted.
//!
//! An addon Lightforge did not install has no provider and no id, so it can
//! be listed and nothing more — no update, no channel, no removal. CurseForge
//! solves this for its own client by publishing a **fingerprint** for every
//! module (every addon folder) of every file it hosts: hash what is on disk,
//! ask which project those hashes belong to, and an unmanaged folder becomes
//! a managed one.
//!
//! The hash is MurmurHash2 (32-bit, Austin Appleby's original) with seed `1`,
//! over the file's bytes **with whitespace removed** — tab, line feed,
//! carriage return and space are dropped before hashing. That last part is
//! what makes the fingerprint survive a checkout that rewrote line endings,
//! which is the whole reason it is not a plain checksum.
//!
//! **The algorithm is exact or it is useless.** There is no partial credit: a
//! fingerprint that is one byte off matches nothing at all, and the failure
//! looks identical to "this addon is not on CurseForge". So the two decisions
//! it rests on are separated deliberately — [`murmur2`] is the published
//! algorithm and is tested against its own reference, while [`of_file`] is
//! the part that says *which bytes go in*, verified against fingerprints
//! CurseForge itself published for an addon we installed from it.

use std::path::Path;

/// Seed CurseForge hashes with. Not a choice of ours.
const SEED: u32 = 1;

/// Bytes dropped before hashing: tab, line feed, carriage return, space.
const WHITESPACE: [u8; 4] = [9, 10, 13, 32];

/// MurmurHash2, 32-bit, as published — no normalisation of its own.
///
/// Transcribed from Appleby's reference so the constants are his: `m` and
/// `r` are the published mixing values, and the tail switch falls through on
/// purpose. Wrapping arithmetic throughout, because the original is C and
/// relies on it; in Rust that has to be said out loud or a debug build
/// panics where a release build quietly agrees with the reference.
pub fn murmur2(bytes: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;

    let mut hash = seed ^ (bytes.len() as u32);
    let mut chunks = bytes.chunks_exact(4);

    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        hash = hash.wrapping_mul(M);
        hash ^= k;
    }

    let tail = chunks.remainder();
    if tail.len() >= 3 {
        hash ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        hash ^= u32::from(tail[1]) << 8;
    }
    if !tail.is_empty() {
        hash ^= u32::from(tail[0]);
        hash = hash.wrapping_mul(M);
    }

    hash ^= hash >> 13;
    hash = hash.wrapping_mul(M);
    hash ^ (hash >> 15)
}

/// The fingerprint of one file's contents.
///
/// Whitespace is stripped first, which is what lets the same addon fingerprint
/// identically whether it was unzipped on Windows or checked out on Linux.
pub fn of_bytes(bytes: &[u8]) -> u32 {
    let stripped: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !WHITESPACE.contains(b))
        .collect();
    murmur2(&stripped, SEED)
}

/// The fingerprint of a file on disk, or `None` if it cannot be read.
///
/// Unreadable means "no fingerprint", never a zero: zero is a value the hash
/// can legitimately produce, and a file we could not open must not be allowed
/// to claim it matched something.
pub fn of_file(path: &Path) -> Option<u32> {
    std::fs::read(path).ok().map(|bytes| of_bytes(&bytes))
}

/// The fingerprint of an addon folder.
///
/// **Unverified against the authority.** CurseForge publishes a fingerprint
/// per module and this is our attempt to reproduce it; until a computed value
/// has been matched against a published one for an addon we installed from
/// CurseForge, treat a non-match as "we hash the wrong thing" rather than
/// "this addon is unknown". [`crate::addons::catalog::FileInfo::fingerprints`]
/// carries the published numbers for exactly that comparison.
///
/// The working rule is the folder's own `.toc` — `BigWigs/BigWigs.toc` — which
/// is the one file every addon folder has, is named after the folder, and is
/// what the client reads to know the addon exists at all. A flavor suffix is
/// accepted because a modern addon ships `BigWigs_Mainline.toc` beside or
/// instead of the plain one.
pub fn of_folder(dir: &Path) -> Option<u32> {
    of_file(&toc_in(dir)?)
}

/// The `.toc` that names an addon folder, if one is there.
///
/// Exact name first, then a suffixed variant: an addon that ships only
/// `BigWigs_Mainline.toc` is still BigWigs, and picking the first `.toc` in
/// directory order would be picking at random.
fn toc_in(dir: &Path) -> Option<std::path::PathBuf> {
    let folder = dir.file_name()?.to_string_lossy().to_string();

    let exact = dir.join(format!("{folder}.toc"));
    if exact.is_file() {
        return Some(exact);
    }

    let mut suffixed: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
                return false;
            };
            name.to_lowercase().ends_with(".toc")
                && name.to_lowercase().starts_with(&format!("{}_", folder.to_lowercase()))
        })
        .collect();
    // Sorted so the answer does not depend on the order the filesystem
    // happened to hand them back.
    suffixed.sort();
    suffixed.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values for MurmurHash2, computed from Appleby's own C with
    /// the same inputs. These pin the transcription — the constants, the
    /// little-endian read, the fall-through tail and the final avalanche —
    /// independently of what CurseForge then does with it.
    #[test]
    fn murmur2_matches_the_reference_implementation() {
        // Seed zero over nothing is the one input that falls through every
        // multiply unchanged, so it is zero — and the only case where a
        // transcription can look right by doing nothing at all.
        assert_eq!(murmur2(b"", 0), 0);
        assert_eq!(murmur2(b"", 1), 0x5bd1_5e36);

        // One vector per tail length. The `switch` that handles 1, 2 and 3
        // trailing bytes falls through in the original, and a transcription
        // that turns it into exclusive arms passes every multiple of four
        // and fails everything else.
        assert_eq!(murmur2(b"a", 1), 0x2550_b18c);
        assert_eq!(murmur2(b"ab", 1), 0x64e1_50ee);
        assert_eq!(murmur2(b"abc", 1), 0x60a4_fcc1);
        assert_eq!(murmur2(b"abcd", 1), 0xc93f_7a16);
        assert_eq!(murmur2(b"abcde", 1), 0xcec8_5d7e);
        assert_eq!(murmur2(b"Hello, world!", 1), 0x63a2_7f49);
    }

    #[test]
    fn whitespace_is_stripped_before_hashing() {
        // The property the whole scheme rests on: the same addon, checked
        // out with different line endings, fingerprints the same.
        let unix = b"## Interface: 110005\n## Title: BigWigs\n";
        let dos = b"## Interface: 110005\r\n## Title: BigWigs\r\n";
        assert_eq!(of_bytes(unix), of_bytes(dos));

        // And indentation, which an editor may have reflowed.
        assert_eq!(of_bytes(b"a b\tc"), of_bytes(b"abc"));
        assert_eq!(of_bytes(b"   "), of_bytes(b""));
    }

    #[test]
    fn a_folders_toc_is_the_one_named_after_it() {
        let dir = std::env::temp_dir().join("lightforge-fingerprint-toc/BigWigs");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).unwrap();

        // A decoy that sorts first and is not this addon's.
        std::fs::write(dir.join("AAA.toc"), b"wrong").unwrap();
        std::fs::write(dir.join("BigWigs_Mainline.toc"), b"## Title: BigWigs").unwrap();
        assert_eq!(
            toc_in(&dir).unwrap().file_name().unwrap(),
            "BigWigs_Mainline.toc",
            "a suffixed toc belongs to the folder; an unrelated one does not"
        );

        // The exact name wins when both are present.
        std::fs::write(dir.join("BigWigs.toc"), b"## Title: BigWigs").unwrap();
        assert_eq!(toc_in(&dir).unwrap().file_name().unwrap(), "BigWigs.toc");

        // And a folder with no toc of its own has no fingerprint.
        let bare = dir.parent().unwrap().join("Empty");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(of_folder(&bare), None);

        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn an_unreadable_file_has_no_fingerprint_rather_than_zero() {
        // Zero is a value the hash can produce, so "could not read" must not
        // be expressible as a number that could match a real addon.
        assert_eq!(of_file(Path::new("/nonexistent/BigWigs.toc")), None);
    }
}
