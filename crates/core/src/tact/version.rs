//! What Blizzard currently ships: the remote half of the update check.
//!
//! Two endpoints, both returning BPSV, the same pipe-delimited table grammar
//! as `.build.info`:
//!
//! ```text
//! https://<region>.version.battle.net/<product>/versions
//! https://<region>.version.battle.net/<product>/cdns
//! ```
//!
//! Not through `ribbit-client` (HTTP 400 from the TCP endpoint on both
//! protocol versions) nor `tact-client` (V2 rejects the response, V1 is plain
//! HTTP on port 1119). Direct over TLS through [`crate::net::Http`].
//!
//! TLS keeps an observer from steering the launcher at a different valid
//! build; the hash checks downstream stop arbitrary content.

use crate::net::{Http, NetError};

/// Blizzard's regional version services, saved in the config in Blizzard's
/// spelling (`eu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Us,
    #[default]
    Eu,
    Kr,
    Cn,
    Tw,
    Sg,
}

impl Region {
    /// Every region, in the order a picker should offer them: the three with
    /// most players first, then the rest.
    pub const ALL: [Region; 6] =
        [Region::Eu, Region::Us, Region::Kr, Region::Tw, Region::Cn, Region::Sg];

    /// What to call this region in a picker: `us` serves the Americas and Oceania.
    pub fn label(self) -> &'static str {
        match self {
            Self::Us => "Americas & Oceania",
            Self::Eu => "Europe",
            Self::Kr => "Korea",
            Self::Cn => "China",
            Self::Tw => "Taiwan",
            Self::Sg => "Southeast Asia",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Us => "us",
            Self::Eu => "eu",
            Self::Kr => "kr",
            Self::Cn => "cn",
            Self::Tw => "tw",
            Self::Sg => "sg",
        }
    }

    /// Host serving this region's version tables, over TLS.
    pub fn host(self) -> String {
        format!("https://{}.version.battle.net", self.as_str())
    }
}

impl std::str::FromStr for Region {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "us" => Self::Us,
            "eu" => Self::Eu,
            "kr" => Self::Kr,
            "cn" => Self::Cn,
            "tw" => Self::Tw,
            "sg" => Self::Sg,
            _ => return Err(VersionError::UnknownRegion(s.to_owned())),
        })
    }
}

/// One region's current build, as the service reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub region: String,
    /// Hash of the build config, `.build.info`'s `Build Key`: what an update
    /// check compares.
    pub build_config: String,
    pub cdn_config: String,
    pub key_ring: Option<String>,
    pub build_id: u32,
    /// Human-readable, e.g. `12.1.0.69814`.
    pub versions_name: String,
    pub product_config: String,
}

/// One region's CDN hosts and paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cdn {
    pub name: String,
    /// Path prefix for data and config requests, e.g. `tpr/wow`.
    pub path: String,
    pub hosts: Vec<String>,
    /// Full URLs with query parameters, http and https mixed.
    pub servers: Vec<String>,
    pub config_path: String,
}

impl Cdn {
    /// Servers that can be reached over TLS, preferred over the plain ones.
    pub fn https_servers(&self) -> Vec<&str> {
        self.servers
            .iter()
            .map(String::as_str)
            .filter(|s| s.starts_with("https://"))
            .collect()
    }
}

/// Fetch the version table for a product: the build that is live.
pub async fn versions(http: &Http, region: Region, product: &str) -> Result<Vec<Version>, VersionError> {
    let url = format!("{}/{product}/versions", region.host());
    parse_versions(&http.get_text(&url).await?)
}

/// Fetch the background-download table: a build listed ahead of its release
/// for clients to pre-load. Empty between patches, and for a product that
/// has none the service answers `No matched data for selector …`, which
/// parses to no rows as well.
pub async fn bgdl(http: &Http, region: Region, product: &str) -> Result<Vec<Version>, VersionError> {
    let url = format!("{}/{product}/bgdl", region.host());
    parse_versions(&http.get_text(&url).await?)
}

