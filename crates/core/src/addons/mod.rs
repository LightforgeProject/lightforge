//! Addon management.
//!
//! Two halves, both offline:
//!
//! - **What is installed**, read from the `.toc` files in a product's `AddOns`
//!   directory. A `.toc` carries the addon's title, version, and, crucially,
//!   the provider ids its author published under (`X-Curse-Project-ID`,
//!   `X-Wago-ID`, `X-WoWI-ID`).
//! - **A WoWUp import token**, which is base64 over a JSON object listing the
//!   provider ids of someone else's addon list.
//!
//! Both speak provider ids, so an import reconciles against an install with
//! no API call; only fetching needs the network.
//!
//! The token format is read off a real WoWUp export: an object, snake_case
//! keys, provider `Curse`, ids as strings, and each entry pins a `version_id`
//! (a file id).

pub mod browse;
pub mod catalog;
pub mod fingerprint;
pub mod install;
pub mod manage;
pub mod providers;
pub mod richtext;

pub use catalog::{Catalog, Origin, Resolved, Status};
pub use richtext::Document as RichText;
pub use install::{Ledger, Record};
pub use manage::{Options as ManageOptions, Report, Unavailable};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::product::ProductCode;

/// Which builds of an addon are acceptable: a **floor, not a filter**, as in
/// WoWUp. [`Channel::Alpha`] allows alphas, and the newest acceptable build
/// still wins. Only CurseForge and GitHub publish pre-releases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Releases only, the default.
    #[default]
    Stable,
    Beta,
    Alpha,
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Alpha => "alpha",
        }
    }

    /// For a menu or a settings row, in floor order.
    pub const ALL: [Channel; 3] = [Channel::Stable, Channel::Beta, Channel::Alpha];

    /// How it reads in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stable => "Stable only",
            Self::Beta => "Stable and beta",
            Self::Alpha => "Stable, beta and alpha",
        }
    }

    /// Whether a build of this kind may be installed under this channel.
    pub fn accepts(&self, build: Channel) -> bool {
        build <= *self
    }
}

/// Where an addon came from.
///
/// [`Provider::Other`] keeps an unrecognised name, so it can be reported.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Provider {
    Curse,
    Wago,
    GitHub,
    WowInterface,
    TukUI,
    Other(String),
}

impl Provider {
    /// Parse the name as a token or `.toc` writes it, case-insensitively.
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "curse" | "curseforge" => Self::Curse,
            "wago" | "wago.io" => Self::Wago,
            "github" => Self::GitHub,
            "wowinterface" | "wowi" => Self::WowInterface,
            "tukui" | "tukui.org" => Self::TukUI,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Curse => "curse",
            Self::Wago => "wago",
            Self::GitHub => "github",
            Self::WowInterface => "wowinterface",
            Self::TukUI => "tukui",
            Self::Other(name) => name,
        }
    }

    /// Whether Lightforge can resolve this provider. Not Wago: its API needs a key.
    pub fn is_supported(&self) -> bool {
        matches!(
            self,
            Self::Curse | Self::GitHub | Self::WowInterface | Self::TukUI
        )
    }
}

/// One addon named by an import token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Provider's project id, as a string, WoWUp writes these quoted.
    pub id: String,
    pub name: String,
    #[serde(rename = "provider_name")]
    pub provider: String,
    /// The exact file the exporter had installed, for reproducing their setup.
    #[serde(default)]
    pub version_id: Option<String>,
}

impl Entry {
    pub fn provider(&self) -> Provider {
        Provider::parse(&self.provider)
    }
}

/// A decoded WoWUp import token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    #[serde(default)]
    pub collection_name: String,
    /// WoWUp's own flavor name, `Retail`, `Classic`, `ClassicEra`, …
    #[serde(default)]
    pub client_type: String,
    #[serde(default)]
    pub addons: Vec<Entry>,
}

impl Collection {
    /// Decode a token: base64 (padded or not) over JSON.
    pub fn parse(token: &str) -> Result<Self, AddonError> {
        let compact: String = token.split_whitespace().collect();
        let json = base64_decode(&compact).ok_or(AddonError::NotBase64)?;
        serde_json::from_slice(&json).map_err(AddonError::Token)
    }

