//! WoW product identity and per-product profile.
//!
//! A "product" is one independently-managed WoW install: Retail, Classic, PTR,
//! or a user-added custom entry. Each carries its own Proton-GE pin, its own
//! realmlist/CDN overrides, and its own prefix state — switching one must never
//! disturb another.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Blizzard's TACT product code (the `--productcode=` argument, and the value
/// in a `.build.info` `Product!` column).
///
/// Official codes are modelled as variants so the common paths are
/// exhaustively matchable; anything else round-trips through
/// [`ProductCode::Custom`] so user-added entries are first-class.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ProductCode {
    /// Retail — `wow`
    Retail,
    /// Classic progression — `wow_classic`
    Classic,
    /// Classic Era — `wow_classic_era`
    ClassicEra,
    /// Retail PTR — `wowt`
    Ptr,
    /// The second retail PTR realm — `wowxptr`. Blizzard runs two at once
    /// during a patch cycle, and they carry different builds.
    Ptr2,
    /// Classic PTR — `wow_classic_ptr`
    ClassicPtr,
    /// Classic Era PTR — `wow_classic_era_ptr`
    EraPtr,
    /// Retail beta — `wow_beta`
    Beta,
    /// Classic-line beta — `wow_classic_beta`. Currently the "World of
    /// Warcraft: Forever" beta, which ships on the 1.x line.
    ClassicBeta,
    /// Any code Lightforge doesn't model explicitly, including custom entries.
    Custom(String),
}

impl ProductCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Retail => "wow",
            Self::Classic => "wow_classic",
            Self::ClassicEra => "wow_classic_era",
            Self::Ptr => "wowt",
            Self::Ptr2 => "wowxptr",
            Self::ClassicPtr => "wow_classic_ptr",
            Self::EraPtr => "wow_classic_era_ptr",
            Self::Beta => "wow_beta",
            Self::ClassicBeta => "wow_classic_beta",
            Self::Custom(code) => code,
        }
    }

    /// Which game this product is a build of.
    pub fn line(&self) -> ProductLine {
        match self {
            Self::Retail | Self::Ptr | Self::Ptr2 | Self::Beta => ProductLine::Retail,
            Self::Classic | Self::ClassicPtr => ProductLine::Classic,
            Self::ClassicEra | Self::EraPtr => ProductLine::ClassicEra,
            Self::ClassicBeta => ProductLine::Forever,
            Self::Custom(code) => ProductLine::Custom(code.clone()),
        }
    }

    /// Which build of its line this product is.
    pub fn branch(&self) -> Branch {
        match self {
            Self::Retail | Self::Classic | Self::ClassicEra | Self::Custom(_) => Branch::Live,
            Self::Ptr | Self::ClassicPtr | Self::EraPtr => Branch::Ptr,
            Self::Ptr2 => Branch::Ptr2,
            Self::Beta | Self::ClassicBeta => Branch::Beta,
        }
    }

    /// A few characters that identify this product in a space too small for
    /// its display name — the dashboard's product bar, before a product has
    /// artwork of its own.
    ///
    /// Distinguishing, not descriptive: every one of these is a World of
    /// Warcraft, so the word that tells them apart is the only word worth the
    /// room. The two betas are separated by line rather than both reading
    /// "Beta", since a launcher that cannot tell them apart is exactly the
    /// launcher that installs the wrong one.
    pub fn short_name(&self) -> &str {
        match self {
            Self::Retail => "Retail",
            Self::Classic => "Classic",
            Self::ClassicEra => "Era",
            Self::Ptr => "PTR",
            Self::Ptr2 => "PTR 2",
            Self::ClassicPtr => "C-PTR",
            Self::EraPtr => "Era PTR",
            Self::Beta => "Beta",
            Self::ClassicBeta => "C-Beta",
            Self::Custom(code) => code,
        }
    }

    /// The game binary this product is expected to ship, by name only.
    ///
    /// A hint, not an authority — see [`crate::paths::Layout::wow_exe`], which
    /// prefers what is actually on disk. One real install carries three
    /// different names: `_retail_/Wow.exe`, `_classic_era_/WowClassic.exe` and
    /// `_classic_beta_/WowB.exe`, so a table alone would be guessing for any
    /// flavor nobody has looked inside yet.
    pub fn executable(&self) -> &str {
        match self {
            Self::Retail | Self::Ptr | Self::Ptr2 => "Wow.exe",
            Self::Classic | Self::ClassicEra | Self::ClassicPtr | Self::EraPtr => {
                "WowClassic.exe"
            }
            Self::Beta | Self::ClassicBeta => "WowB.exe",
            Self::Custom(_) => "Wow.exe",
        }
    }

    /// The install subdirectory this product uses beneath the WoW root, e.g.
    /// `_retail_`.
    ///
    /// Borrowed for every code with a fixed answer, owned only for the custom
    /// `wow_*` codes whose directory is derived — which is why this is a
    /// [`Cow`] rather than a `&str`.
    pub fn install_dir(&self) -> Cow<'_, str> {
        Cow::Borrowed(match self {
            Self::Retail => "_retail_",
            Self::Classic => "_classic_",
            Self::ClassicEra => "_classic_era_",
            Self::Ptr => "_ptr_",
            Self::Ptr2 => "_xptr_",
            Self::ClassicPtr => "_classic_ptr_",
            Self::EraPtr => "_classic_era_ptr_",
            Self::Beta => "_beta_",
            // A real install carries `_classic_beta_/WowB.exe`.
            Self::ClassicBeta => "_classic_beta_",
            // A code this build does not know still follows the convention
            // every known one does: `wow_<flavor>` installs to `_<flavor>_`.
            // Deriving it beats installing into a literal `wow_foo/` directory
            // no WoW client would ever look in.
            Self::Custom(code) => match code.strip_prefix("wow_") {
                Some(flavor) if !flavor.is_empty() => return Cow::Owned(format!("_{flavor}_")),
                _ => code,
            },
        })
    }

    /// Whether this is an official Blizzard product code.
    pub fn is_official(&self) -> bool {
        !matches!(self, Self::Custom(_))
    }
}

