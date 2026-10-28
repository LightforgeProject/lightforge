use std::path::{Path, PathBuf};

use super::read::decode_install;
use super::wire::{double_field, message_field, read_varint, string_field, varint_field};
use super::{agent_tags, io, locale_from_tags, ProductDbError};
use crate::buildinfo::{self, BuildInfo};
use crate::product::ProductCode;

/// One installed product, as Agent records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductInstall {
    /// Product code, written to both `uid` and `product_code`; they differ
    /// only for Hearthstone, which Battle.net installs itself.
    pub product: String,
    /// Windows-visible install root, e.g. `C:/Program Files (x86)/World of Warcraft`.
    pub install_path: String,
    pub region: String,
    pub locale: String,
    /// Account country (`AUT`) and GeoIP country (`AT`), from the account.
    /// Empty when unknown.
    pub account_country: String,
    pub geo_country: String,
    /// Flavor subdirectory, e.g. `_classic_era_`.
    pub flavor_dir: String,
    pub version: String,
    pub build_key: String,
    pub tags: String,
    pub key_ring: Option<String>,
    /// Milliseconds since the epoch.
    pub installed_at_ms: u64,
    /// `wow` for every WoW flavor.
    pub family: String,
}

impl ProductInstall {
    /// Build a record from what `.build.info` already says about a product.
    pub fn from_build_info(
        info: &BuildInfo,
        code: &ProductCode,
        install_path: impl Into<String>,
    ) -> Result<Self, ProductDbError> {
        let cell = |column: &str| {
            info.get(code, column)
                .map(str::to_owned)
                .map_err(|_| ProductDbError::MissingColumn(column.to_owned()))
        };

        let region = cell("Branch").unwrap_or_else(|_| "us".to_owned());
        let tags = cell("Tags").unwrap_or_default();
        let key_ring = info
            .get(code, "KeyRing")
            .ok()
            .map(str::to_owned)
            .filter(|k| !k.is_empty());

        Ok(Self {
            product: code.as_str().to_owned(),
            install_path: install_path.into(),
            region,
            locale: locale_from_tags(&tags).unwrap_or_else(|| "enUS".to_owned()),
            account_country: String::new(),
            geo_country: String::new(),
            flavor_dir: code.install_dir().into_owned(),
            version: cell(buildinfo::COL_VERSION)?,
            build_key: cell(buildinfo::COL_BUILD_KEY)?,
            tags,
            key_ring,
            installed_at_ms: now_ms(),
            family: "wow".to_owned(),
        })
    }

    /// Rewrite the tags as Agent writes them,
    /// `Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:… text?`. Selects
    /// the same files as the plain form, but matches Agent's bytes.
    pub fn with_agent_tags(mut self) -> Self {
        self.tags = agent_tags(
            &self.region,
            &self.locale,
            &self.account_country,
            &self.geo_country,
        );
        self
    }

    /// Set the countries Agent would otherwise fill in from the account.
    pub fn with_countries(
        mut self,
        account: impl Into<String>,
        geo: impl Into<String>,
    ) -> Self {
        self.account_country = account.into();
        self.geo_country = geo.into();
        self
    }

