//! `.product.db` — Agent's record of what is installed where.
//!
//! **Why this exists:** Battle.net refuses to adopt an install without it. Its
//! "Locate the game" flow reads `<install>/.product.db` and, finding none,
//! answers `error 2800` in the same millisecond — with no message beyond "This
//! folder doesn't contain the correct version of this game". Every other file a
//! real install has, Lightforge already writes; this was the last one, and it
//! is the difference between an install the launcher owns and one Battle.net
//! will hand an SSO session to.
//!
//! The format is protocol-buffer wire format with no schema published by
//! Blizzard, so the layout below was read field by field out of a real
//! install's file. Only what is observed is written — no invented fields.
//!
//! ```text
//! 1  ProductInstall            (repeated: one per installed product)
//!    1  uid                    "wow_classic_era"
//!    2  product_code           "wow_classic_era"
//!    3  Settings
//!       1  install_path        "C:/Program Files (x86)/World of Warcraft"
//!       2  region              "eu"
//!       3,4,5                  0, 2, 3      (constant in every real record)
//!       6,7 locale             "enUS"
//!       8  AssetLocale { 1 locale, 2: 3 }
//!       9,10 empty messages
//!       11 account country     "AUT"
//!       12 geo country         "AT"
//!       13 flavor directory    "_classic_era_"
//!    4  CachedProductState
//!       1  BaseProductState
//!          1,2,3 installed / playable / update complete
//!          7  version          "1.15.9.69722"
//!          12,14 build key     (twice, same value)
//!          17 tags
//!          19 key ring         (absent when the product has none)
//!          21 timestamp (ms)
//!       2,3 timing stubs
//!    6  product family         "wow"
//!    7  0
//!    8  settings json          {"user_install_package_settings":[]}
//! ```

use std::path::{Path, PathBuf};

use crate::buildinfo::{self, BuildInfo};
use crate::product::ProductCode;