    /// Encode this list as a token, the shape [`Collection::parse`] reads and
    /// WoWUp imports.
    pub fn to_token(&self) -> Result<String, AddonError> {
        let json = serde_json::to_vec(self).map_err(AddonError::Token)?;
        Ok(base64_encode(&json))
    }

    /// WoWUp's name for a product (`client_type`); unknown ones export as retail.
    pub fn client_type_of(code: &ProductCode) -> &'static str {
        match code {
            ProductCode::Classic | ProductCode::ClassicPtr => "Classic",
            ProductCode::ClassicEra
            | ProductCode::EraPtr
            | ProductCode::ClassicBeta
            | ProductCode::Forever => "ClassicEra",
            ProductCode::Ptr | ProductCode::Ptr2 => "RetailPtr",
            ProductCode::Beta => "Beta",
            _ => "Retail",
        }
    }

    /// The product this list was exported from; `None` when unrecognised.
    pub fn product(&self) -> Option<ProductCode> {
        match self.client_type.trim().to_ascii_lowercase().as_str() {
            "retail" => Some(ProductCode::Retail),
            "classic" => Some(ProductCode::Classic),
            "classicera" | "classic_era" => Some(ProductCode::ClassicEra),
            "retailptr" | "ptr" => Some(ProductCode::Ptr),
            "classicptr" => Some(ProductCode::ClassicPtr),
            "beta" => Some(ProductCode::Beta),
            _ => None,
        }
    }

    /// Entries grouped by provider, for a UI that has to explain what it can
    /// and cannot fetch.
    pub fn by_provider(&self) -> BTreeMap<Provider, Vec<&Entry>> {
        let mut grouped: BTreeMap<Provider, Vec<&Entry>> = BTreeMap::new();
        for entry in &self.addons {
            grouped.entry(entry.provider()).or_default().push(entry);
        }
        grouped
    }

    /// Reconcile against what is on disk, by provider id first, then title,
    /// then folder name (on a real install, `!BugGrabber` carries no
    /// `X-Curse-Project-ID`). [`Reconciled::matched_by`] says which rule matched.
    pub fn reconcile(&self, installed: &[Installed]) -> Vec<Reconciled> {
        self.addons
            .iter()
            .map(|entry| {
                let provider = entry.provider();
                let found = installed
                    .iter()
                    .find(|i| i.has_id(&provider, &entry.id))
                    .map(|i| (i, MatchedBy::ProviderId))
                    .or_else(|| {
                        installed
                            .iter()
                            .find(|i| {
                                i.title.as_deref().is_some_and(|t| same_name(t, &entry.name))
                            })
                            .map(|i| (i, MatchedBy::Title))
                    })
                    .or_else(|| {
                        installed
                            .iter()
                            .find(|i| same_name(&i.folder, &entry.name))
                            .map(|i| (i, MatchedBy::Folder))
                    });

                Reconciled {
                    entry: entry.clone(),
                    installed: found.map(|(i, _)| i.folder.clone()),
                    version: found.and_then(|(i, _)| i.version.clone()),
                    matched_by: found.map(|(_, how)| how),
                }
            })
            .collect()
    }
}

/// How a token entry was matched to something on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    /// The `.toc` names the same provider id. Certain.
    ProviderId,
    /// `## Title` equals the entry's name. Near-certain, but a guess.
    Title,
    /// The folder name equals the entry's name. Weakest of the three.
    Folder,
}

impl MatchedBy {
    /// Whether this match is exact, not inferred from a name.
    pub fn is_certain(&self) -> bool {
        matches!(self, Self::ProviderId)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProviderId => "id",
            Self::Title => "title",
            Self::Folder => "folder",
        }
    }
}

/// One token entry, against the install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    pub entry: Entry,
    /// Folder it was found in, if it is installed at all.
    pub installed: Option<String>,
    /// Version string from the installed `.toc`.
    pub version: Option<String>,
    /// Which rule matched it, if any.
    pub matched_by: Option<MatchedBy>,
}

