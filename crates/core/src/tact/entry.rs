//! The 30-byte header in front of every file inside a local `data.NNN`.
//!
//! CDN archives store bare BLTE files; local storage puts this header in
//! front, and neither of its checksums is a plain hash.
//!
//! ```text
//! 0x00  [u8; 16]  index key, padded to 16 bytes and byte-reversed
//! 0x10  u32 LE    total entry size, this header included
//! 0x14  u16       flags (zero on every entry observed)
//! 0x16  u32 LE    checksum A
//! 0x1a  u32 LE    checksum B
//! ```
//!
//! Checksum A is `hashlittle` over the first 0x16 bytes, seed `0x3D6BE971`.
//! Checksum B is a table-driven XOR fold that mixes in **where the entry
//! sits** (archive number and offset), so a header cannot be relocated.
//!
//! Verified on 12.1.0: 19,980 of 20,000 sampled headers rebuild byte for
//! byte from their index entry. The other 20, in `data.000` only, fill the
//! 16-byte key field instead of padding a 9-byte key; every manifest-reachable
//! file (357 of 357) is padded, so [`Header::build`] writes the padded form.
//!
//! Algorithm reference: <https://wowdev.wiki/CASC>.

use super::idx::CASC_ENTRY_HEADER;

/// Bytes of the key stored in an index entry, and mirrored in this header.
const KEY_BYTES: usize = 9;

/// Constants from Blizzard's implementation. Index 10 equals checksum A's
/// seed, a sign the table is right.
const TABLE: [u32; 16] = [
    0x0493_96b8, 0x72a8_2a9b, 0xee62_6cca, 0x9917_754f, 0x15de_40b1, 0xf5a8_a9b6, 0x421e_ac7e,
    0xa9d5_5c9a, 0x317f_d40c, 0x04fa_f80d, 0x3d6b_e971, 0x5293_3cfd, 0x27f6_4b7d, 0xc6f5_c11b,
    0xd575_7e3a, 0x6c38_8745,
];

/// Seed for checksum A.
const CHECKSUM_A_SEED: u32 = 0x3D6B_E971;

/// A parsed entry header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The truncated key, in the order indices store it.
    pub key: [u8; KEY_BYTES],
    /// Total bytes of this entry, header included.
    pub size: u32,
    pub flags: u16,
    pub checksum_a: u32,
    pub checksum_b: u32,
}

impl Header {
    /// Build the header for a file about to be written at a known location.
    /// `size` includes this header, as the index entry records it.
    pub fn build(key: &[u8; KEY_BYTES], size: u32, archive_id: u16, offset: u32) -> [u8; CASC_ENTRY_HEADER] {
        let mut out = [0u8; CASC_ENTRY_HEADER];

        // Right-padded to 16 bytes, then reversed: the key bytes end up last.
        for (i, byte) in key.iter().enumerate() {
            out[15 - i] = *byte;
        }
        out[0x10..0x14].copy_from_slice(&size.to_le_bytes());
        // Flags are zero on every entry in a real install.
        out[0x14..0x16].copy_from_slice(&0u16.to_le_bytes());

        let a = checksum_a(&out[..0x16]);
        out[0x16..0x1a].copy_from_slice(&a.to_le_bytes());

        // B covers the first 0x1a bytes, so A has to already be in place.
        let b = checksum_b(&out, archive_id, offset);
        out[0x1a..0x1e].copy_from_slice(&b.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < CASC_ENTRY_HEADER {
            return None;
        }
        let mut key = [0u8; KEY_BYTES];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = bytes[15 - i];
        }
        Some(Self {
            key,
            size: u32::from_le_bytes(bytes[0x10..0x14].try_into().ok()?),
            flags: u16::from_le_bytes(bytes[0x14..0x16].try_into().ok()?),
            checksum_a: u32::from_le_bytes(bytes[0x16..0x1a].try_into().ok()?),
            checksum_b: u32::from_le_bytes(bytes[0x1a..0x1e].try_into().ok()?),
        })
    }

    /// Whether both checksums match the header's own contents and position.
    pub fn verify(bytes: &[u8], archive_id: u16, offset: u32) -> bool {
        let Some(header) = Self::parse(bytes) else {
            return false;
        };
        header.checksum_a == checksum_a(&bytes[..0x16])
            && header.checksum_b == checksum_b(bytes, archive_id, offset)
    }

}

/// `hashlittle` over the header up to the checksum fields.
pub fn checksum_a(header_prefix: &[u8]) -> u32 {
    tact_parser::jenkins3::hashlittle(header_prefix, CHECKSUM_A_SEED)
}