    pub(super) fn encode(&self) -> Vec<u8> {
        let mut settings = Vec::new();
        string_field(&mut settings, 1, &self.install_path);
        string_field(&mut settings, 2, &self.region);
        varint_field(&mut settings, 3, 0);
        // 2 and 3 are constant across every record in a real file.
        varint_field(&mut settings, 4, 2);
        varint_field(&mut settings, 5, 3);
        string_field(&mut settings, 6, &self.locale);
        string_field(&mut settings, 7, &self.locale);

        let mut asset_locale = Vec::new();
        string_field(&mut asset_locale, 1, &self.locale);
        varint_field(&mut asset_locale, 2, 3);
        message_field(&mut settings, 8, &asset_locale);

        message_field(&mut settings, 9, &[]);
        message_field(&mut settings, 10, &[]);
        string_field(&mut settings, 11, &self.account_country);
        string_field(&mut settings, 12, &self.geo_country);
        string_field(&mut settings, 13, &self.flavor_dir);

        let mut base = Vec::new();
        varint_field(&mut base, 1, 1); // installed
        varint_field(&mut base, 2, 1); // playable
        varint_field(&mut base, 3, 1); // patch application complete
        varint_field(&mut base, 4, 0);
        varint_field(&mut base, 5, 0);
        string_field(&mut base, 7, &self.version);
        message_field(&mut base, 10, &[]);
        // The build key appears twice: active and installed.
        string_field(&mut base, 12, &self.build_key);
        string_field(&mut base, 14, &self.build_key);
        string_field(&mut base, 17, &self.tags);
        if let Some(key_ring) = &self.key_ring {
            string_field(&mut base, 19, key_ring);
        }
        varint_field(&mut base, 21, self.installed_at_ms);

        let mut cached = Vec::new();
        message_field(&mut cached, 1, &base);

        // Two timing stubs: real ones only describe the last operation.
        let mut timing = Vec::new();
        double_field(&mut timing, 1, 0.0);
        varint_field(&mut timing, 2, 0);
        varint_field(&mut timing, 3, 0);
        varint_field(&mut timing, 5, 0);
        message_field(&mut cached, 2, &timing);

        let mut stamp = Vec::new();
        double_field(&mut stamp, 1, 0.0);
        message_field(&mut cached, 3, &stamp);

        let mut install = Vec::new();
        string_field(&mut install, 1, &self.product);
        string_field(&mut install, 2, &self.product);
        message_field(&mut install, 3, &settings);
        message_field(&mut install, 4, &cached);
        string_field(&mut install, 6, &self.family);
        varint_field(&mut install, 7, 0);
        string_field(&mut install, 8, r#"{"user_install_package_settings":[]}"#);
        install
    }
}

/// Encode a whole `.product.db`.
pub fn encode(installs: &[ProductInstall]) -> Vec<u8> {
    let mut out = Vec::new();
    for install in installs {
        message_field(&mut out, 1, &install.encode());
    }
    out
}

/// Merge `installs` into an existing `product.db`, leaving every other record
/// exactly as it was found.
///
/// Spliced as raw bytes: [`decode`](super::decode) reads a few of dozens of
/// fields, and a rebuild would drop the rest. Unreadable input gives `None`.
pub fn splice(existing: &[u8], installs: &[ProductInstall]) -> Option<Vec<u8>> {
    rewrite(existing, installs, None)
}

/// Drop `product`'s record from an existing `product.db`, byte-preserving
/// like [`splice`].
pub fn without(existing: &[u8], product: &str) -> Option<Vec<u8>> {
    rewrite(existing, &[], Some(product))
}

/// [`splice`] and [`without`]: replace or add `installs`, drop `remove`.
fn rewrite(existing: &[u8], installs: &[ProductInstall], remove: Option<&str>) -> Option<Vec<u8>> {
    let encoded: Vec<(String, Vec<u8>)> = installs
        .iter()
        .map(|install| (install.product.clone(), install.encode()))
        .collect();
    merge(existing, &encoded, remove)
}

/// The byte-preserving merge behind [`splice`], [`without`] and [`graft`].
///
/// `records` are record *payloads*: field 1's contents, paired with the
/// product code they are for. A payload replaces the record of the same code
/// and is appended when there is none.
fn merge(
    existing: &[u8],
    records: &[(String, Vec<u8>)],
    remove: Option<&str>,
) -> Option<Vec<u8>> {
    /// One top-level field, kept exactly as it arrived.
    struct Item {
        /// Field number. `1` is a product record; a real registry also holds
        /// others after them, which are none of our business but must survive.
        field: u32,
        raw: Vec<u8>,
        /// Product code, when this is a record we could read one out of.
        code: Option<String>,
    }

    let mut items: Vec<Item> = Vec::new();
    for raw in raw_fields(existing)? {
        let code = match raw.field == 1 {
            true => raw.payload.and_then(decode_install).map(|i| i.product),
            false => None,
        };
        items.push(Item { field: raw.field, raw: raw.bytes.to_vec(), code });
    }

    if let Some(remove) = remove {
        items.retain(|item| item.code.as_deref() != Some(remove));
    }

    // Replace what we already own, and collect what has to be added.
    let mut fresh: Vec<Vec<u8>> = Vec::new();
    for (product, payload) in records {
        let mut encoded = Vec::new();
        message_field(&mut encoded, 1, payload);
        match items.iter_mut().find(|item| item.code.as_deref() == Some(product)) {
            Some(item) => item.raw = encoded,
            None => fresh.push(encoded),
        }
    }

    // New records go with the others, as Agent would place them.
    let after_records = items.iter().rposition(|item| item.field == 1).map_or(0, |at| at + 1);
    let mut out = Vec::new();
    for item in &items[..after_records] {
        out.extend_from_slice(&item.raw);
    }
    for record in &fresh {
        out.extend_from_slice(record);
    }
    for item in &items[after_records..] {
        out.extend_from_slice(&item.raw);
    }
    Some(out)
}

/// One field of a message, as the exact bytes it arrived as.
struct Raw<'a> {
    field: u32,
    /// Key, length and value. Everything this field occupied.
    bytes: &'a [u8],
    /// The value alone, for a length-delimited field.
    payload: Option<&'a [u8]>,
}