impl Reconciled {
    pub fn is_installed(&self) -> bool {
        self.installed.is_some()
    }
}

/// Compare two addon names, ignoring case, surrounding space and a leading
/// `!`. Never a substring: "BigWigs" is not "BigWigs_Options".
fn same_name(a: &str, b: &str) -> bool {
    let normalise = |s: &str| {
        s.trim()
            .trim_start_matches('!')
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    normalise(a) == normalise(b)
}

/// The flavor name the addon proxy (and CurseForge behind it) uses for a
/// product. Addon packages are published per flavor, not per product code.
pub fn flavor_of(code: &ProductCode) -> &'static str {
    match code {
        ProductCode::Retail | ProductCode::Ptr | ProductCode::Ptr2 | ProductCode::Beta => {
            "retail"
        }
        ProductCode::Classic | ProductCode::ClassicPtr => "classic",
        ProductCode::ClassicEra | ProductCode::EraPtr => "classic_era",
        // CurseForge files Forever addons under their own version type; the
        // proxy falls back to Era per addon.
        ProductCode::ClassicBeta | ProductCode::Forever => "forever",
        // An unknown or custom product is almost always a retail-line server.
        ProductCode::Custom(_) => "retail",
    }
}

/// What one game client reads in an `AddOns` folder: which `.toc` it picks,
/// and which interface number it calls current.
///
/// A client looks for its game suffix, then its family's, then the bare
/// name: a Mists client reads `X_Mists.toc`, `X_Classic.toc`, `X.toc`
/// (warcraft.wiki.gg, TOC format, 2026-10). The default reads only the bare
/// name and never calls anything out of date.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Client {
    suffixes: &'static [&'static str],
    /// `12.1.5` reads as `120105`.
    pub interface: Option<u32>,
}

impl Client {
    /// `version` is the installed one, `.build.info`'s `Version`. The Classic
    /// line changes expansion under one product code, so its major decides.
    pub fn of(code: &ProductCode, version: Option<&str>) -> Self {
        let interface = version.and_then(interface_of);
        let major = interface.map(|i| i / 10_000);
        let suffixes: &'static [&'static str] = match (code, major) {
            (ProductCode::Forever, _) => &["Forever", "Camelot", "Mainline"],
            (ProductCode::Classic | ProductCode::ClassicPtr | ProductCode::ClassicBeta, None) => &["Mists", "Classic"],
            (ProductCode::ClassicEra | ProductCode::EraPtr, None) | (_, Some(1)) => &["Vanilla", "Classic"],
            (_, Some(2)) => &["TBC", "Classic"],
            (_, Some(3)) => &["Wrath", "Classic"],
            (_, Some(4)) => &["Cata", "Classic"],
            (_, Some(5)) => &["Mists", "Classic"],
            _ => &["Standard", "Mainline"],
        };
        Self { suffixes, interface }
    }
}

/// `12.1.5.70575` -> `120105`, the number a `## Interface` line carries.
fn interface_of(version: &str) -> Option<u32> {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
    Some(major * 10_000 + minor * 100 + patch)
}

/// A parsed `.toc` file: the `## Key: value` header block.
///
/// Keys are looked up case-insensitively; addons disagree on capitals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toc {
    fields: Vec<(String, String)>,
}