impl FromStr for ProductCode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "wow" => Self::Retail,
            "wow_classic" => Self::Classic,
            "wow_classic_era" => Self::ClassicEra,
            "wowt" => Self::Ptr,
            "wowxptr" => Self::Ptr2,
            "wow_classic_ptr" => Self::ClassicPtr,
            "wow_classic_era_ptr" => Self::EraPtr,
            "wow_beta" => Self::Beta,
            "wow_classic_beta" => Self::ClassicBeta,
            other => Self::Custom(other.to_owned()),
        })
    }
}

impl From<String> for ProductCode {
    fn from(s: String) -> Self {
        // Avoid re-allocating when the string is already a custom code.
        match s.parse::<Self>() {
            Ok(Self::Custom(_)) => Self::Custom(s),
            Ok(known) => known,
            Err(never) => match never {},
        }
    }
}

impl From<ProductCode> for String {
    fn from(code: ProductCode) -> Self {
        match code {
            ProductCode::Custom(s) => s,
            other => other.as_str().to_owned(),
        }
    }
}

impl fmt::Display for ProductCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which game a product is, with the branch factored out.
///
/// PTR and beta are *versions of* a game, not games beside it — Blizzard's own
/// launcher presents one tile per line with a version picker behind it, and
/// showing five World of Warcrafts in a row implies five different games.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProductLine {
    Retail,
    Classic,
    ClassicEra,
    /// "World of Warcraft: Forever", currently beta-only. Its own line rather
    /// than a branch of Era: Battle.net gives it its own game family (`WoWF`,
    /// see [`crate::battlenet`]) and its own tile, and this bar is modelled on
    /// that one. Its *addons* are still Era addons — compatibility and
    /// identity are different questions.
    Forever,
    /// A code Lightforge doesn't model. Its own line of one, since nothing
    /// can be assumed about what a custom server is a branch of.
    Custom(String),
}

impl ProductLine {
    /// The stable key a manifest names this line by.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Retail => "retail",
            Self::Classic => "classic",
            Self::ClassicEra => "classic_era",
            Self::Forever => "forever",
            Self::Custom(code) => code,
        }
    }

    /// The line's own name — no branch in it.
    pub fn label(&self) -> &str {
        match self {
            Self::Retail => "Retail",
            Self::Classic => "Classic",
            Self::ClassicEra => "Classic Era",
            Self::Forever => "Forever",
            Self::Custom(code) => code,
        }
    }

    /// The product this line *is* when nobody has picked a branch: its live
    /// one. Also what the UI themes a branch from when the branch has no art
    /// of its own — Classic's PTR should look like Classic, not like whatever
    /// the manifest happens to default to.
    pub fn head(&self) -> ProductCode {
        match self {
            Self::Retail => ProductCode::Retail,
            Self::Classic => ProductCode::Classic,
            Self::ClassicEra => ProductCode::ClassicEra,
            Self::Forever => ProductCode::ClassicBeta,
            Self::Custom(code) => ProductCode::Custom(code.clone()),
        }
    }

    /// Every branch of this line Lightforge knows, live first.
    ///
    /// Knowing about a branch is not the same as having it installed or even
    /// configured — this is the menu, not the inventory.
    pub fn branches(&self) -> Vec<ProductCode> {
        match self {
            Self::Retail => vec![ProductCode::Retail, ProductCode::Ptr, ProductCode::Ptr2, ProductCode::Beta],
            Self::Classic => vec![ProductCode::Classic, ProductCode::ClassicPtr],
            Self::ClassicEra => vec![ProductCode::ClassicEra, ProductCode::EraPtr],
            Self::Forever => vec![ProductCode::ClassicBeta],
            // Nothing is known about a custom code beyond itself.
            Self::Custom(code) => vec![ProductCode::Custom(code.clone())],
        }
    }
}

