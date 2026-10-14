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
//! Because both sides speak provider ids, an import can be reconciled against
//! an install **without asking any API anything**: what is already there, what
//! is missing, and what is a different version. Only actually fetching an addon
//! needs the network.
//!
//! The token format here was read off a real WoWUp export rather than assumed:
//! it is an object (not an array), its keys are snake_case, its provider string
//! is `Curse` rather than `curseforge`, its ids are strings, and every entry
//! pins an exact `version_id`: a *file* id, not just a project.

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

/// Which builds of an addon are acceptable.
///
/// A **floor, not a filter**: the same semantics WoWUp settled on. Asking for
/// [`Channel::Alpha`] does not mean "only alphas": it means alphas are
/// allowed, and the newest acceptable build still wins, which is usually the
/// stable one. Anything else would pin a user to a stale alpha the day the
/// author shipped a release.
///
/// Only providers that publish pre-release builds can honour this, of ours,
/// CurseForge and GitHub. WoWInterface and Tukui publish one build, so a
/// channel there is inert rather than wrong.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Releases only. The default, because an addon manager that quietly
    /// installs alphas is a bug report waiting to happen.
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
/// [`Provider::Other`] keeps an unrecognised name rather than dropping the
/// entry: a token from a newer WoWUp should list what it could not resolve, not
/// silently shrink.
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

    /// Whether Lightforge can resolve this provider at all today.
    ///
    /// Wago is the exception: its API needs a key nobody has arranged, so a
    /// Wago entry is listed and skipped rather than silently attempted.
    pub fn is_supported(&self) -> bool {
        matches!(
            self,
            Self::Curse | Self::GitHub | Self::WowInterface | Self::TukUI
        )
    }

    /// Whether this provider is resolved without any server of ours.
    ///
    /// The keyless ones keep working when the catalog proxy is down, which is
    /// why the split exists.
    pub fn is_keyless(&self) -> bool {
        matches!(self, Self::GitHub | Self::WowInterface | Self::TukUI)
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
    /// The exact file the exporter had installed. A pin, not a requirement:
    /// importing the *latest* is usually what someone sharing a list means,
    /// but the pin is what makes reproducing their exact setup possible.
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

    /// Encode this list as a token, in the same shape [`Collection::parse`]
    /// reads: base64 over JSON.
    ///
    /// The point is that it travels, to WoWUp, to another machine, to someone
    /// being handed a setup, so it is the format other tools already read
    /// rather than one of Lightforge's own.
    pub fn to_token(&self) -> Result<String, AddonError> {
        let json = serde_json::to_vec(self).map_err(AddonError::Token)?;
        Ok(base64_encode(&json))
    }

    /// WoWUp's name for a product, for the `client_type` an export carries.
    ///
    /// The inverse of what [`Collection::product`] reads. A product WoWUp has
    /// no name for exports as retail, which is what it runs addons for.
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

    /// The product this list was exported from, as far as it can be told.
    ///
    /// WoWUp's names are its own; anything unrecognised is left to the caller
    /// to decide about rather than guessed into the wrong product.
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

    /// Reconcile against what is on disk. No network involved: both sides
    /// carry provider ids.
    ///
    /// Ids are tried first because they are exact, but plenty of addons omit
    /// them, on a real 84-addon install, `!BugGrabber` and
    /// `HandyNotes_Midnight` were both present with no `X-Curse-Project-ID` in
    /// their `.toc` at all, and an id-only match reported them missing. So the
    /// title and then the folder name are tried after, each requiring an exact
    /// match after normalisation. [`Reconciled::matched_by`] says which rule
    /// fired, so a UI can show an id match as certain and a name match as a
    /// guess rather than pretending they are the same thing.
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
    /// Whether this match is exact rather than inferred from a name.
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

/// Compare two addon names for equality, ignoring case, surrounding space, and
/// the `!` prefix authors use to sort themselves to the top of the list.
///
/// Exact after normalisation, never a substring: "BigWigs" must not match
/// "BigWigs_Options".
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
        // Forever and its beta run on the 1.x line, so their addons are Era addons.
        ProductCode::ClassicEra
        | ProductCode::EraPtr
        | ProductCode::ClassicBeta
        | ProductCode::Forever => "classic_era",
        // An unknown or custom product is almost always a retail-line server.
        ProductCode::Custom(_) => "retail",
    }
}