impl Toc {
    pub fn parse(text: &str) -> Self {
        let fields = text
            .lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("##")?;
                let (key, value) = rest.split_once(':')?;
                Some((key.trim().to_owned(), value.trim().to_owned()))
            })
            .collect();
        Self { fields }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// `## Interface: 120100, 120105`: a multi-version line in modern addons.
    pub fn interfaces(&self) -> Vec<u32> {
        self.get("Interface")
            .map(|v| v.split(',').filter_map(|p| p.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    /// Addons that must load first: `Dependencies`, `RequiredDeps`, or any
    /// key beginning with `Dep`, repeated lines adding up. `OptionalDeps` is
    /// not among them.
    pub fn dependencies(&self) -> Vec<String> {
        let required = |key: &str| {
            key.eq_ignore_ascii_case("RequiredDeps") || key.get(..3).is_some_and(|k| k.eq_ignore_ascii_case("dep"))
        };
        self.fields
            .iter()
            .filter(|(key, _)| required(key))
            .flat_map(|(_, value)| value.split(','))
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    }
}

/// An addon folder found in a product's `AddOns` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Directory name, which is also how WoW refers to the addon.
    pub folder: String,
    pub path: PathBuf,
    /// `## Title`, colour codes stripped.
    pub title: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    /// Provider ids the author published under.
    pub ids: BTreeMap<Provider, String>,
    pub interfaces: Vec<u32>,
    /// Addons this one needs loaded first, by folder name.
    pub dependencies: Vec<String>,
    /// The client would call it out of date: no `.toc` for its flavour, or
    /// none of the interface numbers it names is the client's.
    pub out_of_date: bool,
}

impl Installed {
    fn has_id(&self, provider: &Provider, id: &str) -> bool {
        self.ids.get(provider).is_some_and(|known| known == id)
    }
}

/// Scan a product's `AddOns` directory as `client` reads it.
///
/// Folders without a readable `.toc` are skipped, as WoW skips them.
pub fn scan(addons_dir: &Path, client: &Client) -> Result<Vec<Installed>, AddonError> {
    let entries = std::fs::read_dir(addons_dir).map_err(io(addons_dir))?;

    let mut found = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let Some((toc_path, read_by_client)) = toc_for(&entry.path(), &folder, client) else {
            continue;
        };
        let Ok(text) = read_toc(&toc_path) else { continue };
        let toc = Toc::parse(&text);

        let mut ids = BTreeMap::new();
        for (field, provider) in [
            ("X-Curse-Project-ID", Provider::Curse),
            ("X-Wago-ID", Provider::Wago),
            ("X-WoWI-ID", Provider::WowInterface),
        ] {
            if let Some(id) = toc.get(field) {
                ids.insert(provider, id.to_owned());
            }
        }

        let interfaces = toc.interfaces();
        let out_of_date =
            client.interface.is_some_and(|current| !read_by_client || !interfaces.contains(&current));
        found.push(Installed {
            title: toc.get("Title").map(strip_color_codes),
            version: toc.get("Version").map(str::to_owned),
            author: toc.get("Author").map(str::to_owned),
            interfaces,
            dependencies: toc.dependencies(),
            out_of_date,
            ids,
            path: entry.path(),
            folder,
        });
    }

    found.sort_by_key(|a| a.folder.to_lowercase());
    Ok(found)
}

/// Pick the `.toc` for a folder the way `client` does, and whether it is one
/// the client reads. Failing that, any `.toc`, so the folder is still listed.
///
/// Case-insensitive: the client runs under Wine, which matches names so.
fn toc_for(dir: &Path, folder: &str, client: &Client) -> Option<(PathBuf, bool)> {
    let tocs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("toc")))
        .collect();
    let named = |stem: &str| {
        tocs.iter().find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(stem)))
    };
    client
        .suffixes
        .iter()
        .find_map(|suffix| named(&format!("{folder}_{suffix}")))
        .or_else(|| named(folder))
        .map(|path| (path.clone(), true))
        .or_else(|| tocs.first().map(|path| (path.clone(), false)))
}

/// Required dependencies no folder in `installed` provides, as
/// `(addon folder, dependency)`. Blizzard's own `Blizzard_*` addons ship with
/// the client, not in `AddOns`.
pub fn missing_dependencies(installed: &[Installed]) -> Vec<(String, String)> {
    let present: std::collections::HashSet<String> = installed.iter().map(|a| a.folder.to_lowercase()).collect();
    installed
        .iter()
        .flat_map(|addon| addon.dependencies.iter().map(move |dep| (addon.folder.clone(), dep.clone())))
        .filter(|(_, dep)| !present.contains(&dep.to_lowercase()))
        .filter(|(_, dep)| !dep.get(..9).is_some_and(|p| p.eq_ignore_ascii_case("blizzard_")))
        .collect()
}