/// One installed product, as Agent records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductInstall {
    /// Product code — `uid` and `product_code` are the same string in every
    /// real record.
    pub product: String,
    /// Windows-visible install root, e.g. `C:/Program Files (x86)/World of Warcraft`.
    pub install_path: String,
    pub region: String,
    pub locale: String,
    /// Account country (`AUT`) and GeoIP country (`AT`). Agent fills these from
    /// the logged-in account; empty is accepted rather than invented.
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

    /// Rewrite the tags the way Agent writes them.
    ///
    /// Agent's own records read
    /// `Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:… text?` — the `?`
    /// marks a tag as advisory, and the account/GeoIP tags are present even
    /// though no manifest defines them. Our downloader writes the plain form,
    /// which selects identical files (`TagSet` strips `?` and ignores names the
    /// manifest does not define), but it is not byte-identical to what Agent
    /// expects to read back.
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

    fn encode(&self) -> Vec<u8> {
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

        // Two timing stubs; real records carry doubles here that only describe
        // the last operation, so zeros are honest rather than invented.
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

/// Write `.product.db` into an install root, atomically.
///
/// Agent reads this file while it runs, so it is written to a temporary file in
/// the same directory and renamed — a half-written `.product.db` would be worse
/// than none at all.
pub fn write(install_root: &Path, installs: &[ProductInstall]) -> Result<PathBuf, ProductDbError> {
    let path = install_root.join(".product.db");
    let temp = install_root.join(".product.db.lightforge-tmp");
    let bytes = encode(installs);

    std::fs::write(&temp, &bytes).map_err(|source| ProductDbError::Io {
        path: temp.clone(),
        source,
    })?;
    std::fs::rename(&temp, &path).map_err(|source| ProductDbError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Agent's tag expression for a region, locale and account.
///
/// Both halves are identical but for the trailing `speech?` / `text?`.
pub fn agent_tags(region: &str, locale: &str, account: &str, geo: &str) -> String {
    let mut common = format!("Windows x86_64 {}?", region.to_uppercase());
    if !account.is_empty() {
        common.push_str(&format!(" acct-{account}?"));
    }
    if !geo.is_empty() {
        common.push_str(&format!(" geoip-{geo}?"));
    }
    common.push_str(&format!(" {locale}"));
    format!("{common} speech?:{common} text?")
}

/// The locale named in a tag expression, e.g. `enUS` from
/// `Windows x86_64 EU enUS speech:…`.
fn locale_from_tags(tags: &str) -> Option<String> {
    tags.split([' ', ':'])
        .map(|tag| tag.trim_end_matches('?'))
        .find(|tag| {
            tag.len() == 4
                && tag[..2].chars().all(|c| c.is_ascii_lowercase())
                && tag[2..].chars().all(|c| c.is_ascii_uppercase())
        })
        .map(str::to_owned)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

// --------------------------------------------------------- wire encoding --

fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        match value {
            0 => {
                out.push(byte);
                return;
            }
            _ => out.push(byte | 0x80),
        }
    }
}

fn key(out: &mut Vec<u8>, field: u32, wire: u8) {
    varint(out, u64::from(field) << 3 | u64::from(wire));
}

fn varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    key(out, field, 0);
    varint(out, value);
}

fn string_field(out: &mut Vec<u8>, field: u32, value: &str) {
    key(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn message_field(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    key(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn double_field(out: &mut Vec<u8>, field: u32, value: f64) {
    key(out, field, 1);
    out.extend_from_slice(&value.to_le_bytes());
}

#[derive(Debug, thiserror::Error)]
pub enum ProductDbError {
    #[error("`.build.info` has no {0} for this product")]
    MissingColumn(String),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal wire-format reader, so the tests check the bytes rather than
    /// checking that `encode` agrees with itself.
    fn fields(buf: &[u8]) -> Vec<(u32, u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let (k, next) = read_varint(buf, i);
            i = next;
            let (field, wire) = ((k >> 3) as u32, (k & 7) as u8);
            match wire {
                0 => {
                    let (v, next) = read_varint(buf, i);
                    i = next;
                    out.push((field, wire, v.to_le_bytes().to_vec()));
                }
                1 => {
                    out.push((field, wire, buf[i..i + 8].to_vec()));
                    i += 8;
                }
                2 => {
                    let (len, next) = read_varint(buf, i);
                    i = next;
                    out.push((field, wire, buf[i..i + len as usize].to_vec()));
                    i += len as usize;
                }
                _ => panic!("unexpected wire type {wire}"),
            }
        }
        out
    }

    fn read_varint(buf: &[u8], mut i: usize) -> (u64, usize) {
        let (mut result, mut shift) = (0u64, 0);
        loop {
            let byte = buf[i];
            i += 1;
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return (result, i);
            }
            shift += 7;
        }
    }

    fn sample() -> ProductInstall {
        ProductInstall {
            product: "wow_classic_era".into(),
            install_path: "C:/Program Files (x86)/World of Warcraft".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: "_classic_era_".into(),
            version: "1.15.9.69722".into(),
            build_key: "3645f0fe9dc5215ea90eaf7cdb7379ce".into(),
            tags: "Windows x86_64 EU? enUS speech?:Windows x86_64 EU? enUS text?".into(),
            key_ring: None,
            installed_at_ms: 1_789_676_802_000,
            family: "wow".into(),
        }
    }

    #[test]
    fn a_record_lands_in_the_fields_agent_reads() {
        let bytes = encode(&[sample()]);
        let top = fields(&bytes);
        assert_eq!(top.len(), 1, "one ProductInstall");

        let install = fields(&top[0].2);
        let text = |field: u32| {
            install
                .iter()
                .find(|(f, _, _)| *f == field)
                .map(|(_, _, v)| String::from_utf8_lossy(v).into_owned())
                .unwrap()
        };
        // uid and product_code are the same string in every real record.
        assert_eq!(text(1), "wow_classic_era");
        assert_eq!(text(2), "wow_classic_era");
        assert_eq!(text(6), "wow");

        let settings = fields(&install.iter().find(|(f, _, _)| *f == 3).unwrap().2);
        let setting = |field: u32| {
            settings
                .iter()
                .find(|(f, _, _)| *f == field)
                .map(|(_, _, v)| String::from_utf8_lossy(v).into_owned())
                .unwrap()
        };
        assert_eq!(setting(1), "C:/Program Files (x86)/World of Warcraft");
        assert_eq!(setting(2), "eu");
        assert_eq!(setting(11), "AUT");
        assert_eq!(setting(12), "AT");
        // The flavor directory is how Agent finds the game inside the root.
        assert_eq!(setting(13), "_classic_era_");
    }

    #[test]
    fn a_product_without_a_key_ring_omits_the_field_entirely() {
        // Classic Era has no key ring, and a real record for such a product
        // leaves field 19 out rather than writing an empty string.
        let bytes = encode(&[sample()]);
        let install = fields(&fields(&bytes)[0].2);
        let cached = fields(&install.iter().find(|(f, _, _)| *f == 4).unwrap().2);
        let base = fields(&cached.iter().find(|(f, _, _)| *f == 1).unwrap().2);

        assert!(base.iter().all(|(f, _, _)| *f != 19));
        // The build key is written twice, as active and installed.
        let keys: Vec<String> = base
            .iter()
            .filter(|(f, _, _)| *f == 12 || *f == 14)
            .map(|(_, _, v)| String::from_utf8_lossy(v).into_owned())
            .collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k == "3645f0fe9dc5215ea90eaf7cdb7379ce"));
    }

    #[test]
    fn a_key_ring_is_written_when_the_product_has_one() {
        let install = ProductInstall {
            key_ring: Some("3ca57fe7319a297346440e4d2a03a0cd".into()),
            ..sample()
        };
        let bytes = encode(&[install]);
        let install = fields(&fields(&bytes)[0].2);
        let cached = fields(&install.iter().find(|(f, _, _)| *f == 4).unwrap().2);
        let base = fields(&cached.iter().find(|(f, _, _)| *f == 1).unwrap().2);

        assert!(base.iter().any(|(f, _, _)| *f == 19));
    }

    #[test]
    fn several_products_share_one_file() {
        // A real install lists every product installed under the same root.
        let second = ProductInstall {
            product: "wow".into(),
            flavor_dir: "_retail_".into(),
            ..sample()
        };
        let bytes = encode(&[sample(), second]);
        assert_eq!(fields(&bytes).len(), 2);
    }

    #[test]
    fn agent_style_tags_match_what_a_real_record_carries() {
        // Verbatim shape from Gabriel's install, wow row.
        assert_eq!(
            agent_tags("eu", "enUS", "AUT", "AT"),
            "Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:\
             Windows x86_64 EU? acct-AUT? geoip-AT? enUS text?"
                .replace("\\\n             ", "")
        );
        // Without an account, the acct/geoip tags are left out rather than
        // written empty.
        assert_eq!(
            agent_tags("us", "deDE", "", ""),
            "Windows x86_64 US? deDE speech?:Windows x86_64 US? deDE text?"
        );
    }

    #[test]
    fn agent_tags_parse_back_to_the_same_names_we_select_on() {
        use crate::tact::manifest::TagSet;

        // The `?` suffixes must not change selection, or rewriting the tags
        // would silently change what an update fetches. `TagSet` strips them,
        // so both forms carry the same required names.
        let plain = TagSet::parse("Windows x86_64 EU enUS speech:Windows x86_64 EU enUS text");
        let agent = TagSet::parse(&agent_tags("eu", "enUS", "", ""));
        assert_eq!(format!("{plain:?}"), format!("{agent:?}"));
    }

    #[test]
    fn the_locale_is_read_out_of_the_tag_expression() {
        assert_eq!(
            locale_from_tags("Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:…"),
            Some("enUS".to_owned())
        );
        assert_eq!(
            locale_from_tags("Windows x86_64 EU deDE speech:…"),
            Some("deDE".to_owned())
        );
        assert_eq!(locale_from_tags("Windows x86_64"), None);
    }

    #[test]
    fn varints_match_the_wire_format() {
        let mut out = Vec::new();
        varint(&mut out, 300);
        assert_eq!(out, [0xac, 0x02]);

        // Field 21's key is itself a two-byte varint (21 << 3 = 168), so the
        // value does not start at offset 1 — read the key first.
        let mut out = Vec::new();
        varint_field(&mut out, 21, 1_789_676_802_000);
        let (key, after_key) = read_varint(&out, 0);
        assert_eq!((key >> 3, key & 7), (21, 0));
        assert_eq!(read_varint(&out, after_key).0, 1_789_676_802_000);
    }
}

// ---------------------------------------------------------------- reading --

/// One product Agent reports as installed, as far as Lightforge cares.
///
/// A deliberately small slice of the record: enough to name a product, find
/// its files and say what it is on. Everything else in there describes an
/// install Lightforge does not manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Product code, e.g. `hs_beta`.
    pub product: String,
    /// Windows-visible install root, e.g. `C:/Program Files (x86)/Hearthstone`.
    pub install_path: String,
    /// Installed version, when the record carries one.
    pub version: Option<String>,
    /// Product family, e.g. `hsb`. Empty when absent.
    pub family: String,
}

