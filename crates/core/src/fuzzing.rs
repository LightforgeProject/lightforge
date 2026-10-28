//! Entry points for the `cargo fuzz` targets in `fuzz/`, one per parser of
//! bytes that arrive from Blizzard's CDN, a store on disk, Agent or Steam.
//!
//! Compiled only under `--cfg fuzzing`, which `cargo fuzz` sets. Where a
//! format carries its own checksums, they are recomputed over the fuzzed
//! input first: random bytes never match an MD5 or Jenkins hash, and the
//! parser would never get past the check.

use crate::tact::{archive, encoding::Encoding, idx, manifest, productdb, verify};

pub fn archive_index(bytes: &[u8]) {
    let mut sealed = bytes.to_vec();
    archive::try_seal(&mut sealed);
    let _ = archive::ArchiveIndex::parse(&sealed, "fuzz");
    let _ = archive::FileIndex::parse(&sealed);
}

pub fn idx(bytes: &[u8]) {
    let mut fixed = bytes.to_vec();
    fix_idx_hashes(&mut fixed);
    let Ok(index) = idx::Index::parse(&fixed) else {
        return;
    };
    if let Ok(written) = index.to_bytes() {
        idx::Index::parse(&written).expect("an index we wrote parses");
    }
}

fn fix_idx_hashes(bytes: &mut [u8]) {
    use tact_parser::jenkins3::hashlittle;

    let Some(data_size) = le_u32(bytes, 0) else {
        return;
    };
    let body_end = 8 + data_size;
    if data_size < 8 || bytes.len() < body_end {
        return;
    }
    let hash = hashlittle(&bytes[8..body_end], 0);
    bytes[4..8].copy_from_slice(&hash.to_le_bytes());

    let ds = idx::data_offset((data_size - 8) / 8);
    let Some(section) = le_u32(bytes, ds) else {
        return;
    };
    let Some(entries) = bytes.get(ds + 8..ds + 8 + section) else {
        return;
    };
    let hash = idx::entries_hash(entries);
    bytes[ds + 4..ds + 8].copy_from_slice(&hash.to_le_bytes());
}

fn le_u32(bytes: &[u8], at: usize) -> Option<usize> {
    let word = bytes.get(at..at + 4)?.try_into().ok()?;
    usize::try_from(u32::from_le_bytes(word)).ok()
}

pub fn idx_file_name(bytes: &[u8]) {
    if let Ok(name) = std::str::from_utf8(bytes) {
        let _ = idx::parse_file_name(name);
    }
}

pub fn encoding(bytes: &[u8]) {
    let Ok(encoding) = Encoding::parse(bytes) else {
        return;
    };
    // A repeated ckey is allowed; `get` returns any one of them.
    if let Some(first) = encoding.entries().first() {
        assert_eq!(encoding.get(&first.ckey).map(|e| e.ckey), Some(first.ckey));
    }
}

pub fn manifest(bytes: &[u8]) {
    let tags = manifest::TagSet::parse("Windows x86_64 enUS");
    if let Ok(install) = manifest::InstallManifest::parse(bytes) {
        let _ = install.select(&tags);
    }
    if let Ok(download) = manifest::DownloadManifest::parse(bytes) {
        let _ = download.select(&tags);
        let _ = download.optional_names(&tags);
    }
}

pub fn blte(bytes: &[u8]) {
    let _ = crate::tact::blte::decode(bytes);

    // The table hashes to the key on a real file; without that the walk stops
    // at its first check.
    let header_size = bytes.get(4..8).map_or(0, |b| u32::from_be_bytes(b.try_into().expect("4 bytes")));
    let hashed = usize::try_from(header_size)
        .ok()
        .filter(|&n| n > 0)
        .and_then(|n| bytes.get(..n))
        .unwrap_or(bytes);
    let mut key = [0u8; idx::KEY_SIZE];
    key.copy_from_slice(&md5::compute(hashed).0[..idx::KEY_SIZE]);
    let _ = verify::blte_fault(bytes, &key);
}

pub fn productdb_decode(bytes: &[u8]) {
    let _ = productdb::decode(bytes);
}

pub fn productdb_splice(bytes: &[u8]) {
    let install = productdb::ProductInstall {
        product: "wow_classic_era".into(),
        install_path: "C:/Program Files (x86)/World of Warcraft".into(),
        region: "eu".into(),
        locale: "enUS".into(),
        account_country: "AUT".into(),
        geo_country: "AT".into(),
        flavor_dir: "_classic_era_".into(),
        version: "1.15.8.63829".into(),
        build_key: "7d8ae4cd9c1f639a3ea04fd47e21c746".into(),
        tags: "Windows x86_64 EU? enUS speech?:Windows x86_64 EU? enUS text?".into(),
        key_ring: None,
        installed_at_ms: 0,
        family: "wow".into(),
    };
    let Some(spliced) = productdb::splice(bytes, std::slice::from_ref(&install)) else {
        return;
    };
    let found = productdb::decode(&spliced);
    assert!(
        found.iter().any(|i| i.product == install.product),
        "a spliced record decodes",
    );
}

pub fn steam_libraryfolders(bytes: &[u8]) {
    let _ = crate::steam::parse_library_paths(&String::from_utf8_lossy(bytes));
}

pub fn steam_loginusers(bytes: &[u8]) {
    let _ = crate::steam_shortcut::most_recent(&String::from_utf8_lossy(bytes));
}

pub fn steam_shortcuts(bytes: &[u8]) {
    let Some(root) = crate::steam_shortcut::parse(bytes) else {
        return;
    };
    let written = crate::steam_shortcut::render(&root);
    assert_eq!(crate::steam_shortcut::parse(&written), Some(root), "shortcuts.vdf round-trips");
}