/// Read a `.toc` lossily: some have a BOM or are not UTF-8, and the header
/// fields are ASCII anyway.
fn read_toc(path: &Path) -> Result<String, std::io::Error> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

/// Strip WoW's inline colour escapes: `|cffRRGGBB…|r`.
fn strip_color_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '|' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // |cAARRGGBB, the escape plus eight hex digits.
            Some('c') | Some('C') => {
                for _ in 0..8 {
                    if chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                        chars.next();
                    }
                }
            }
            // |r ends a colour run; |T…|t is a texture, left alone.
            Some('r') | Some('R') => {}
            Some(other) => {
                out.push('|');
                out.push(other);
            }
            None => out.push('|'),
        }
    }
    out.trim().to_owned()
}

/// Standard base64, padded, as WoWUp writes it.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let packed = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            match i <= chunk.len() {
                true => out.push(TABLE[(packed >> (18 - i * 6)) as usize & 0x3f] as char),
                false => out.push('='),
            }
        }
    }
    out
}

/// Standard base64 with or without padding.
#[allow(clippy::cast_possible_truncation, reason = "a 6-bit table index, then the low byte of the buffer")]
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        // URL-safe variants appear in the wild; accept them too.
        let byte = match byte {
            b'-' => b'+',
            b'_' => b'/',
            other => other,
        };
        let value = TABLE.iter().position(|c| *c == byte)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[derive(Debug, thiserror::Error)]
pub enum AddonError {
    #[error("this is not a WoWUp import token — it is not valid base64")]
    NotBase64,

