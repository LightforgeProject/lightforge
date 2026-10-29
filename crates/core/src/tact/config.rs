//! Build and CDN configuration files.
//!
//! The one part of an install not inside CASC: `key = value` text under
//! `Data/config`, by content hash, reachable from `.build.info`'s build key.
//!
//! ```text
//! Data/config/97/12/9712ae9c84cc54111e9e3ddf9a6b8019
//!             ^^ ^^ first two byte-pairs of the hash
//! ```
//!
//! The build config names the encoding manifest by both keys; the encoding
//! key reads it straight from storage, without an encoding lookup.

use std::path::{Path, PathBuf};

use super::storage::EKey;

/// A 16-byte content key: the MD5 of a file's decoded bytes.
pub type CKey = [u8; 16];

/// A manifest named by both of its keys: `ekey` addresses storage, `ckey` is
/// how the rest of the pipeline names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPair {
    pub ckey: CKey,
    /// Absent when the config lists only a content key, as it does for `root`.
    pub ekey: Option<EKey>,
}

/// The fields of a build config Lightforge actually uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildConfig {
    /// e.g. `WOW-69814patch12.1.0_Retail`.
    pub build_name: Option<String>,
    /// TACT product code the build belongs to, e.g. `wow`.
    pub build_uid: Option<String>,
    /// Maps content keys to encoding keys. Always carries both keys.
    pub encoding: KeyPair,
    /// Maps file ids to content keys. Listed by content key alone.
    pub root: KeyPair,
    pub install: Option<KeyPair>,
    pub download: Option<KeyPair>,
    pub size: Option<KeyPair>,
}

impl BuildConfig {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config = tact_parser::config::ConfigFile::parse(text)
            .map_err(|e| ConfigError::Malformed(e.to_string()))?;

        let pair = |key: &str| -> Option<KeyPair> {
            // `key = <ckey>` or `key = <ckey> <ekey>`; `key-size` lines are sizes.
            let value = config.values.get(key)?;
            let mut hashes = value.split_whitespace().filter_map(|h| parse_hash(h).ok());
            Some(KeyPair { ckey: hashes.next()?, ekey: hashes.next() })
        };

        Ok(Self {
            build_name: config.values.get("build-name").cloned(),
            build_uid: config.values.get("build-uid").cloned(),
            encoding: pair("encoding").ok_or(ConfigError::MissingKey("encoding"))?,
            root: pair("root").ok_or(ConfigError::MissingKey("root"))?,
            install: pair("install"),
            download: pair("download"),
            size: pair("size"),
        })
    }

    /// Read the build config named by `hash` out of a config directory.
    pub fn load(config_dir: impl AsRef<Path>, hash: &str) -> Result<Self, ConfigError> {
        let path = config_path(config_dir.as_ref(), hash)?;
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        Self::parse(&text)
    }
}

/// The archives and archive group a CDN config lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnConfig {
    pub archives: Vec<String>,
    /// The `archives-index-size` list, **not aligned with `archives`**:
    /// pairing by position matched 2-4% on three real configs. Indices are
    /// checked by their geometry instead.
    pub archives_index_size: Vec<u64>,
    /// Present in the config but **not fetchable** (403). Agent builds it
    /// locally from the per-archive indices.
    pub archive_group: Option<String>,
    pub file_index: Option<String>,
    pub file_index_size: Option<u64>,
}

impl CdnConfig {
    /// Each archive index to fetch. The size is always `None`, see
    /// [`CdnConfig::archives_index_size`].
    pub fn archive_indices(&self) -> Vec<(&str, Option<u64>)> {
        self.archives.iter().map(|a| (a.as_str(), None)).collect()
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config = tact_parser::config::ConfigFile::parse(text)
            .map_err(|e| ConfigError::Malformed(e.to_string()))?;
        let list = |key: &str| -> Vec<String> {
            config
                .values
                .get(key)
                .map(|v| v.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let sizes = |key: &str| -> Vec<u64> {
            config
                .values
                .get(key)
                .map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect())
                .unwrap_or_default()
        };

        Ok(Self {
            archives: list("archives"),
            archives_index_size: sizes("archives-index-size"),
            archive_group: config.values.get("archive-group").cloned(),
            file_index: config.values.get("file-index").cloned(),
            file_index_size: config
                .values
                .get("file-index-size")
                .and_then(|v| v.split_whitespace().next())
                .and_then(|n| n.parse().ok()),
        })
    }

    pub fn load(config_dir: impl AsRef<Path>, hash: &str) -> Result<Self, ConfigError> {
        let path = config_path(config_dir.as_ref(), hash)?;
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        Self::parse(&text)
    }
}

/// `<dir>/<h0h1>/<h2h3>/<hash>`: the fan-out CASC and the CDN store configs under.
pub fn config_path(dir: &Path, hash: &str) -> Result<PathBuf, ConfigError> {
    if hash.len() < 4 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ConfigError::BadHash(hash.to_owned()));
    }
    Ok(dir.join(&hash[0..2]).join(&hash[2..4]).join(hash))
}

/// Put a config's bytes into `Data/config`, where the client looks first
/// (missing, it fetches them: 1.7 s on `wow_classic_beta`). An existing file
/// is left alone: same hash, same bytes, maybe open.
pub fn store(data_dir: &Path, hash: &str, bytes: &[u8]) -> Result<PathBuf, ConfigError> {
    let path = config_path(&data_dir.join("config"), hash)?;
    if path.is_file() {
        return Ok(path);
    }
    // Atomic: the client reads this directory while it runs, and a
    // half-written config is worse than an absent one, which it knows how to fetch.
    crate::fsutil::write_atomic(&path, bytes).map_err(io(&path))?;
    Ok(path)
}