impl Installed {
    /// Battle.net's own plumbing rather than a game someone plays.
    pub fn is_infrastructure(&self) -> bool {
        matches!(self.product.as_str(), "agent" | "bna" | "battle.net")
    }
}

/// Read Agent's `product.db` — what Battle.net has installed, and where.
///
/// The counterpart to [`encode`], against the same layout documented at the
/// top of this module. Written separately rather than derived from a schema
/// because there is no published schema; the two directions read the same
/// field numbers and are kept honest by a round-trip test.
///
/// Tolerant by design. Unknown fields are skipped, a record missing a field
/// yields a default for it, and malformed bytes end the scan rather than
/// failing the file: this is someone else's file, written by a program that
/// updates on its own schedule, and a launcher that refuses to start because
/// Agent added a field is worse than one that shows fewer tiles.
pub fn decode(bytes: &[u8]) -> Vec<Installed> {
    let mut found = Vec::new();
    for (field, value) in Fields::new(bytes) {
        if field == 1 {
            if let Wire::Bytes(record) = value {
                if let Some(install) = decode_install(record) {
                    found.push(install);
                }
            }
        }
    }
    found
}

/// Read `product.db` out of Agent's data directory.
pub fn read(path: &Path) -> Result<Vec<Installed>, ProductDbError> {
    let bytes = std::fs::read(path).map_err(|source| ProductDbError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(decode(&bytes))
}

fn decode_install(bytes: &[u8]) -> Option<Installed> {
    let mut product = String::new();
    let mut install_path = String::new();
    let mut family = String::new();
    let mut version = None;

    for (field, value) in Fields::new(bytes) {
        match (field, value) {
            (1, Wire::Bytes(b)) => product = string(b),
            (3, Wire::Bytes(settings)) => {
                for (field, value) in Fields::new(settings) {
                    if let (1, Wire::Bytes(b)) = (field, value) {
                        install_path = string(b);
                    }
                }
            }
            (4, Wire::Bytes(cached)) => {
                for (field, value) in Fields::new(cached) {
                    if let (1, Wire::Bytes(base)) = (field, value) {
                        for (field, value) in Fields::new(base) {
                            if let (7, Wire::Bytes(b)) = (field, value) {
                                version = Some(string(b)).filter(|v: &String| !v.is_empty());
                            }
                        }
                    }
                }
            }
            (6, Wire::Bytes(b)) => family = string(b),
            _ => {}
        }
    }

    // A record with no code names nothing; a record with no path points
    // nowhere. Either way there is no tile to draw.
    (!product.is_empty() && !install_path.is_empty()).then_some(Installed {
        product,
        install_path,
        version,
        family,
    })
}

fn string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// One protobuf field's payload, in the two forms this file uses.
enum Wire<'a> {
    Bytes(&'a [u8]),
    /// A varint or fixed-width field. Nothing this reader wants is stored as
    /// one, so the value is skipped rather than carried.
    Scalar,
}

/// Walks the fields of one protobuf message, stopping at the first byte it
/// cannot make sense of.
struct Fields<'a> {
    rest: &'a [u8],
}

