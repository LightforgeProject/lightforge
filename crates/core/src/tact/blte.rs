//! BLTE, the wrapper around every file on the CDN and in a CASC archive.
//!
//! Ours rather than `blte` 0.4.3's, which trusts what it decodes: it
//! allocates whatever the chunk table declares (141 GiB for one fuzzed table,
//! and a failed allocation aborts the process), reads zlib output without a
//! bound, and misreads the encrypted chunk header. Every caller decodes before
//! it can check the content key, so the input here is untrusted.
//!
//! ```text
//! "BLTE"          4 bytes
//! header_size     u32 BE   0 means one chunk: the rest of the file
//! flags           u8       0x0f on all 1,043,369 files of a real store
//! chunk_count     u24 BE
//! chunk_count x   compressed u32 BE, decompressed u32 BE, md5 16 bytes
//! <chunks>        each starts with its mode byte
//! ```
//!
//! Modes in that store: `Z` (1,338,676 chunks), `N` (248,174) and `E`
//! (5,219, under 353 keys, none of them public). `4` and `F` never occur.

use std::io::Read;

/// Most a file may decode to. Retail's encoding manifest, the largest file a
/// build has, is 191 MiB.
pub const MAX_DECODED_BYTES: u64 = 1 << 30;

pub fn decode(encoded: &[u8]) -> Result<Vec<u8>, BlteError> {
    if encoded.get(..4) != Some(b"BLTE") {
        return Err(BlteError::NotBlte);
    }
    let header_size = be32(encoded, 4).ok_or(BlteError::Truncated)? as usize;

    if header_size == 0 {
        let mut out = Vec::new();
        decode_chunk(0, &encoded[8..], MAX_DECODED_BYTES, &mut out)?;
        if out.len() as u64 > MAX_DECODED_BYTES {
            return Err(BlteError::TooLarge(out.len() as u64));
        }
        return Ok(out);
    }

    let table = encoded.get(..header_size).ok_or(BlteError::Truncated)?;
    let stride = match table.get(8) {
        Some(0x0f) => 24,
        Some(0x10) => 40,
        Some(&flags) => return Err(BlteError::UnknownFlags(flags)),
        None => return Err(BlteError::Truncated),
    };
    let count = table.get(9..12).map_or(0, |c| usize::from(c[0]) << 16 | usize::from(c[1]) << 8 | usize::from(c[2]));
    if count == 0 || 12 + count * stride != header_size {
        return Err(BlteError::BadTable { header_size, count });
    }

    let rows: Vec<&[u8]> = table[12..].chunks_exact(stride).collect();
    let declared: u64 = rows.iter().map(|row| u64::from(be32(row, 4).expect("inside the table"))).sum();
    if declared > MAX_DECODED_BYTES {
        return Err(BlteError::TooLarge(declared));
    }

    #[allow(clippy::cast_possible_truncation, reason = "at most MAX_DECODED_BYTES")]
    let mut out = Vec::with_capacity(declared as usize);
    let mut at = header_size;
    for (index, row) in rows.iter().enumerate() {
        let compressed = be32(row, 0).expect("inside the table") as usize;
        let decompressed = u64::from(be32(row, 4).expect("inside the table"));
        let chunk = encoded.get(at..at + compressed).ok_or(BlteError::Truncated)?;
        if md5::compute(chunk).0 != row[8..24] {
            return Err(BlteError::ChunkChecksum { index });
        }

        let start = out.len();
        decode_chunk(index, chunk, decompressed, &mut out)?;
        let got = (out.len() - start) as u64;
        if got != decompressed {
            return Err(BlteError::SizeMismatch { index, declared: decompressed, got });
        }
        at += compressed;
    }
    Ok(out)
}