    #[error("the token decoded, but is not a WoWUp addon list")]
    Token(#[source] serde_json::Error),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> AddonError + '_ {
    move |source| AddonError::Io { path: path.to_path_buf(), source }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Forever asks for its own flavor, which the proxy backs with Era addons;
    /// Era itself never asks for Forever's, whose builds target the 1.60 API.
    #[test]
    fn forever_has_its_own_flavor_and_era_keeps_its() {
        assert_eq!(flavor_of(&ProductCode::ClassicBeta), "forever");
        assert_eq!(flavor_of(&ProductCode::Forever), "forever");
        assert_eq!(flavor_of(&ProductCode::ClassicEra), "classic_era");
        assert_eq!(flavor_of(&ProductCode::EraPtr), "classic_era");
    }

    /// A trimmed copy of a real WoWUp export, structure verified against the
    /// genuine article, not assumed.
    const TOKEN: &str = concat!(
        "eyJjb2xsZWN0aW9uX25hbWUiOiJXb3dVcF9leHBvcnRfMSIsImNsaWVudF90eXBlIjoiUmV0YWlsIiwi",
        "YWRkb25zIjpbeyJpZCI6IjIzODIiLCJuYW1lIjoiQmlnV2lncyIsInByb3ZpZGVyX25hbWUiOiJDdXJz",
        "ZSIsInZlcnNpb25faWQiOiI4ODY2NTA4In0seyJpZCI6IjEzMzQ0IiwibmFtZSI6IkJ1Z0dyYWJiZXIi",
        "LCJwcm92aWRlcl9uYW1lIjoiQ3Vyc2UiLCJ2ZXJzaW9uX2lkIjoiODkwNzU4NyJ9XX0"
    );

    #[test]
    fn a_channel_is_a_floor_not_a_filter() {
        // Alpha accepts a stable build; stable does not accept an alpha.
        assert!(Channel::Alpha.accepts(Channel::Stable));
        assert!(Channel::Alpha.accepts(Channel::Beta));
        assert!(Channel::Alpha.accepts(Channel::Alpha));
        assert!(Channel::Beta.accepts(Channel::Stable));
        assert!(!Channel::Beta.accepts(Channel::Alpha));
        assert!(Channel::Stable.accepts(Channel::Stable));
        assert!(!Channel::Stable.accepts(Channel::Beta));

        // Stable is the default, so a config or ledger that says nothing
        // never opts someone into pre-release builds.
        assert_eq!(Channel::default(), Channel::Stable);
    }

    #[test]
    fn a_channel_goes_over_the_wire_in_lowercase() {
        // The proxy reads these strings; a rename here is a contract break.
        assert_eq!(serde_json::to_string(&Channel::Beta).unwrap(), "\"beta\"");
        assert_eq!(
            serde_json::from_str::<Channel>("\"alpha\"").unwrap(),
            Channel::Alpha
        );
        for channel in Channel::ALL {
            let wire = serde_json::to_string(&channel).unwrap();
            assert_eq!(wire.trim_matches('"'), channel.as_str());
        }
    }

    #[test]
    fn a_real_token_decodes_to_its_wrapper_object() {
        let collection = Collection::parse(TOKEN).unwrap();
        assert_eq!(collection.client_type, "Retail");
        assert_eq!(collection.product(), Some(ProductCode::Retail));
        assert_eq!(collection.addons.len(), 2);

        let first = &collection.addons[0];
        assert_eq!(first.id, "2382");
        assert_eq!(first.name, "BigWigs");
        assert_eq!(first.provider(), Provider::Curse);
        // The export pins an exact file, not just the project.
        assert_eq!(first.version_id.as_deref(), Some("8866508"));
    }

    #[test]
    fn a_token_without_padding_still_decodes() {
        // WoWUp writes them unpadded; a user may paste a padded copy.
        let padded = format!("{TOKEN}=");
        assert_eq!(Collection::parse(&padded).unwrap().addons.len(), 2);
    }

    #[test]
    fn something_that_is_not_a_token_says_so() {
        assert!(matches!(Collection::parse("not a token!!"), Err(AddonError::NotBase64)));
        // Valid base64, wrong contents.
        assert!(matches!(Collection::parse("aGVsbG8="), Err(AddonError::Token(_))));
    }

    /// A round trip, not the bytes.
    #[test]
    fn a_token_survives_the_round_trip() {
        let collection = Collection {
            collection_name: "Lightforge".to_owned(),
            client_type: "Retail".to_owned(),
            addons: vec![
                Entry {
                    id: "3358".to_owned(),
                    name: "BigWigs".to_owned(),
                    provider: "curseforge".to_owned(),
                    version_id: Some("5309821".to_owned()),
                },
                // No pinned file, and a name with a character JSON has to
                // escape: both of which an encoder can quietly mangle.
                Entry {
                    id: "weakauras/WeakAuras2".to_owned(),
                    name: "WeakAuras \"2\"".to_owned(),
                    provider: "github".to_owned(),
                    version_id: None,
                },
            ],
        };

        let token = collection.to_token().unwrap();
        assert_eq!(Collection::parse(&token).unwrap(), collection);
    }

    /// Every length class of the final chunk, since that is where a hand-rolled
    /// base64 goes wrong.
    #[test]
    fn encoding_pads_every_tail_length() {
        for (bytes, expected) in
            [(&b"a"[..], "YQ=="), (&b"ab"[..], "YWI="), (&b"abc"[..], "YWJj"), (&b""[..], "")]
        {
            assert_eq!(base64_encode(bytes), expected);
            assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
        }
    }

    #[test]
    fn provider_names_are_matched_the_way_wowup_writes_them() {
        assert_eq!(Provider::parse("Curse"), Provider::Curse);
        assert_eq!(Provider::parse("curseforge"), Provider::Curse);
        assert_eq!(Provider::parse("GitHub"), Provider::GitHub);
        assert_eq!(Provider::parse("TukUI"), Provider::TukUI);
        assert_eq!(Provider::parse("WowInterface"), Provider::WowInterface);
        assert!(Provider::Curse.is_supported());
        // Wago is known but unresolvable: listed, never attempted.
        assert!(!Provider::Wago.is_supported());
        // An unknown provider is kept, so it can be reported, not lost.
        assert_eq!(Provider::parse("Fancy"), Provider::Other("fancy".into()));
        assert!(!Provider::Other("fancy".into()).is_supported());
    }

    #[test]
    fn a_toc_yields_its_header_fields() {
        let toc = Toc::parse(
            "## Interface: 120100, 120105\n\
             ## Title: |cffFFFFFFBigWigs|r\n\
             ## Version: v424.8\n\
             ## X-Curse-Project-ID: 2382\n\
             local x = 1 -- not a header\n",
        );
        assert_eq!(toc.get("title"), Some("|cffFFFFFFBigWigs|r"));
        assert_eq!(toc.get("Version"), Some("v424.8"));
        assert_eq!(toc.interfaces(), vec![120100, 120105]);
        assert_eq!(toc.get("Notes"), None);
    }

    /// The header lines of BigWigs v424 (retail `AddOns`, 2026-10-28).
    const BIGWIGS: &str = "## Interface: 11508, 11509, 16001, 20505, 20506, 30405, 38002, 40402, 50503, 50504, 120100, 120105, 120107\n\
        ## OptionalDeps: Ace3, LibCandyBar-3.0, LibSharedMedia-3.0, LibSink-2.0, LibDBIcon-1.0\n";
    const BIGWIGS_CORE: &str = "## Interface: 11508, 11509, 16001, 20505, 20506, 30405, 38002, 40402, 50503, 50504, 120100, 120105, 120107\n\
        ## Dependencies: BigWigs\n";

    #[test]
    fn required_dependencies_leave_the_optional_ones_out() {
        assert_eq!(Toc::parse(BIGWIGS_CORE).dependencies(), ["BigWigs"]);
        assert!(Toc::parse(BIGWIGS).dependencies().is_empty());
        // Any key starting with Dep, repeated lines adding up.
        let toc = Toc::parse("## RequiredDeps: A, B\n## Dep: C\n## DefaultState: disabled\n");
        assert_eq!(toc.dependencies(), ["A", "B", "C"]);
    }

    #[test]
    fn the_client_follows_the_installed_expansion() {
        let retail = Client::of(&ProductCode::Retail, Some("12.1.5.70575"));
        assert_eq!((retail.suffixes[0], retail.interface), ("Standard", Some(120_105)));
        let classic = Client::of(&ProductCode::Classic, Some("5.5.4.70631"));
        assert_eq!((classic.suffixes, classic.interface), (&["Mists", "Classic"][..], Some(50_504)));
        let era = Client::of(&ProductCode::ClassicEra, Some("1.15.9.70576"));
        assert_eq!((era.suffixes[0], era.interface), ("Vanilla", Some(11_509)));
    }

    fn addons_with(folders: &[(&str, &[(&str, &str)])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (folder, tocs) in folders {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
            for (name, text) in *tocs {
                std::fs::write(dir.path().join(folder).join(name), text).unwrap();
            }
        }
        dir
    }

    #[test]
    fn each_client_reads_its_own_toc_and_is_judged_by_it() {
        let dir = addons_with(&[(
            "Details",
            &[("Details.toc", "## Interface: 120105\n"), ("Details_Vanilla.toc", "## Interface: 11509\n")],
        )]);
        let scan_as = |code, version| scan(dir.path(), &Client::of(&code, Some(version))).unwrap().remove(0);

        let era = scan_as(ProductCode::ClassicEra, "1.15.9.70576");
        assert_eq!((era.interfaces.as_slice(), era.out_of_date), (&[11_509][..], false));
        assert!(!scan_as(ProductCode::Retail, "12.1.5.70575").out_of_date);
        // Mists reads the bare file, which names only retail.
        assert!(scan_as(ProductCode::Classic, "5.5.4.70631").out_of_date);
    }

    #[test]
    fn a_folder_with_only_another_flavours_toc_is_listed_and_out_of_date() {
        let dir = addons_with(&[("Questie", &[("Questie-Classic.toc", "## Interface: 11509\n")])]);
        let found = scan(dir.path(), &Client::of(&ProductCode::Retail, Some("12.1.5.70575"))).unwrap();
        assert!(found[0].out_of_date);
    }

    #[test]
    fn a_dependency_counts_as_missing_only_when_no_folder_provides_it() {
        let dir = addons_with(&[
            ("BigWigs_Core", &[("BigWigs_Core.toc", BIGWIGS_CORE)]),
            ("Plater", &[("Plater.toc", "## Dependencies: Blizzard_NamePlates\n")]),
        ]);
        let installed = scan(dir.path(), &Client::default()).unwrap();
        assert_eq!(missing_dependencies(&installed), [("BigWigs_Core".to_owned(), "BigWigs".to_owned())]);

        std::fs::create_dir_all(dir.path().join("BigWigs")).unwrap();
        std::fs::write(dir.path().join("BigWigs/BigWigs.toc"), BIGWIGS).unwrap();
        assert!(missing_dependencies(&scan(dir.path(), &Client::default()).unwrap()).is_empty());
    }

    #[test]
    fn colour_codes_are_stripped_from_titles() {
        assert_eq!(strip_color_codes("|cffFFFFFFBigWigs|r"), "BigWigs");
        assert_eq!(strip_color_codes("plain"), "plain");
        // A texture escape is not a colour escape and must survive.
        assert_eq!(strip_color_codes("|TIcon:16|t Deadly"), "|TIcon:16|t Deadly");
    }

    #[test]
    fn reconciling_matches_on_provider_ids_without_any_network() {
        let collection = Collection::parse(TOKEN).unwrap();
        let installed = vec![Installed {
            folder: "BigWigs".into(),
            path: PathBuf::from("/tmp/BigWigs"),
            title: Some("BigWigs".into()),
            version: Some("v424.8".into()),
            author: None,
            ids: BTreeMap::from([(Provider::Curse, "2382".to_owned())]),
            interfaces: vec![120100],
            dependencies: Vec::new(),
            out_of_date: false,
        }];

        let report = collection.reconcile(&installed);
        assert_eq!(report.len(), 2);
        assert!(report[0].is_installed());
        assert_eq!(report[0].version.as_deref(), Some("v424.8"));
        // BugGrabber is in the list but not on disk.
        assert!(!report[1].is_installed());
    }

    #[test]
    fn an_addon_without_a_provider_id_still_matches_by_title() {
        // Taken from a real install: `!BugGrabber` is present, its `.toc` has
        // no X-Curse-Project-ID, and its title is exactly the token's name.
        let collection = Collection::parse(TOKEN).unwrap();
        let installed = vec![Installed {
            folder: "!BugGrabber".into(),
            path: PathBuf::from("/tmp/!BugGrabber"),
            title: Some("BugGrabber".into()),
            version: Some("v12.1.0".into()),
            author: None,
            ids: BTreeMap::new(),
            interfaces: Vec::new(),
            dependencies: Vec::new(),
            out_of_date: false,
        }];

        let report = collection.reconcile(&installed);
        let grabber = report.iter().find(|r| r.entry.name == "BugGrabber").unwrap();
        assert!(grabber.is_installed());
        assert_eq!(grabber.matched_by, Some(MatchedBy::Title));
        // And it is reported as a guess, not as certainty.
        assert!(!grabber.matched_by.unwrap().is_certain());
    }

    #[test]
    fn a_name_match_is_exact_never_a_prefix() {
        assert!(same_name("BigWigs", "bigwigs"));
        assert!(same_name("!BugGrabber", "BugGrabber"));
        assert!(same_name("HandyNotes:  Midnight", "HandyNotes: Midnight"));
        // The case that would wreck it: a plugin folder is not its parent.
        assert!(!same_name("BigWigs_Options", "BigWigs"));
    }

    #[test]
    fn an_id_only_matches_within_its_own_provider() {
        let installed = Installed {
            folder: "Thing".into(),
            path: PathBuf::new(),
            title: None,
            version: None,
            author: None,
            ids: BTreeMap::from([(Provider::Wago, "2382".to_owned())]),
            interfaces: Vec::new(),
            dependencies: Vec::new(),
            out_of_date: false,
        };
        assert!(!installed.has_id(&Provider::Curse, "2382"));
        assert!(installed.has_id(&Provider::Wago, "2382"));
    }
}