/// Parse 32 hex characters into 16 bytes.
pub fn parse_hash(hex: &str) -> Result<[u8; 16], ConfigError> {
    if hex.len() != 32 {
        return Err(ConfigError::BadHash(hex.to_owned()));
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| ConfigError::BadHash(hex.to_owned()))?;
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config is malformed: {0}")]
    Malformed(String),

    #[error("build config has no `{0}` entry")]
    MissingKey(&'static str),

    #[error("`{0}` is not a content hash")]
    BadHash(String),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConfigError + '_ {
    move |source| ConfigError::Io { path: path.to_path_buf(), source }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::hex;

    /// Trimmed verbatim from a real 12.1.0 retail install, keeping the shapes
    /// that matter: a one-hash `root`, two-hash `encoding`, and the `-size`
    /// siblings that must not be mistaken for hashes.
    const REAL_BUILD: &str = "\
# Build Configuration

root = 669be7c403ea35a20f73ea7effcaa544
install = 833bce183913ff7c2281e37bd6ad0ae1 a52ba5e9a40fd8d675dd625900806fdd
install-size = 23659 22716
download = 3bafe7e01ef12cc5bc5c63bfd4f8c4da 0a2648dbfb17cc47b8c935890f4499bd
download-size = 74339670 63235259
encoding = bdb644bbe038d17b9ad370d8a855f305 e144ce500f663d0ef512720d96553f20
encoding-size = 199496988 187471520
build-name = WOW-69814patch12.1.0_Retail
build-uid = wow
build-product = WoW
";

    #[test]
    fn a_real_build_config_yields_both_encoding_keys() {
        let build = BuildConfig::parse(REAL_BUILD).unwrap();
        assert_eq!(build.build_name.as_deref(), Some("WOW-69814patch12.1.0_Retail"));
        assert_eq!(build.build_uid.as_deref(), Some("wow"));

        // Encoding is the one entry that must carry an ekey: it is read from
        // local storage before any ckey can be resolved.
        assert_eq!(hex(&build.encoding.ckey), "bdb644bbe038d17b9ad370d8a855f305");
        assert_eq!(
            build.encoding.ekey.map(|e| hex(&e)).as_deref(),
            Some("e144ce500f663d0ef512720d96553f20")
        );

        // Root lists a content key only, so it has to go through encoding.
        assert_eq!(hex(&build.root.ckey), "669be7c403ea35a20f73ea7effcaa544");
        assert_eq!(build.root.ekey, None);

        assert!(build.install.unwrap().ekey.is_some());
        assert!(build.download.unwrap().ekey.is_some());
        assert_eq!(build.size, None);
    }

    #[test]
    fn size_lines_are_not_mistaken_for_hashes() {
        // `encoding-size` is also a two-value line, right beneath `encoding`.
        let build = BuildConfig::parse(REAL_BUILD).unwrap();
        assert_ne!(hex(&build.encoding.ckey), "199496988");
        assert_eq!(build.encoding.ckey.len(), 16);
    }

    #[test]
    fn a_config_without_encoding_is_refused() {
        let text = "root = 669be7c403ea35a20f73ea7effcaa544\n";
        assert!(matches!(
            BuildConfig::parse(text),
            Err(ConfigError::MissingKey("encoding"))
        ));
    }

    #[test]
    fn a_seeded_config_lands_where_the_client_looks_and_is_not_rewritten() {
        let dir = std::env::temp_dir()
            .join(format!("lightforge-config-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let data = dir.join("Data");
        let hash = "9712ae9c84cc54111e9e3ddf9a6b8019";

        let path = store(&data, hash, b"root = abc\n").unwrap();
        assert_eq!(path, data.join("config").join("97").join("12").join(hash));
        assert_eq!(std::fs::read(&path).unwrap(), b"root = abc\n");
        // Nothing half-written is left beside it.
        let siblings: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(siblings.len(), 1, "{siblings:?}");

        // A second pass leaves the file alone.
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(store(&data, hash, b"root = abc\n").unwrap(), path);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);

        assert!(store(&data, "nothex", b"x").is_err(), "a bad hash is not a path");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configs_are_addressed_by_their_hash_fan_out() {
        let path = config_path(Path::new("/Data/config"), "9712ae9c84cc54111e9e3ddf9a6b8019").unwrap();
        assert_eq!(
            path,
            Path::new("/Data/config/97/12/9712ae9c84cc54111e9e3ddf9a6b8019")
        );
        assert!(matches!(config_path(Path::new("/c"), "xyz"), Err(ConfigError::BadHash(_))));
        assert!(matches!(config_path(Path::new("/c"), "zzzzzzzz"), Err(ConfigError::BadHash(_))));
    }

    #[test]
    fn hashes_round_trip_through_hex() {
        let hash = "9712ae9c84cc54111e9e3ddf9a6b8019";
        assert_eq!(hex(&parse_hash(hash).unwrap()), hash);
        assert!(parse_hash("tooshort").is_err());
    }

    #[test]
    fn a_real_cdn_config_lists_its_archives() {
        let text = "# CDN Configuration\n\narchives = 0017a402f556fbece46c38dc431a2c9b 0095ab2008815140eadb0adfb4b033e5\narchive-group = df4cb2674525671a03db7e5bd7b83f2c\n";
        let cdn = CdnConfig::parse(text).unwrap();
        assert_eq!(cdn.archives.len(), 2);
        assert_eq!(cdn.archive_group.as_deref(), Some("df4cb2674525671a03db7e5bd7b83f2c"));
    }
}
