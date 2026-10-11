//! Recognising an addon by its bytes, so a hand-installed one can be adopted.
//!
//! An addon Lightforge did not install has no provider and no id, so it can
//! be listed and nothing more, no update, no channel, no removal. CurseForge
//! solves this for its own client by publishing a **fingerprint** for every
//! module (every addon folder) of every file it hosts: hash what is on disk,
//! ask which project those hashes belong to, and an unmanaged folder becomes
//! a managed one.
//!
//! The hash is MurmurHash2 (32-bit, Austin Appleby's original) with seed `1`,
//! over the file's bytes **with whitespace removed**: tab, line feed,
//! carriage return and space are dropped before hashing. That last part is
//! what makes the fingerprint survive a checkout that rewrote line endings,
//! which is the whole reason it is not a plain checksum.
//!
//! **The algorithm is exact or it is useless.** There is no partial credit: a
//! fingerprint that is one byte off matches nothing at all, and the failure
//! looks identical to "this addon is not on CurseForge". So the two decisions
//! it rests on are separated deliberately, [`murmur2`] is the published
//! algorithm and is tested against its own reference, while [`of_file`] is
//! the part that says *which bytes go in*, verified against fingerprints
//! CurseForge itself published for an addon we installed from it.

use std::path::Path;

/// The one file WoW loads from an addon folder without being told to.
const IMPLICITLY_LOADED: &str = "Bindings.xml";

/// Seed CurseForge hashes with. Not a choice of ours.
const SEED: u32 = 1;

/// Bytes dropped before hashing: tab, line feed, carriage return, space.
const WHITESPACE: [u8; 4] = [9, 10, 13, 32];

/// MurmurHash2, 32-bit, as published, no normalisation of its own.
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
    let (chunks, tail) = bytes.as_chunks::<4>();

    for chunk in chunks {
        let mut k = u32::from_le_bytes(*chunk);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        hash = hash.wrapping_mul(M);
        hash ^= k;
    }

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

/// The fingerprint of an addon folder, as CurseForge computes it.
///
/// Reverse-engineered against 34 folders from two unrelated addons whose
/// published fingerprints the catalog states, and it reproduces all 34. The
/// parts that are not guessable from the outside, in the order they bite:
///
/// 1. **Not every file counts.** The set is the folder's `.toc` files, the
///    paths those tocs *load*, and the files the client loads implicitly.
///    Only unconditionally, though: a line carrying a `[...]` clause,
///    `BossPrototype.lua [ExcludeLoadGameType vanilla]`: is out, and so is
///    a file nothing references. BigWigs has 338 files on disk and a
///    fingerprint over 19.
/// 2. **Each file is hashed, then the hashes are hashed.** Per file it is
///    [`of_bytes`]; the folder is those numbers written out as decimal,
///    concatenated, and hashed again.
/// 3. **Sorted by value, not by name.** Ascending numeric order of the file
///    hashes themselves, which is why no filename ordering ever matched.
///
/// `None` when the folder declares nothing, no toc, or a toc that loads
/// nothing that exists. An addon with no fingerprint cannot be adopted,
/// which is the honest outcome rather than hashing an arbitrary subset.
pub fn of_folder(dir: &Path) -> Option<u32> {
    let mut hashes: Vec<u32> = declared(dir)
        .iter()
        .filter_map(|path| of_file(path))
        .collect();
    if hashes.is_empty() {
        return None;
    }

    hashes.sort_unstable();
    let joined: String = hashes.iter().map(u32::to_string).collect();
    Some(murmur2(joined.as_bytes(), SEED))
}