/// Split a message into its fields without interpreting them. `None` for
/// bytes that are not a field sequence: the caller then rewrites nothing.
fn raw_fields(bytes: &[u8]) -> Option<Vec<Raw<'_>>> {
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let start = at;
        let (key, used) = read_varint(bytes.get(at..)?)?;
        at += used;
        let field = u32::try_from(key >> 3).ok()?;

        let mut payload = None;
        match key & 7 {
            0 => at += read_varint(bytes.get(at..)?)?.1,
            1 => at = at.checked_add(8).filter(|end| *end <= bytes.len())?,
            2 => {
                let (len, used) = read_varint(bytes.get(at..)?)?;
                at += used;
                let len = usize::try_from(len).ok()?;
                let end = at.checked_add(len).filter(|end| *end <= bytes.len())?;
                payload = Some(bytes.get(at..end)?);
                at = end;
            }
            5 => at = at.checked_add(4).filter(|end| *end <= bytes.len())?,
            // Groups are long gone from the wire format.
            _ => return None,
        }
        found.push(Raw { field, bytes: bytes.get(start..at)?, payload });
    }
    Some(found)
}

/// One product record lifted whole out of another prefix's registry; opaque,
/// see [`lift`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub product: String,
    pub(super) payload: Vec<u8>,
}

/// Lift one product's record out of another `product.db`, with its install
/// path rewritten and every other byte kept.
///
/// For a game Lightforge does not manage, say a Diablo in a Lutris prefix:
/// Agent's own record carries the build key, install state and tags nothing
/// here can invent, so it is copied and only the path changes.
///
/// `None` when the file holds no record for `product`, or is not a registry.
pub fn lift(bytes: &[u8], product: &str, install_path: &str) -> Option<Record> {
    for raw in raw_fields(bytes)? {
        if raw.field != 1 {
            continue;
        }
        let payload = raw.payload?;
        if decode_install(payload).map(|i| i.product).as_deref() != Some(product) {
            continue;
        }
        return Some(Record {
            product: product.to_owned(),
            payload: with_install_path(payload, install_path)?,
        });
    }
    None
}