/// A parsed `.toc` file: the `## Key: value` header block.
///
/// Keys are kept as written but looked up case-insensitively, because real
/// addons disagree about capitalisation.
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
}

impl Installed {
    fn has_id(&self, provider: &Provider, id: &str) -> bool {
        self.ids.get(provider).is_some_and(|known| known == id)
    }

    /// Whether this looks like part of a larger addon rather than its own
    /// thing, `BigWigs_Options` next to `BigWigs`, for example.
    ///
    /// A guess, and named as one: it only matters for grouping in a list, never
    /// for deciding what to install or delete.
    pub fn parent_of(&self, others: &[Installed]) -> Option<String> {
        let (prefix, _) = self.folder.split_once('_')?;
        others
            .iter()
            .find(|o| o.folder == prefix && o.folder != self.folder)
            .map(|o| o.folder.clone())
    }
}

/// Scan a product's `AddOns` directory.
///
/// Folders without a readable `.toc` are skipped rather than reported: WoW
/// itself ignores them, and `Blizzard_*` stubs and leftover junk are common.
pub fn scan(addons_dir: &Path) -> Result<Vec<Installed>, AddonError> {
    let entries = std::fs::read_dir(addons_dir).map_err(|source| AddonError::Io {
        path: addons_dir.to_path_buf(),
        source,
    })?;

    let mut found = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let Some(toc_path) = toc_for(&entry.path(), &folder) else {
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

        found.push(Installed {
            title: toc.get("Title").map(strip_color_codes),
            version: toc.get("Version").map(str::to_owned),
            author: toc.get("Author").map(str::to_owned),
            interfaces: toc.interfaces(),
            ids,
            path: entry.path(),
            folder,
        });
    }

    found.sort_by_key(|a| a.folder.to_lowercase());
    Ok(found)
}

/// Pick the `.toc` for a folder.
///
/// Modern addons ship one per flavor, `BigWigs.toc`, `BigWigs_Vanilla.toc`,
/// `BigWigs_Mists.toc`: where the bare name is the mainline (retail) one. The
/// bare file is preferred, then an explicit `_Mainline`, then any `.toc` at
/// all, since the metadata we read is the same across them.
fn toc_for(dir: &Path, folder: &str) -> Option<PathBuf> {
    let exact = dir.join(format!("{folder}.toc"));
    if exact.is_file() {
        return Some(exact);
    }
    let mainline = dir.join(format!("{folder}_Mainline.toc"));
    if mainline.is_file() {
        return Some(mainline);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("toc")))
}

/// Read a `.toc`, tolerating the encodings addons actually ship.
///
/// Some are UTF-8 with a BOM and a few older ones are not valid UTF-8 at all;
/// the header fields are ASCII either way, so lossy decoding loses nothing that
/// matters here and keeps one bad addon from hiding the other eighty.
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

/// Standard base64, padded. The inverse of [`base64_decode`], and padded
/// because that is what WoWUp writes and what other tools expect to read.
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

/// Standard base64 with or without padding. Kept here rather than pulling in a
/// crate for forty lines of table lookup.
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // Asking for alpha accepts a stable build; asking for stable does not
        // accept an alpha. This is WoWUp's rule, and the reason for it is that
        // "alpha" means "I'll take risky builds", not "never give me a
        // release", the newest acceptable build still wins.
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

    /// The point of an export is that something else can read it back, so the
    /// test is the round trip rather than the bytes.
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
        // Keyless providers need no server of ours; Curse does.
        assert!(Provider::TukUI.is_keyless());
        assert!(!Provider::Curse.is_keyless());
        assert!(Provider::Curse.is_supported());
        // Wago is known but unresolvable: listed, never attempted.
        assert!(!Provider::Wago.is_supported());
        // An unknown provider is kept, so it can be reported rather than lost.
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
        };
        assert!(!installed.has_id(&Provider::Curse, "2382"));
        assert!(installed.has_id(&Provider::Wago, "2382"));
    }
}