impl FromStr for ProductLine {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "retail" => Self::Retail,
            "classic" => Self::Classic,
            "classic_era" => Self::ClassicEra,
            "forever" => Self::Forever,
            other => Self::Custom(other.to_owned()),
        })
    }
}

/// Which build of a line a product is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Branch {
    Live,
    Ptr,
    /// The second PTR realm, which only the retail line runs.
    Ptr2,
    Beta,
}

impl Branch {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Ptr => "PTR",
            Self::Ptr2 => "PTR 2",
            Self::Beta => "Beta",
        }
    }
}

impl Branch {
    /// The stable key a manifest names this branch by.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Ptr => "ptr",
            Self::Ptr2 => "ptr2",
            Self::Beta => "beta",
        }
    }
}

impl FromStr for Branch {
    type Err = UnknownBranch;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "live" => Ok(Self::Live),
            "ptr" => Ok(Self::Ptr),
            "ptr2" => Ok(Self::Ptr2),
            "beta" => Ok(Self::Beta),
            other => Err(UnknownBranch(other.to_owned())),
        }
    }
}

/// A manifest named a branch this build does not know.
///
/// Unlike a line, a branch cannot be invented: the set is closed, and
/// guessing would put a product in a version list under a name the UI cannot
/// render. Callers treat it as "no branch stated" and fall back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown branch `{0}`")]
pub struct UnknownBranch(pub String);

impl fmt::Display for Branch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Every product code this build knows by name, live builds first.
///
/// The client's half of the catalog: what Lightforge can describe without
/// asking anyone. [`Catalog`] layers the hosted manifest on top, so a product
/// Blizzard adds after this binary shipped still lands in the right place.
pub const KNOWN_CODES: &[ProductCode] = &[
    ProductCode::Retail,
    ProductCode::Ptr,
    ProductCode::Ptr2,
    ProductCode::Beta,
    ProductCode::Classic,
    ProductCode::ClassicPtr,
    ProductCode::ClassicEra,
    ProductCode::EraPtr,
    ProductCode::ClassicBeta,
];

/// One product in the catalog: what it is, and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub code: ProductCode,
    /// Stable display name. Never an expansion name — see [`ProductLine`].
    pub label: String,
    pub line: ProductLine,
    pub branch: Branch,
}

/// Which products exist, what they are called, and how they group.
///
/// Built into the binary, then overridden by the hosted manifest. The split
/// matters: Blizzard publishes no product listing — `tact::version` documents
/// the dead `summary` endpoint — so the set of products is something someone
/// has to *state*, and stating it in a document that revalidates on every
/// launch beats stating it in a binary that ships every few weeks.
///
/// Only identity is server-driven. Where files go and what runs stays derived
/// from the code, because a typo in a manifest should mis-*label* a product,
/// never install one over another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Catalog {
    /// What this binary knows on its own, with no manifest in hand.
    pub fn builtin() -> Self {
        Self {
            entries: KNOWN_CODES
                .iter()
                .map(|code| CatalogEntry {
                    label: format!("{} {}", code.line().label(), code.branch().label()),
                    line: code.line(),
                    branch: code.branch(),
                    code: code.clone(),
                })
                .collect(),
        }
    }

    /// Layer stated entries over the built-in ones.
    ///
    /// A stated code replaces its built-in entry; an unknown one is appended.
    /// Order follows the statement, then whatever built-ins it did not
    /// mention, so a manifest controls how the bar reads.
    pub fn with(stated: impl IntoIterator<Item = CatalogEntry>) -> Self {
        let mut entries: Vec<CatalogEntry> = stated.into_iter().collect();
        for builtin in Self::builtin().entries {
            if !entries.iter().any(|e| e.code == builtin.code) {
                entries.push(builtin);
            }
        }
        Self { entries }
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn get(&self, code: &ProductCode) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| &e.code == code)
    }

    /// Which line a code belongs to according to the catalog, falling back to
    /// what the code itself says.
    pub fn line_of(&self, code: &ProductCode) -> ProductLine {
        self.get(code).map(|e| e.line.clone()).unwrap_or_else(|| code.line())
    }

    /// Every branch of one line, live first, in catalog order.
    pub fn branches(&self, line: &ProductLine) -> Vec<&CatalogEntry> {
        let mut found: Vec<&CatalogEntry> =
            self.entries.iter().filter(|e| &e.line == line).collect();
        found.sort_by_key(|e| match e.branch {
            Branch::Live => 0,
            Branch::Ptr => 1,
            Branch::Ptr2 => 2,
            Branch::Beta => 3,
        });
        found
    }
}