/// A record with its settings block's install path replaced, byte for byte
/// identical everywhere else.
fn with_install_path(record: &[u8], install_path: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut replaced = false;
    for raw in raw_fields(record)? {
        // Field 3 is the settings block, whose field 1 is the install path.
        if raw.field != 3 {
            out.extend_from_slice(raw.bytes);
            continue;
        }
        let mut settings = Vec::new();
        for field in raw_fields(raw.payload?)? {
            match field.field == 1 {
                true => string_field(&mut settings, 1, install_path),
                false => settings.extend_from_slice(field.bytes),
            }
        }
        message_field(&mut out, 3, &settings);
        replaced = true;
    }
    replaced.then_some(out)
}

/// Graft lifted records into Agent's registry, under the same rules as
/// [`register`]: nothing alive in the prefix, and a file that cannot be read is
/// left alone and reported as `false`.
pub fn graft(agent_dir: &Path, records: &[Record]) -> Result<bool, ProductDbError> {
    let payloads: Vec<(String, Vec<u8>)> = records
        .iter()
        .map(|r| (r.product.clone(), r.payload.clone()))
        .collect();
    let path = agent_dir.join("product.db");
    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(ProductDbError::Io { path, source }),
    };
    let Some(merged) = merge(&existing, &payloads, None) else {
        return Ok(false);
    };
    replace(&path, &existing, &merged)
}

/// Register `installs` in Agent's own registry, the list the client builds its
/// library from.
///
/// It decides whether a downloaded product shows Play or Install: the
/// per-install `.product.db` is only read by Locate. Only with nothing alive
/// in the prefix (Agent rewrites it on exit). An unreadable file is left
/// alone and reported as `false`.
pub fn register(agent_dir: &Path, installs: &[ProductInstall]) -> Result<bool, ProductDbError> {
    let path = agent_dir.join("product.db");
    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        // No registry yet: seeding one with only our products would tell the
        // client its own components are missing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(ProductDbError::Io { path, source }),
    };

    let Some(merged) = splice(&existing, installs) else {
        return Ok(false);
    };
    replace(&path, &existing, &merged)
}

/// Remove `product` from Agent's registry, so Battle.net stops listing a game
/// that is no longer on disk. The counterpart of [`register`], under the same
/// rules: nothing alive in the prefix, and a file it cannot read is left alone
/// and reported as `false`.
pub fn unregister(agent_dir: &Path, product: &str) -> Result<bool, ProductDbError> {
    let path = agent_dir.join("product.db");
    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(ProductDbError::Io { path, source }),
    };
    let Some(trimmed) = without(&existing, product) else {
        return Ok(false);
    };
    replace(&path, &existing, &trimmed)
}

/// Write a rewritten registry over the old one, atomically (Agent reads it
/// while running), unless nothing changed.
fn replace(path: &Path, existing: &[u8], merged: &[u8]) -> Result<bool, ProductDbError> {
    if merged == existing {
        return Ok(false);
    }
    crate::fsutil::write_atomic(path, merged).map_err(io(path))?;
    Ok(true)
}

/// Write `.product.db` into an install root, atomically.
pub fn write(install_root: &Path, installs: &[ProductInstall]) -> Result<PathBuf, ProductDbError> {
    let path = install_root.join(".product.db");
    crate::fsutil::write_atomic(&path, &encode(installs)).map_err(io(&path))?;
    Ok(path)
}

/// Put `installs` into an install root's `.product.db`, keeping every other
/// product's record already there.
///
/// Products share one install root, and Locate must still find the siblings.
/// A file that does not decode is replaced.
pub fn upsert(install_root: &Path, installs: &[ProductInstall]) -> Result<PathBuf, ProductDbError> {
    let path = install_root.join(".product.db");
    let Ok(existing) = std::fs::read(&path) else {
        return write(install_root, installs);
    };
    match splice(&existing, installs) {
        Some(merged) => {
            replace(&path, &existing, &merged)?;
            Ok(path)
        }
        None => write(install_root, installs),
    }
}

#[allow(clippy::cast_possible_truncation, reason = "milliseconds fit u64 for 584 million years")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