/// The files a folder's tocs declare, plus the tocs themselves.
///
/// A `BTreeSet` so a file two tocs both load is counted once and the result
/// does not depend on directory order. Paths are Windows-style in a toc and
/// are read as relative to the addon folder; anything that escapes it, or
/// that is not actually there, is dropped rather than followed.
fn declared(dir: &Path) -> std::collections::BTreeSet<std::path::PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };

    let tocs: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toc"))
        })
        .collect();

    // Loaded by the client from an addon folder whether a toc mentions it or
    // not, so CurseForge counts it and a toc-only reading misses it. This
    // was the difference between 32 of 34 folders matching and all 34.
    let bindings = dir.join(IMPLICITLY_LOADED);
    if bindings.is_file() {
        out.insert(bindings);
    }

    for toc in tocs {
        let Ok(text) = std::fs::read_to_string(&toc) else { continue };
        out.insert(toc.clone());

        for line in text.lines() {
            let line = line.trim();
            // Metadata, blanks, and anything load-conditional.
            if line.is_empty() || line.starts_with('#') || line.contains('[') {
                continue;
            }
            let Some(path) = under(dir, line) else { continue };
            if path.is_file() {
                out.insert(path);
            }
        }
    }
    out
}

/// A toc's `Libs\AceGUI-3.0\AceGUI-3.0.lua` as a path inside the folder.
///
/// Refuses anything that would leave it. A toc is a text file an addon ships,
/// so it is input like any other, and `..\..\..\something` must resolve to
/// nothing rather than to a file outside the addon.
fn under(dir: &Path, line: &str) -> Option<std::path::PathBuf> {
    let mut path = dir.to_path_buf();
    for part in line.split(['\\', '/']) {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return None;
        }
        path.push(part);
    }
    (path != dir).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values for MurmurHash2, computed from Appleby's own C with
    /// the same inputs. These pin the transcription, the constants, the
    /// little-endian read, the fall-through tail and the final avalanche,
    /// independently of what CurseForge then does with it.
    #[test]
    fn murmur2_matches_the_reference_implementation() {
        // Seed zero over nothing is the one input that falls through every
        // multiply unchanged, so it is zero, and the only case where a
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
    fn only_what_the_toc_loads_unconditionally_is_fingerprinted() {
        let dir = std::env::temp_dir().join("lightforge-fingerprint-declared/BigWigs_Core");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(dir.join("Libs")).unwrap();

        std::fs::write(
            dir.join("BigWigs_Core.toc"),
            "## Interface: 110005\n\
             ## Title: Core\n\
             \n\
             Constants.lua\n\
             BossPrototype.lua [ExcludeLoadGameType vanilla]\n\
             Libs\\Ace.lua\n\
             Missing.lua\n",
        )
        .unwrap();
        for name in ["Constants.lua", "BossPrototype.lua", "Vestigial.xml"] {
            std::fs::write(dir.join(name), format!("-- {name}")).unwrap();
        }
        std::fs::write(dir.join("Libs/Ace.lua"), b"-- ace").unwrap();

        let declared = declared(&dir);
        let names: Vec<String> = declared
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            names,
            ["BigWigs_Core.toc", "Constants.lua", "Libs/Ace.lua"],
            "the toc itself and what it loads without a condition — a \
             conditional line, a file nothing references, and a line naming \
             a file that is not there are all out"
        );

        // A folder with nothing declared has no fingerprint rather than a
        // hash of whatever happened to be lying in it.
        let bare = dir.parent().unwrap().join("Empty");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(of_folder(&bare), None);

        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn a_toc_cannot_reach_outside_its_own_folder() {
        let root = std::env::temp_dir().join("lightforge-fingerprint-escape");
        let dir = root.join("Addon");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.join("secret.lua"), b"-- not yours").unwrap();

        // A toc is a text file the addon ships, so it is untrusted input.
        assert_eq!(under(&dir, r"..\secret.lua"), None);
        assert_eq!(under(&dir, "../secret.lua"), None);
        assert_eq!(under(&dir, r"C:\Windows\win.ini"), None);
        assert_eq!(under(&dir, ""), None);
        assert_eq!(under(&dir, r"Libs\Ace.lua"), Some(dir.join("Libs").join("Ace.lua")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_file_has_no_fingerprint_rather_than_zero() {
        // Zero is a value the hash can produce, so "could not read" must not
        // be expressible as a number that could match a real addon.
        assert_eq!(of_file(Path::new("/nonexistent/BigWigs.toc")), None);
    }
}