/// A product as Lightforge presents it: identity plus its user-facing label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Product {
    pub code: ProductCode,
    /// Name shown in the UI, e.g. "World of Warcraft" or "Classic Era".
    pub display_name: String,
    pub profile: ProductProfile,
}

impl Product {
    pub fn new(code: ProductCode, display_name: impl Into<String>) -> Self {
        Self {
            code,
            display_name: display_name.into(),
            profile: ProductProfile::default(),
        }
    }
}

/// Everything the user can configure per product.
///
/// `None` on an override means "use the product's stock behavior"; this is the
/// distinction that keeps a custom realmlist from leaking into Retail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProductProfile {
    /// Which Proton-GE build this product launches under. `None` means the
    /// app-wide default applies.
    pub proton_ge_version: Option<String>,
    /// Contents written to the product's `realmlist`/portal config, if set.
    pub realmlist_override: Option<String>,
    /// CDN/patch endpoint override used by the downloader, if set.
    pub cdn_override: Option<String>,
    pub prefix_state: PrefixState,
}

/// Lifecycle of a product's Wine/Proton prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixState {
    /// No prefix has been created yet.
    #[default]
    Absent,
    /// `wineboot -u` ran but automated setup hasn't finished.
    Provisioning,
    /// Prefix is set up and usable.
    Ready,
    /// Setup failed or the prefix was detected as broken; needs recreation.
    Broken,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductError {
    #[error("no product configured with code `{0}`")]
    Unknown(ProductCode),

    #[error("a product with code `{0}` already exists")]
    Duplicate(ProductCode),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_codes_round_trip() {
        for code in [
            "wow",
            "wow_classic",
            "wow_classic_era",
            "wowt",
            "wow_classic_ptr",
            "wow_beta",
        ] {
            let parsed: ProductCode = code.parse().unwrap();
            assert!(parsed.is_official());
            assert_eq!(parsed.as_str(), code);
        }
    }

    #[test]
    fn unknown_code_becomes_custom() {
        let parsed: ProductCode = "turtle_wow".parse().unwrap();
        assert_eq!(parsed, ProductCode::Custom("turtle_wow".into()));
        assert!(!parsed.is_official());
        assert_eq!(parsed.install_dir(), "turtle_wow");
        // A code shaped like a WoW flavor gets a WoW flavor directory.
        let future: ProductCode = "wow_midnight_ptr".parse().unwrap();
        assert_eq!(future.install_dir(), "_midnight_ptr_");
    }

    #[test]
    fn every_known_code_belongs_to_a_line_that_lists_it_back() {
        let codes = [
            ProductCode::Retail,
            ProductCode::Ptr,
            ProductCode::Ptr2,
            ProductCode::Beta,
            ProductCode::Classic,
            ProductCode::ClassicPtr,
            ProductCode::ClassicEra,
            ProductCode::EraPtr,
            ProductCode::ClassicBeta,
        ];
        for code in codes {
            let line = code.line();
            assert!(
                line.branches().contains(&code),
                "{} claims line {} but that line does not list it",
                code.as_str(),
                line.label()
            );
        }
    }

    #[test]
    fn a_lines_head_is_the_live_branch_of_that_line() {
        for line in [
            ProductLine::Retail,
            ProductLine::Classic,
            ProductLine::ClassicEra,
            ProductLine::Forever,
        ] {
            let head = line.head();
            assert_eq!(head.line(), line, "{} head is in another line", line.label());
            assert_eq!(
                head,
                line.branches()[0],
                "{} head is not its first branch",
                line.label()
            );
        }
    }

    #[test]
    fn a_custom_code_is_its_own_line_of_one() {
        let code = ProductCode::Custom("turtle_wow".into());
        let line = code.line();
        assert_eq!(line.branches(), vec![code.clone()]);
        assert_eq!(line.head(), code);
        assert_eq!(code.branch(), Branch::Live);
    }

    #[test]
    fn the_new_codes_round_trip_through_their_strings() {
        for code in [ProductCode::Ptr2, ProductCode::EraPtr] {
            assert_eq!(code.as_str().parse::<ProductCode>().unwrap(), code);
        }
        assert_eq!(ProductCode::Ptr2.install_dir(), "_xptr_");
        assert_eq!(ProductCode::EraPtr.install_dir(), "_classic_era_ptr_");
    }
}