/// `/versions` and `/bgdl` share their columns.
fn parse_versions(text: &str) -> Result<Vec<Version>, VersionError> {
    Bpsv::parse(text)?
        .rows()
        .map(|row| {
            Ok(Version {
                region: row.get("Region")?.to_owned(),
                build_config: row.get("BuildConfig")?.to_owned(),
                cdn_config: row.get("CDNConfig")?.to_owned(),
                key_ring: row.get("KeyRing").ok().filter(|k| !k.is_empty()).map(str::to_owned),
                build_id: row.get("BuildId")?.parse().unwrap_or(0),
                versions_name: row.get("VersionsName")?.to_owned(),
                product_config: row.get("ProductConfig").unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Fetch the CDN table for a product.
pub async fn cdns(http: &Http, region: Region, product: &str) -> Result<Vec<Cdn>, VersionError> {
    let url = format!("{}/{product}/cdns", region.host());
    let table = Bpsv::parse(&http.get_text(&url).await?)?;

    table
        .rows()
        .map(|row| {
            Ok(Cdn {
                name: row.get("Name")?.to_owned(),
                path: row.get("Path")?.to_owned(),
                hosts: row.get("Hosts").unwrap_or_default().split_whitespace().map(str::to_owned).collect(),
                servers: row.get("Servers").unwrap_or_default().split_whitespace().map(str::to_owned).collect(),
                config_path: row.get("ConfigPath").unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// A product code Lightforge knows by name, and its UI label. Never an
/// expansion name: `wow_classic` stays "Classic".
pub const KNOWN_PRODUCTS: &[(&str, &str)] = &[
    ("wow", "Retail"),
    ("wowt", "Retail PTR"),
    ("wowxptr", "Retail PTR 2"),
    ("wow_beta", "Retail Beta"),
    ("wow_classic", "Classic"),
    ("wow_classic_ptr", "Classic PTR"),
    ("wow_classic_beta", "Classic Beta"),
    ("wow_classic_era", "Classic Era"),
    ("wow_classic_era_ptr", "Classic Era PTR"),
    ("wow_anniversary", "Anniversary"),
    ("wowf", "Forever"),
    ("osi", "Diablo II: Resurrected"),
    ("d3", "Diablo III"),
    ("fenris", "Diablo IV"),
    ("anbs", "Diablo Immortal"),
    ("hsb", "Hearthstone"),
    ("w1r", "Warcraft I Remastered"),
    ("w2r", "Warcraft II Remastered"),
    ("w3", "Warcraft III: Reforged"),
    ("s1", "StarCraft Remastered"),
    ("s2", "StarCraft II"),
    ("pro", "Overwatch 2"),
    ("hero", "Heroes of the Storm"),
];

/// What a probe found out about one product code.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductStatus {
    pub code: String,
    pub label: Option<String>,
    /// `None` when the service has no such product (404).
    pub version: Option<Version>,
}

impl ProductStatus {
    pub fn exists(&self) -> bool {
        self.version.is_some()
    }
}

/// Ask whether one product code is live, and what it ships. `Ok(None)`: no
/// such product. There is no listing (`summary` is 404, Ribbit is gone), so
/// discovery probes [`KNOWN_PRODUCTS`].
pub async fn probe(
    http: &Http,
    region: Region,
    product: &str,
) -> Result<Option<Version>, VersionError> {
    match versions(http, region, product).await {
        Ok(rows) => Ok(for_region(&rows, region, |v| &v.region).cloned()),
        Err(VersionError::Net(NetError::Status { status: 404, .. })) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Probe many codes at once, in the order given. A code that errors is
/// reported absent: one flaky request should not sink the table.
pub async fn probe_all(
    http: &Http,
    region: Region,
    codes: &[&str],
    concurrency: usize,
) -> Vec<ProductStatus> {
    use futures_util::StreamExt;

    let label = |code: &str| {
        KNOWN_PRODUCTS
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, l)| (*l).to_owned())
    };

    let mut found: Vec<ProductStatus> = futures_util::stream::iter(codes.iter().copied())
        .map(|code| async move {
            ProductStatus {
                code: code.to_owned(),
                label: label(code),
                version: probe(http, region, code).await.ok().flatten(),
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    // `buffer_unordered` yields out of order.
    found.sort_by_key(|s| codes.iter().position(|c| *c == s.code).unwrap_or(usize::MAX));
    found
}

/// Pick one region's row out of a table, falling back to the first: every
/// region reports the same build.
pub fn for_region<T>(rows: &[T], region: Region, name: impl Fn(&T) -> &str) -> Option<&T> {
    rows.iter()
        .find(|r| name(r) == region.as_str())
        .or_else(|| rows.first())
}

/// How a local install compares to what the service currently ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No local `.build.info` row, the product has never been installed.
    NotInstalled,
    /// Local build config matches the service's.
    Current { build: String, version: String },
    /// The service ships a different build config. Not "newer": the check is
    /// hash equality, so a rollback lands here too.
    Behind {
        local_build: String,
        remote_build: String,
        local_version: Option<String>,
        remote_version: String,
    },
}

impl Status {
    /// Compare a local `Build Key` from `.build.info` against a remote row.
    pub fn compare(local_build: Option<&str>, local_version: Option<&str>, remote: &Version) -> Self {
        let Some(local) = local_build.filter(|b| !b.is_empty()) else {
            return Self::NotInstalled;
        };
        if local.eq_ignore_ascii_case(&remote.build_config) {
            return Self::Current {
                build: remote.build_config.clone(),
                version: remote.versions_name.clone(),
            };
        }
        Self::Behind {
            local_build: local.to_owned(),
            remote_build: remote.build_config.clone(),
            local_version: local_version.map(str::to_owned),
            remote_version: remote.versions_name.clone(),
        }
    }

    pub fn needs_update(&self) -> bool {
        matches!(self, Self::Behind { .. })
    }

    /// The button this maps to, as far as the version check can tell.
    pub fn action(&self) -> crate::ActionState {
        match self {
            Self::NotInstalled => crate::ActionState::Install,
            Self::Current { .. } => crate::ActionState::Play,
            Self::Behind { local_build, remote_build, .. } => crate::ActionState::Update {
                local_build: local_build.clone(),
                remote_build: remote_build.clone(),
            },
        }
    }
}

/// A BPSV table: typed header row, optional `##` comments, then data rows.
/// Read-only, unlike [`crate::buildinfo`], which must round-trip byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bpsv {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Bpsv {
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let mut lines = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty() && !l.starts_with("##"));

        let header = lines.next().ok_or(VersionError::EmptyTable)?;
        let columns: Vec<String> = header
            .split('|')
            // `BuildConfig!HEX:16` -> `BuildConfig`.
            .map(|c| c.split('!').next().unwrap_or(c).trim().to_owned())
            .collect();

        let rows: Vec<Vec<String>> = lines
            .map(|line| line.split('|').map(str::to_owned).collect())
            .collect();

        Ok(Self { columns, rows })
    }

    pub fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        self.rows.iter().map(move |cells| Row { columns: &self.columns, cells })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// One row, addressed by column name.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    columns: &'a [String],
    cells: &'a [String],
}

impl Row<'_> {
    pub fn get(&self, column: &str) -> Result<&str, VersionError> {
        // Case-insensitive: the service's own spelling is inconsistent
        // (`VersionsName!String:0`).
        let i = self
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(column))
            .ok_or_else(|| VersionError::MissingColumn(column.to_owned()))?;
        self.cells
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| VersionError::MissingColumn(column.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("`{0}` is not a Blizzard region")]
    UnknownRegion(String),

    #[error("the version service returned an empty table")]
    EmptyTable,

    #[error("the version table has no `{0}` column")]
    MissingColumn(String),

    #[error(transparent)]
    Net(#[from] NetError),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured verbatim from `https://eu.version.battle.net/wow/versions`,
    /// including the `## seqn` line and the lower-case `String` type that the
    /// service really does emit for one column.
    #[test]
    fn known_product_codes_are_unique_and_labelled() {
        let mut codes: Vec<&str> = KNOWN_PRODUCTS.iter().map(|(c, _)| *c).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "a duplicate code would probe twice and label once");
        assert!(KNOWN_PRODUCTS.iter().all(|(_, label)| !label.is_empty()));
    }

    #[test]
    fn labels_name_the_stream_not_the_expansion() {
        // wow_classic stays "Classic" when its expansion rolls over.
        let banned = ["Midnight", "Pandaria", "Mists", "Burning Crusade", "Wrath", "Cataclysm"];
        for (code, label) in KNOWN_PRODUCTS {
            for word in banned {
                assert!(!label.contains(word), "{code} is labelled with an expansion: {label}");
            }
        }
    }

    const VERSIONS: &str = "\
Region!STRING:0|BuildConfig!HEX:16|CDNConfig!HEX:16|KeyRing!HEX:16|BuildId!DEC:4|VersionsName!String:0|ProductConfig!HEX:16
## seqn = 4019651
us|9712ae9c84cc54111e9e3ddf9a6b8019|272d201d5b2d6fec8fdb4aa59b2a9eea|3ca57fe7319a297346440e4d2a03a0cd|69814|12.1.0.69814|53020d32e1a25648c8e1eafd5771935f
eu|9712ae9c84cc54111e9e3ddf9a6b8019|272d201d5b2d6fec8fdb4aa59b2a9eea|3ca57fe7319a297346440e4d2a03a0cd|69814|12.1.0.69814|53020d32e1a25648c8e1eafd5771935f
";

    const CDNS: &str = "\
Name!STRING:0|Path!STRING:0|Hosts!STRING:0|Servers!STRING:0|ConfigPath!STRING:0
## seqn = 4019522
eu|tpr/wow|level3.blizzard.com|http://level3.blizzard.com/?maxhosts=8 https://level3.ssl.blizzard.com/?maxhosts=8&fallback=1|tpr/configs/data
";

    #[test]
    fn the_seqn_comment_is_not_read_as_a_row() {
        // `## seqn = N` sits between header and data; it is not a row.
        let table = Bpsv::parse(VERSIONS).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.rows().next().unwrap().get("Region").unwrap(), "us");
    }

    #[test]
    fn columns_resolve_by_name_ignoring_the_type_suffix_and_case() {
        let table = Bpsv::parse(VERSIONS).unwrap();
        let row = table.rows().next().unwrap();
        assert_eq!(row.get("BuildConfig").unwrap(), "9712ae9c84cc54111e9e3ddf9a6b8019");
        // The service's own spelling is `VersionsName!String:0`, lower-case s.
        assert_eq!(row.get("versionsname").unwrap(), "12.1.0.69814");
        assert!(matches!(row.get("Nonexistent"), Err(VersionError::MissingColumn(_))));
    }

    #[test]
    fn a_versions_table_parses_into_the_fields_an_update_check_needs() {
        let table = Bpsv::parse(VERSIONS).unwrap();
        let rows: Vec<_> = table.rows().collect();
        let eu = rows.iter().find(|r| r.get("Region").unwrap() == "eu").unwrap();

        assert_eq!(eu.get("BuildConfig").unwrap(), "9712ae9c84cc54111e9e3ddf9a6b8019");
        assert_eq!(eu.get("BuildId").unwrap(), "69814");
        assert_eq!(eu.get("KeyRing").unwrap(), "3ca57fe7319a297346440e4d2a03a0cd");
    }

    #[test]
    fn https_servers_are_separable_from_plain_ones() {
        let table = Bpsv::parse(CDNS).unwrap();
        let row = table.rows().next().unwrap();
        let cdn = Cdn {
            name: row.get("Name").unwrap().into(),
            path: row.get("Path").unwrap().into(),
            hosts: row.get("Hosts").unwrap().split_whitespace().map(str::to_owned).collect(),
            servers: row.get("Servers").unwrap().split_whitespace().map(str::to_owned).collect(),
            config_path: row.get("ConfigPath").unwrap().into(),
        };
        assert_eq!(cdn.path, "tpr/wow");
        assert_eq!(cdn.servers.len(), 2);
        assert_eq!(cdn.https_servers(), vec!["https://level3.ssl.blizzard.com/?maxhosts=8&fallback=1"]);
    }

    #[test]
    fn regions_resolve_to_tls_hosts() {
        assert_eq!(Region::Eu.host(), "https://eu.version.battle.net");
        assert_eq!("US".parse::<Region>().unwrap(), Region::Us);
        assert!("mars".parse::<Region>().is_err());
        assert_eq!(Region::default(), Region::Eu);
    }

    #[test]
    fn a_missing_region_falls_back_rather_than_failing() {
        let rows = vec![
            Version {
                region: "us".into(),
                build_config: "a".into(),
                cdn_config: "b".into(),
                key_ring: None,
                build_id: 1,
                versions_name: "1.0".into(),
                product_config: String::new(),
            },
        ];
        assert_eq!(for_region(&rows, Region::Us, |v| &v.region).unwrap().region, "us");
        // Every region ships the same build today.
        assert_eq!(for_region(&rows, Region::Kr, |v| &v.region).unwrap().region, "us");
        assert!(for_region::<Version>(&[], Region::Eu, |v| &v.region).is_none());
    }

    fn remote(build: &str, name: &str) -> Version {
        Version {
            region: "eu".into(),
            build_config: build.into(),
            cdn_config: "272d201d5b2d6fec8fdb4aa59b2a9eea".into(),
            key_ring: None,
            build_id: 69814,
            versions_name: name.into(),
            product_config: String::new(),
        }
    }

    #[test]
    fn a_matching_build_key_is_up_to_date() {
        let r = remote("9712ae9c84cc54111e9e3ddf9a6b8019", "12.1.0.69814");
        let s = Status::compare(Some("9712ae9c84cc54111e9e3ddf9a6b8019"), Some("12.1.0.69814"), &r);
        assert!(matches!(s, Status::Current { .. }));
        assert!(!s.needs_update());
        assert_eq!(s.action(), crate::ActionState::Play);

        // Hashes are hex; case is not a difference.
        let upper = Status::compare(Some("9712AE9C84CC54111E9E3DDF9A6B8019"), None, &r);
        assert!(matches!(upper, Status::Current { .. }));
    }

    #[test]
    fn a_differing_build_key_needs_an_update_in_either_direction() {
        let r = remote("aaaabbbbccccddddeeeeffff00001111", "12.2.0.70000");
        let s = Status::compare(Some("9712ae9c84cc54111e9e3ddf9a6b8019"), Some("12.1.0.69814"), &r);
        assert!(s.needs_update());
        assert_eq!(
            s.action(),
            crate::ActionState::Update {
                local_build: "9712ae9c84cc54111e9e3ddf9a6b8019".into(),
                remote_build: "aaaabbbbccccddddeeeeffff00001111".into(),
            }
        );

        // A rollback is still "does not match".
        let older = remote("11110000111100001111000011110000", "12.0.5.60000");
        assert!(Status::compare(Some("9712ae9c84cc54111e9e3ddf9a6b8019"), None, &older).needs_update());
    }

    #[test]
    fn no_local_row_means_install_not_update() {
        let r = remote("9712ae9c84cc54111e9e3ddf9a6b8019", "12.1.0.69814");
        assert_eq!(Status::compare(None, None, &r), Status::NotInstalled);
        // An empty cell is the same as no cell: .build.info has empty columns.
        assert_eq!(Status::compare(Some(""), None, &r), Status::NotInstalled);
        assert_eq!(Status::compare(None, None, &r).action(), crate::ActionState::Install);
    }

    /// `https://eu.version.battle.net/wow/bgdl` and `…/wow_classic_era/bgdl`,
    /// both captured 2026-10-28, between patches.
    #[test]
    fn a_bgdl_table_between_patches_lists_no_build() {
        let wow = "Region!STRING:0|BuildConfig!HEX:16|CDNConfig!HEX:16|KeyRing!HEX:16|BuildId!DEC:4|\
                   VersionsName!String:0|ProductConfig!HEX:16\n## seqn = 4083842\n";
        assert!(parse_versions(wow).unwrap().is_empty());
        let era = "No matched data for selector 'v2/products/wow_classic_era/bgdl'";
        assert!(parse_versions(era).unwrap().is_empty());
    }

    #[test]
    fn an_empty_response_is_an_error_not_an_empty_table() {
        assert!(matches!(Bpsv::parse(""), Err(VersionError::EmptyTable)));
        assert!(matches!(Bpsv::parse("## seqn = 1\n"), Err(VersionError::EmptyTable)));
    }
}