impl<'a> Fields<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = self.rest.split_first()?;
            self.rest = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.rest.split_at_checked(n)?;
        self.rest = rest;
        Some(head)
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Wire<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).ok()?;
        let value = match key & 7 {
            0 => {
                self.varint()?;
                Wire::Scalar
            }
            1 => {
                self.take(8)?;
                Wire::Scalar
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                Wire::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Wire::Scalar
            }
            // Groups are long gone from the wire format and Agent emits none.
            _ => return None,
        };
        Some((field, value))
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    /// The two directions must agree, since neither has a schema to check
    /// against — only each other and a real file.
    #[test]
    fn what_we_write_is_what_we_read_back() {
        let install = ProductInstall {
            product: "wow_classic_era".into(),
            install_path: "C:/Program Files (x86)/World of Warcraft".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: "_classic_era_".into(),
            version: "1.15.9.69722".into(),
            build_key: "0".repeat(32),
            tags: "Windows x86_64".into(),
            key_ring: None,
            family: "wow".into(),
            installed_at_ms: 1_700_000_000_000,
        };

        let read = decode(&encode(std::slice::from_ref(&install)));
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].product, install.product);
        assert_eq!(read[0].install_path, install.install_path);
        assert_eq!(read[0].version.as_deref(), Some(install.version.as_str()));
        assert_eq!(read[0].family, install.family);
    }

    #[test]
    fn agent_and_the_client_are_not_games() {
        let plumbing = |product: &str| Installed {
            product: product.into(),
            install_path: "C:/whatever".into(),
            version: None,
            family: String::new(),
        };
        assert!(plumbing("agent").is_infrastructure());
        assert!(plumbing("bna").is_infrastructure());
        assert!(!plumbing("hs_beta").is_infrastructure());
    }

    /// Someone else's file, written by a program that updates on its own
    /// schedule. Every truncation of a good file must end the scan, never
    /// panic and never hang.
    #[test]
    fn a_truncated_file_is_read_as_far_as_it_goes() {
        let install = ProductInstall {
            product: "hs_beta".into(),
            install_path: "C:/Program Files (x86)/Hearthstone".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: String::new(),
            version: "36.6.0.251952".into(),
            build_key: "1".repeat(32),
            tags: String::new(),
            key_ring: None,
            family: "hsb".into(),
            installed_at_ms: 0,
        };
        let whole = encode(&[install]);
        for cut in 0..whole.len() {
            let _ = decode(&whole[..cut]);
        }
        // And arbitrary bytes are a scan that finds nothing, not a crash.
        let noise: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let _ = decode(&noise);
    }
}
