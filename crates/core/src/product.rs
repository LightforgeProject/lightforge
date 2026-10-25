//! WoW product identity and per-product profile.
//!
//! A "product" is one independently-managed WoW install: Retail, Classic, PTR,
//! or a user-added custom entry. Each carries its own Proton-GE pin, its own
//! realmlist/CDN overrides, and its own prefix state, switching one must never
//! disturb another.

use std::borrow::Cow;
use std::collections::HashSet;
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
    /// Retail, `wow`
    Retail,
    /// Classic progression, `wow_classic`
    Classic,
    /// Classic Era, `wow_classic_era`
    ClassicEra,
    /// Retail PTR, `wowt`
    Ptr,
    /// The second retail PTR realm, `wowxptr`. Blizzard runs two at once
    /// during a patch cycle, and they carry different builds.
    Ptr2,
    /// Classic PTR, `wow_classic_ptr`
    ClassicPtr,
    /// Classic Era PTR, `wow_classic_era_ptr`
    EraPtr,
    /// Retail beta, `wow_beta`
    Beta,
    /// Classic-line beta, `wow_classic_beta`. Currently the "World of
    /// Warcraft: Forever" beta, which ships on the 1.x line.
    ClassicBeta,
    /// World of Warcraft: Forever, `wowf`. First published for the
    /// October 2026 stress test; Battle.net lists it in the Forever tile's
    /// version dropdown beside the beta.
    Forever,
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
            Self::Forever => "wowf",
            Self::Custom(code) => code,
        }
    }

    /// Which game this product is a build of.
    pub fn line(&self) -> ProductLine {
        match self {
            Self::Retail | Self::Ptr | Self::Ptr2 | Self::Beta => ProductLine::Retail,
            Self::Classic | Self::ClassicPtr => ProductLine::Classic,
            Self::ClassicEra | Self::EraPtr => ProductLine::ClassicEra,
            Self::ClassicBeta | Self::Forever => ProductLine::Forever,
            Self::Custom(code) => ProductLine::Custom(code.clone()),
        }
    }

    /// Which build of its line this product is.
    pub fn branch(&self) -> Branch {
        match self {
            Self::Retail | Self::Classic | Self::ClassicEra | Self::Forever | Self::Custom(_) => {
                Branch::Live
            }
            Self::Ptr | Self::ClassicPtr | Self::EraPtr => Branch::Ptr,
            Self::Ptr2 => Branch::Ptr2,
            Self::Beta | Self::ClassicBeta => Branch::Beta,
        }
    }

    /// A few characters that identify this product in a space too small for
    /// its display name, the dashboard's product bar, before a product has
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
            Self::Forever => "Forever",
            Self::Custom(code) => code,
        }
    }

    /// The game binary this product is expected to ship, by name only.
    ///
    /// A hint, not an authority; see [`crate::paths::Layout::wow_exe`], which
    /// prefers what is actually on disk. One real install carries three
    /// different names: `_retail_/Wow.exe`, `_classic_era_/WowClassic.exe` and
    /// `_classic_beta_/WowB.exe`, so a table alone would be guessing for any
    /// flavor nobody has looked inside yet.
    pub fn executable(&self) -> &str {
        match self {
            // `wowf`'s product config names `Wow.exe`, not a Classic name.
            Self::Retail | Self::Ptr | Self::Ptr2 | Self::Forever => "Wow.exe",
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
    /// `wow_*` codes whose directory is derived, which is why this is a
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
            // From `wowf`'s product config (`shared_container_default_subfolder`).
            // The code is not `wow_<flavor>` shaped, so the fallback below
            // would have answered a literal `wowf/`.
            Self::Forever => "_forever_",
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
            "wowf" => Self::Forever,
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
/// PTR and beta are *versions of* a game, not games beside it, Blizzard's own
/// launcher presents one tile per line with a version picker behind it, and
/// showing five World of Warcrafts in a row implies five different games.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProductLine {
    Retail,
    Classic,
    ClassicEra,
    /// "World of Warcraft: Forever": `wowf`, with the retired beta beside it. Its own line rather
    /// than a branch of Era: Battle.net gives it its own game family (`WoWF`,
    /// see [`crate::battlenet`]) and its own tile, and this bar is modelled on
    /// that one. Its *addons* are still Era addons, compatibility and
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

    /// The line's own name, no branch in it.
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
    /// of its own, Classic's PTR should look like Classic, not like whatever
    /// the manifest happens to default to.
    pub fn head(&self) -> ProductCode {
        match self {
            Self::Retail => ProductCode::Retail,
            Self::Classic => ProductCode::Classic,
            Self::ClassicEra => ProductCode::ClassicEra,
            // Live `wowf`. The beta it replaced is still a branch so an
            // install of it keeps working, but it is no longer the head.
            Self::Forever => ProductCode::Forever,
            Self::Custom(code) => ProductCode::Custom(code.clone()),
        }
    }

    /// Every branch of this line Lightforge knows, live first.
    ///
    /// Knowing about a branch is not the same as having it installed or even
    /// configured. This is the menu, not the inventory.
    pub fn branches(&self) -> Vec<ProductCode> {
        match self {
            Self::Retail => vec![ProductCode::Retail, ProductCode::Ptr, ProductCode::Ptr2, ProductCode::Beta],
            Self::Classic => vec![ProductCode::Classic, ProductCode::ClassicPtr],
            Self::ClassicEra => vec![ProductCode::ClassicEra, ProductCode::EraPtr],
            Self::Forever => vec![ProductCode::Forever, ProductCode::ClassicBeta],
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
    ProductCode::Forever,
    ProductCode::ClassicBeta,
];

/// Whether Lightforge installs a product itself, or hands it to Battle.net.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The WoW family: Lightforge plans, downloads and launches it.
    Managed,
    /// Everything else. `SPEC.md` is explicit that Lightforge is
    /// WoW-family-primary and a pass-through for the rest, so these are
    /// listed and started, never downloaded or updated here.
    HandOff,
}

impl Kind {
    /// Which side of that line a product code falls on.
    ///
    /// By code shape rather than by a table, so a product Blizzard ships
    /// after this binary lands in the right half. A custom server whose code
    /// does not mention `wow` is read as a hand-off, which is the safe way to
    /// be wrong: it offers less rather than promising an install Lightforge
    /// cannot actually perform.
    pub fn of(code: &ProductCode) -> Self {
        match matches!(code, ProductCode::Custom(_)) && !code.as_str().starts_with("wow") {
            true => Self::HandOff,
            false => Self::Managed,
        }
    }
}

/// One product in the catalog: what it is, and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub code: ProductCode,
    /// Stable display name. Never an expansion name; see [`ProductLine`].
    pub label: String,
    pub line: ProductLine,
    pub branch: Branch,
    pub kind: Kind,
}

/// Which products exist, what they are called, and how they group.
///
/// Built into the binary, then overridden by the hosted manifest. The split
/// matters: Blizzard publishes no product listing, `tact::version` documents
/// the dead `summary` endpoint, so the set of products is something someone
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
        let wow = KNOWN_CODES.iter().map(|code| CatalogEntry {
            // A live branch is the line itself, and its label names the tile
            // and the Add a game row: "Forever", not "Forever Live".
            label: match code.branch() {
                Branch::Live => code.line().label().to_owned(),
                branch => format!("{} {}", code.line().label(), branch.label()),
            },
            line: code.line(),
            branch: code.branch(),
            kind: Kind::of(code),
            code: code.clone(),
        });

        // Everything else Blizzard publishes, from the table the version
        // service already probes against. This is why that table and this
        // catalog are one list rather than two that drift.
        //
        // Filtered by what is already covered rather than by kind: the table
        // holds WoW products that are not `ProductCode` variants,
        // `wow_anniversary` is one, and dropping those would lose them from
        // the catalog altogether.
        let rest = crate::tact::version::KNOWN_PRODUCTS
            .iter()
            .map(|(code, label)| (ProductCode::from((*code).to_owned()), *label))
            .filter(|(code, _)| !KNOWN_CODES.contains(code))
            .map(|(code, label)| CatalogEntry {
                label: label.to_owned(),
                line: code.line(),
                branch: code.branch(),
                kind: Kind::of(&code),
                code,
            });

        Self { entries: wow.chain(rest).collect() }
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

    /// Adopt the product codes Battle.net itself uses.
    ///
    /// `version::KNOWN_PRODUCTS` carries `hsb` for Hearthstone; the client and
    /// `product.db` both say `hs_beta`. Nothing lines up while the catalog
    /// holds one and the machine holds the other, the browse panel adds a
    /// code that never matches what is installed.
    ///
    /// Two codes are the same game when they resolve to the same Battle.net
    /// game family, which is the client's own notion of identity and the only
    /// one available: the codes themselves share no pattern.
    pub fn reconcile(&mut self, known: &[String]) {
        for entry in &mut self.entries {
            if entry.kind != Kind::HandOff {
                continue;
            }
            let Some(family) = crate::battlenet::game_uid(&entry.code) else {
                continue;
            };
            let theirs = known.iter().find(|code| {
                code.as_str() != entry.code.as_str()
                    && crate::battlenet::game_uid(&ProductCode::from((*code).clone()))
                        .as_deref()
                        == Some(family.as_str())
            });
            if let Some(code) = theirs {
                entry.code = ProductCode::from(code.clone());
            }
        }
        // Two keys can name one game. The asset manifest carries both `hsb`
        // and `hs_beta` on purpose, artwork is looked up by whichever code
        // the machine reports, and every manifest key becomes a catalog
        // entry, so Hearthstone arrives twice and the rewrite above makes the
        // two identical. Keeping the first is what collapses the duplicate
        // row in Add a game.
        let mut seen: HashSet<ProductCode> = HashSet::new();
        self.entries.retain(|entry| seen.insert(entry.code.clone()));
    }

    /// Products Lightforge downloads itself.
    pub fn managed(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter().filter(|e| e.kind == Kind::Managed)
    }

    /// Products that are Battle.net's to install and start.
    pub fn hand_offs(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter().filter(|e| e.kind == Kind::HandOff)
    }

    /// Every branch of one line, live first, in catalog order.
    pub fn branches(&self, line: &ProductLine) -> Vec<&CatalogEntry> {
        let mut found: Vec<&CatalogEntry> =
            self.entries.iter().filter(|e| &e.line == line).collect();
        // The head first, which is the live branch everywhere but Forever.
        let head = line.head();
        found.sort_by_key(|e| {
            let rank = match e.branch {
                Branch::Live => 0,
                Branch::Ptr => 1,
                Branch::Ptr2 => 2,
                Branch::Beta => 3,
            };
            (e.code != head, rank)
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
    /// Id of the storage library this product is installed in. `None` means
    /// the default one, which is also what a fresh product gets, the
    /// assignment is written when the install is queued, not before.
    pub library: Option<String>,
    /// Contents written to the product's `realmlist`/portal config, if set.
    pub realmlist_override: Option<String>,
    /// CDN/patch endpoint override used by the downloader, if set.
    pub cdn_override: Option<String>,
    /// Launch settings for this product: wrappers and environment.
    pub tweaks: crate::tweaks::Tweaks,
    /// Optional content opted in to, by download-manifest tag name, e.g.
    /// `HighRes`. Empty matches Battle.net, which installs none of it unless
    /// asked.
    pub optional_content: Vec<String>,
    /// What the last plan said this product offers. A cache, not a setting:
    /// the list is only knowable after reading the build's download manifest,
    /// so the panel would have nothing to show before the first refresh.
    pub optional_available: Vec<String>,
    /// The language this product is installed in, e.g. `deDE`. `None` follows
    /// [`crate::config::Config::locale`], the one chosen at setup.
    pub locale: Option<String>,
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
            assert_eq!(parsed.as_str(), code);
        }
    }

    #[test]
    fn unknown_code_becomes_custom() {
        let parsed: ProductCode = "turtle_wow".parse().unwrap();
        assert_eq!(parsed, ProductCode::Custom("turtle_wow".into()));
        assert_eq!(parsed.install_dir(), "turtle_wow");
        // A code shaped like a WoW flavor gets a WoW flavor directory.
        let future: ProductCode = "wow_midnight_ptr".parse().unwrap();
        assert_eq!(future.install_dir(), "_midnight_ptr_");
    }

    #[test]
    fn the_catalog_is_one_list_rather_than_two_that_drift() {
        let catalog = Catalog::builtin();

        // The WoW family, which Lightforge downloads.
        for code in KNOWN_CODES {
            let entry = catalog.get(code).unwrap_or_else(|| panic!("{}", code.as_str()));
            assert_eq!(entry.kind, Kind::Managed, "{}", code.as_str());
        }

        // And everything the version service probes for, which it does not.
        for (code, label) in crate::tact::version::KNOWN_PRODUCTS {
            let code = ProductCode::from((*code).to_owned());
            let entry = catalog.get(&code).unwrap_or_else(|| panic!("{}", code.as_str()));
            if entry.kind == Kind::HandOff {
                assert_eq!(&entry.label, label, "{}", code.as_str());
            }
        }

        assert!(catalog.hand_offs().any(|e| e.label == "Hearthstone"));
        assert!(catalog.managed().any(|e| e.code == ProductCode::Retail));
    }

    #[test]
    fn the_clients_own_code_wins_over_our_table() {
        let mut catalog = Catalog::builtin();
        // What our table calls Hearthstone before anyone asks the client.
        assert!(catalog.get(&ProductCode::from("hsb".to_owned())).is_some());

        catalog.reconcile(&["battle_net".into(), "hs_beta".into(), "wow".into()]);

        // Now it is the code `product.db` and the browse panel will agree on.
        assert!(catalog.get(&ProductCode::from("hs_beta".to_owned())).is_some());
        assert!(catalog.get(&ProductCode::from("hsb".to_owned())).is_none());

        // And a managed product is never rewritten by what Battle.net thinks.
        assert!(catalog.get(&ProductCode::Retail).is_some());
    }

    #[test]
    fn one_game_under_two_manifest_keys_is_one_row() {
        // The asset manifest states Hearthstone twice, under both codes, so
        // that either one finds its artwork. That must not become two rows.
        let stated = ["hsb", "hs_beta"].map(|code| {
            let code = ProductCode::from(code.to_owned());
            CatalogEntry {
                label: "Hearthstone".to_owned(),
                line: code.line(),
                branch: code.branch(),
                kind: Kind::of(&code),
                code,
            }
        });
        let mut catalog = Catalog::with(stated);
        assert_eq!(catalog.hand_offs().filter(|e| e.label == "Hearthstone").count(), 2);

        catalog.reconcile(&["battle_net".into(), "hs_beta".into()]);

        let rows: Vec<&CatalogEntry> =
            catalog.hand_offs().filter(|e| e.label == "Hearthstone").collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].code, ProductCode::from("hs_beta".to_owned()));
    }

    #[test]
    fn reconciling_against_an_unknown_machine_changes_nothing() {
        let mut catalog = Catalog::builtin();
        let before = catalog.clone();
        // No Battle.net config, or a client that has never seen these games.
        catalog.reconcile(&[]);
        assert_eq!(catalog, before);
    }

    #[test]
    fn what_lightforge_will_not_install_is_decided_by_the_code_shape() {
        // Blizzard's other titles.
        for code in ["hsb", "d3", "fenris", "pro"] {
            assert_eq!(Kind::of(&ProductCode::from(code.to_owned())), Kind::HandOff, "{code}");
        }
        // The WoW family, known and not yet known alike.
        for code in ["wow", "wowt", "wow_classic_era", "wow_midnight_ptr", "wow_anniversary"] {
            assert_eq!(Kind::of(&ProductCode::from(code.to_owned())), Kind::Managed, "{code}");
        }
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
    fn a_lines_head_is_its_first_branch() {
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

    /// Measured from `wowf`'s product config, 2026-10-14. As an unknown code
    /// it would have installed into a literal `wowf/`, where no client looks.
    #[test]
    fn forever_installs_where_its_product_config_says() {
        let code: ProductCode = "wowf".parse().unwrap();
        assert_eq!(code, ProductCode::Forever);
        assert_eq!(code.install_dir(), "_forever_");
        assert_eq!(code.executable(), "Wow.exe");
        assert_eq!(code.line(), ProductLine::Forever);
        assert_eq!(code.branch(), Branch::Live);
        assert_eq!(ProductLine::Forever.head(), ProductCode::Forever, "live leads, the beta is retired");
        assert_eq!(ProductLine::Forever.branches(), vec![ProductCode::Forever, ProductCode::ClassicBeta]);
    }

    /// Add a game lists only live branches, so this is what puts Forever in it.
    #[test]
    fn the_builtin_catalog_names_a_live_branch_after_its_line() {
        let catalog = Catalog::builtin();
        assert_eq!(catalog.get(&ProductCode::Forever).unwrap().label, "Forever");
        assert_eq!(catalog.get(&ProductCode::ClassicBeta).unwrap().label, "Forever Beta");
    }
}
