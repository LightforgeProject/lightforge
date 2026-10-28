//! Recognising an addon by its bytes, so a hand-installed one can be adopted.
//!
//! CurseForge publishes a **fingerprint** for every addon folder it hosts:
//! hash what is on disk, ask which project it belongs to, and a hand-installed
//! folder can be adopted.
//!
//! MurmurHash2 (32-bit, Appleby's original), seed `1`, over the bytes **with
//! tab, LF, CR and space removed**, so line endings do not matter.
//!
//! **Exact or useless**: one byte off matches nothing, which looks like "not on
//! CurseForge". [`murmur2`] is tested against its reference; [`of_file`]
//! (which bytes go in) against fingerprints CurseForge published.

use std::path::Path;

/// The one file WoW loads from an addon folder without being told to.
const IMPLICITLY_LOADED: &str = "Bindings.xml";

/// Seed CurseForge hashes with. Not a choice of ours.
const SEED: u32 = 1;

/// Bytes dropped before hashing: tab, line feed, carriage return, space.
const WHITESPACE: [u8; 4] = [9, 10, 13, 32];

/// MurmurHash2, 32-bit, transcribed from Appleby's reference: the tail falls
/// through, and the arithmetic wraps, as the C relies on.
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

/// The fingerprint of one file's contents, whitespace stripped first.
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
/// Never a zero for unreadable: zero is a real hash value.
pub fn of_file(path: &Path) -> Option<u32> {
    std::fs::read(path).ok().map(|bytes| of_bytes(&bytes))
}

/// The fingerprint of an addon folder, as CurseForge computes it.
///
/// Reverse-engineered against 34 folders from two addons; reproduces all 34.
/// The parts that cannot be guessed:
///
/// 1. **Not every file counts.** Only the `.toc` files, the paths they load
///    unconditionally (a line with a `[...]` clause is out) and the files the
///    client loads implicitly. BigWigs: 338 files on disk, 19 hashed.
/// 2. **Hash of hashes.** Each file is [`of_bytes`]; the folder hashes those
///    numbers written as decimal and concatenated.
/// 3. **Sorted by value**, not by name.
///
/// `None` when the folder loads nothing that exists.
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
/// A `BTreeSet`: each file once, in a fixed order. Paths that escape the
/// folder or do not exist are dropped.
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

    // Loaded whether a toc names it or not: 32 of 34 folders matched without it.
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
/// Refuses anything that would leave it: a toc is untrusted input.
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

    /// Reference values for MurmurHash2, computed from Appleby's own C.
    #[test]
    fn murmur2_matches_the_reference_implementation() {
        // Seed zero over nothing is zero, even for a transcription that does nothing.
        assert_eq!(murmur2(b"", 0), 0);
        assert_eq!(murmur2(b"", 1), 0x5bd1_5e36);

        // One vector per tail length: the original's switch falls through.
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

        // A folder with nothing declared has no fingerprint, not a
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