/// The position-dependent checksum: `offset` folded with the archive number's
/// low two bits, the 32-bit word an index entry packs.
pub fn checksum_b(header: &[u8], archive_id: u16, archive_offset: u32) -> u32 {
    let offset = (archive_offset & 0x3fff_ffff) | ((u32::from(archive_id) & 3) << 30);
    let seed = offset.wrapping_add(CASC_ENTRY_HEADER as u32);
    let encoded = TABLE[(seed & 0xf) as usize] ^ seed;

    // Fold the first 0x1a bytes into four accumulator bytes, with which byte
    // receives which input rotated by the offset.
    let mut hashed = [0u8; 4];
    for (i, byte) in header.iter().take(0x1a).enumerate() {
        hashed[((i as u32).wrapping_add(offset) & 3) as usize] ^= *byte;
    }

    let encoded = encoded.to_le_bytes();
    let mut out = [0u8; 4];
    for (j, slot) in out.iter_mut().enumerate() {
        let i = (j as u32).wrapping_add(0x1a).wrapping_add(offset);
        *slot = hashed[(i & 3) as usize] ^ encoded[(i & 3) as usize];
    }
    u32::from_le_bytes(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real entry from a 12.1.0 install: the first file in bucket 00, which
    /// lives in `data.027` at offset 1049269890 and is 18355 bytes long.
    const REAL_KEY: [u8; KEY_BYTES] = [0x00, 0x00, 0x52, 0xe5, 0x14, 0xf8, 0x9d, 0x30, 0x2b];
    const REAL_HEADER: [u8; CASC_ENTRY_HEADER] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2b, 0x30, 0x9d, 0xf8, 0x14, 0xe5, 0x52, 0x00,
        0x00, 0xb3, 0x47, 0x00, 0x00, 0x00, 0x00, 0x6b, 0xa2, 0x1c, 0x91, 0x8b, 0x9d, 0x63, 0xe3,
    ];

    #[test]
    fn a_real_header_is_reproduced_byte_for_byte() {
        // Both checksums and the reversed key, against bytes Agent wrote.
        let built = Header::build(&REAL_KEY, 18_355, 27, 1_049_269_890);
        assert_eq!(built, REAL_HEADER);
    }

    #[test]
    fn a_real_header_parses_into_its_parts() {
        let header = Header::parse(&REAL_HEADER).unwrap();
        assert_eq!(header.key, REAL_KEY);
        assert_eq!(header.size, 18_355);
        assert_eq!(header.flags, 0);
        assert!(Header::verify(&REAL_HEADER, 27, 1_049_269_890));
    }

    #[test]
    fn the_key_field_is_reversed_not_merely_padded() {
        // The key bytes sit at the END of the field, reversed.
        assert_eq!(&REAL_HEADER[..7], &[0u8; 7]);
        assert_eq!(REAL_HEADER[15], REAL_KEY[0]);
        assert_eq!(REAL_HEADER[7], REAL_KEY[8]);
    }

    #[test]
    fn checksum_b_depends_on_where_the_entry_sits() {
        // The property that makes headers non-relocatable: same bytes, same
        // archive, one byte further along, different checksum.
        let a = Header::build(&REAL_KEY, 18_355, 27, 1_049_269_890);
        let b = Header::build(&REAL_KEY, 18_355, 27, 1_049_269_891);
        assert_ne!(a, b);
        assert_eq!(&a[..0x1a], &b[..0x1a], "only checksum B may differ");

        // And on the archive number, but only its low two bits. That is what
        // the index packs alongside the offset.
        let c = Header::build(&REAL_KEY, 18_355, 31, 1_049_269_890);
        assert_eq!(a, c, "archives 27 and 31 agree in their low two bits");
        let d = Header::build(&REAL_KEY, 18_355, 28, 1_049_269_890);
        assert_ne!(a, d);
    }

    #[test]
    fn verification_rejects_a_header_moved_somewhere_else() {
        assert!(Header::verify(&REAL_HEADER, 27, 1_049_269_890));
        assert!(!Header::verify(&REAL_HEADER, 27, 1_049_269_891));
        assert!(!Header::verify(&REAL_HEADER, 28, 1_049_269_890));

        let mut tampered = REAL_HEADER;
        tampered[0x10] ^= 0xff; // change the recorded size
        assert!(!Header::verify(&tampered, 27, 1_049_269_890));
    }

    #[test]
    fn a_built_header_round_trips_through_parse() {
        let key = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        let built = Header::build(&key, 4096, 5, 12_345);
        let parsed = Header::parse(&built).unwrap();
        assert_eq!(parsed.key, key);
        assert_eq!(parsed.size, 4096);
        assert!(Header::verify(&built, 5, 12_345));
    }

    #[test]
    fn the_key_field_is_padded_to_sixteen_not_filled() {
        // What the downloader writes, and what every manifest-reachable file
        // in a real install looks like: nine significant bytes, seven zeros.
        let built = Header::build(&REAL_KEY, 18_355, 27, 1_049_269_890);
        assert_eq!(&built[..7], &[0u8; 7]);

        // A wide entry, seen in data.000, parses fine and verifies fine; it
        // simply is not a shape we produce. Parsing must not assume padding.
        let mut wide = REAL_HEADER;
        wide[..7].copy_from_slice(&[0x76, 0xa0, 0x3d, 0xbd, 0xd7, 0x10, 0x73]);
        let parsed = Header::parse(&wide).unwrap();
        assert_eq!(parsed.key, REAL_KEY, "the low nine bytes still read out");
    }

    #[test]
    fn a_short_slice_is_rejected_rather_than_panicking() {
        assert!(Header::parse(&[0u8; 12]).is_none());
        assert!(!Header::verify(&[0u8; 12], 0, 0));
    }
}