/// Append one chunk's content to `out`, at most `limit + 1` bytes of it: one
/// past is enough for the caller to see it overran.
fn decode_chunk(index: usize, chunk: &[u8], limit: u64, out: &mut Vec<u8>) -> Result<(), BlteError> {
    let Some((&mode, body)) = chunk.split_first() else {
        return Err(BlteError::Truncated);
    };
    match mode {
        b'N' => out.extend_from_slice(body),
        b'Z' => {
            flate2::read::ZlibDecoder::new(body)
                .take(limit + 1)
                .read_to_end(out)
                .map_err(|source| BlteError::Zlib { index, source })?;
        }
        // u8 key name size (8), then the key name, little-endian.
        b'E' => {
            let key = body.get(1..9).map_or(0, |name| u64::from_le_bytes(name.try_into().expect("8 bytes")));
            return Err(BlteError::Encrypted { index, key });
        }
        other => return Err(BlteError::UnsupportedMode { index, mode: other }),
    }
    Ok(())
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[derive(Debug, thiserror::Error)]
pub enum BlteError {
    #[error("not a BLTE payload")]
    NotBlte,

    #[error("BLTE payload is truncated")]
    Truncated,

    #[error("BLTE chunk table has unknown flags {0:#04x}")]
    UnknownFlags(u8),

    #[error("BLTE chunk table of {header_size} bytes cannot hold {count} chunks")]
    BadTable { header_size: usize, count: usize },

    #[error("BLTE payload decodes to {0} bytes or more, past the 1 GiB limit")]
    TooLarge(u64),

    #[error("BLTE chunk {index} fails its checksum")]
    ChunkChecksum { index: usize },

    #[error("BLTE chunk {index} decodes to {got} bytes, its table says {declared}")]
    SizeMismatch { index: usize, declared: u64, got: u64 },

    #[error("BLTE chunk {index} is encrypted with key {key:016x}")]
    Encrypted { index: usize, key: u64 },

    #[error("BLTE chunk {index} has unsupported mode {mode:#04x}")]
    UnsupportedMode { index: usize, mode: u8 },

    #[error("BLTE chunk {index} is not valid zlib")]
    Zlib {
        index: usize,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(chunks: &[(&[u8], u32)]) -> Vec<u8> {
        let header_size = 12 + 24 * chunks.len();
        let mut out = b"BLTE".to_vec();
        out.extend_from_slice(&(header_size as u32).to_be_bytes());
        out.push(0x0f);
        out.extend_from_slice(&(chunks.len() as u32).to_be_bytes()[1..]);
        for (chunk, decompressed) in chunks {
            out.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
            out.extend_from_slice(&decompressed.to_be_bytes());
            out.extend_from_slice(&md5::compute(chunk).0);
        }
        for (chunk, _) in chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    fn zlib(content: &[u8]) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(vec![b'Z'], flate2::Compression::default());
        std::io::Write::write_all(&mut z, content).unwrap();
        z.finish().unwrap()
    }

    #[test]
    fn chunks_of_mixed_modes_decode_in_order() {
        let encoded = table(&[(&zlib(b"first, "), 7), (b"Nsecond", 6)]);
        assert_eq!(decode(&encoded).unwrap(), b"first, second");
    }

    #[test]
    fn a_single_chunk_file_decodes() {
        let encoded = ::blte::compress_data_single(b"one chunk".to_vec(), ::blte::CompressionMode::ZLib, None).unwrap();
        assert_eq!(decode(&encoded).unwrap(), b"one chunk");
    }

    #[test]
    fn a_table_declaring_too_much_is_refused_before_allocating() {
        let encoded = table(&[(b"Nx", u32::MAX), (b"Ny", u32::MAX)]);
        assert!(matches!(decode(&encoded), Err(BlteError::TooLarge(_))));
    }

    #[test]
    fn zlib_output_past_its_declared_size_is_refused() {
        let encoded = table(&[(&zlib(&[0u8; 4096]), 16)]);
        assert!(matches!(decode(&encoded), Err(BlteError::SizeMismatch { index: 0, declared: 16, got: 17 })));
    }

    #[test]
    fn a_chunk_that_fails_its_checksum_is_refused() {
        let mut encoded = table(&[(b"Nabc", 3)]);
        *encoded.last_mut().unwrap() = b'x';
        assert!(matches!(decode(&encoded), Err(BlteError::ChunkChecksum { index: 0 })));
    }

    #[test]
    fn an_encrypted_chunk_names_its_key() {
        let mut chunk = vec![b'E', 8];
        chunk.extend_from_slice(&0xfa50_5078_126a_cb3e_u64.to_le_bytes());
        chunk.extend_from_slice(&[4, 1, 2, 3, 4, b'S', 0xaa]);
        let encoded = table(&[(&chunk, 1)]);
        assert!(matches!(decode(&encoded), Err(BlteError::Encrypted { index: 0, key: 0xfa50_5078_126a_cb3e })));
    }

    #[test]
    fn an_encrypted_chunk_after_plain_ones_is_still_reported() {
        let encoded = table(&[(b"Nab", 2), (&zlib(b"cd"), 2), (b"E\x08", 1)]);
        assert!(matches!(decode(&encoded), Err(BlteError::Encrypted { index: 2, .. })));
    }
}
