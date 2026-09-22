//! Dynamic theming asset pipeline — **Milestone 5**.
//!
//! Hybrid manifest + per-asset ETags against Tuxcore-hosted branding:
//! `manifest.json` is revalidated with `If-None-Match` on each launch, and the
//! logo/background images it names carry their own independent ETags, so a
//! metadata-only manifest change never re-downloads unchanged art.
//!
//! Cached under [`crate::paths::AppDirs::asset_cache_dir`], where each asset's
//! cache file *is* the image the UI renders — the ETag body and the artwork are
//! the same bytes, so a `304` costs one `stat` and nothing else.
//!
//! Labels are per-product and stable (`Retail`, `Classic`, `Classic Era`), not
//! per-expansion. The art behind `Retail` changes every expansion; the word the
//! UI prints does not, so nothing has to ship an app update to keep reading
//! correctly.
//!
//! The endpoint is not live yet, so [`Origin::Dir`] resolves a manifest and its
//! art straight out of a local fixture directory. Both origins run the same
//! parsing, product lookup and checksum verification; only the two fetch points
//! differ.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::net::{ETagCache, Http, NetError};
use crate::product::{Branch, Catalog, CatalogEntry, ProductCode};

/// Manifest schema this build understands.
///
/// A manifest declaring a newer schema is refused rather than parsed
/// optimistically: the cached art from the last good fetch is a better answer
/// than half-understood theming.
pub const SCHEMA_VERSION: u32 = 1;

/// File name of the manifest within its base URL or fixture directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The themed branding document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// Product code whose theme covers anything `products` doesn't name —
    /// PTR, beta, and user-added custom products.
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub products: BTreeMap<String, Theme>,
}

/// One product's branding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    /// Stable display name — `Retail`, never `Midnight`.
    pub label: String,
    /// Which game this product is a build of — `retail`, `classic`,
    /// `classic_era`, `forever`, or a name of your own.
    ///
    /// Optional. Without it the client works the line out from the product
    /// code, which it can only do for codes it already knows: this field is
    /// how a product released after the binary lands in the right tile
    /// instead of appearing as a game of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Which build of that line — `live`, `ptr`, `ptr2` or `beta`. Optional,
    /// on the same terms as `line`. A value outside that set is ignored
    /// rather than guessed at; the set is closed and the UI has to render it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub logo: AssetRef,
    pub background: AssetRef,
    /// Small square mark for the dashboard's product bar.
    ///
    /// Optional, unlike the other two: the bar falls back to
    /// [`ProductCode::short_name`] for a product with no icon yet, so art can
    /// be added one product at a time without the bar breaking in between.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<AssetRef>,
}

/// One image: where it lives, optionally what it must hash to, and optionally
/// how tall to draw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRef {
    /// Absolute, or relative to the manifest's own location.
    pub url: String,
    /// Lowercase hex SHA-256 of the image bytes. Optional, but when present a
    /// mismatch discards rather than caches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Logical height to draw this image at, in pixels. Only meaningful for
    /// [`Slot::Logo`]; a background is always drawn to fill.
    ///
    /// Logos are drawn to a fixed height, so two of them at the same height
    /// but different aspect ratios read as different sizes — a wide wordmark
    /// looks twice the size of a square one. How much room a particular piece
    /// of art wants is a property of that art, so it is declared next to it
    /// rather than guessed at by the client. Absent means the app's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

/// Which of a theme's two images, for error messages and cache naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Logo,
    Background,
    Icon,
}

impl Slot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Background => "background",
            Self::Icon => "icon",
        }
    }
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Manifest {
    /// Parse and check the schema version in one step.
    pub fn parse(bytes: &[u8]) -> Result<Self, AssetError> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(AssetError::Parse)?;
        if manifest.schema > SCHEMA_VERSION {
            return Err(AssetError::UnsupportedSchema {
                found: manifest.schema,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(manifest)
    }

    /// The theme for a product: its own entry, else the one `default` names.
    ///
    /// A `default` pointing at a product that isn't in the map is treated as
    /// absent — a broken pointer shouldn't read as a themed product.
    pub fn theme(&self, code: &ProductCode) -> Option<&Theme> {
        self.products.get(code.as_str()).or_else(|| {
            self.default
                .as_deref()
                .and_then(|fallback| self.products.get(fallback))
        })
    }
}

impl Manifest {
    /// The product catalog this manifest states, over the built-in one.
    ///
    /// A product is only *stated* here — identity, grouping, what to call it.
    /// Where its files go and what runs stays derived from the code, so a bad
    /// publish can mislabel a product but never install one over another.
    pub fn catalog(&self) -> Catalog {
        Catalog::with(self.products.iter().map(|(code, theme)| {
            let code = ProductCode::from(code.clone());
            CatalogEntry {
                line: match theme.line.as_deref() {
                    Some(line) => line.parse().unwrap_or_else(|never| match never {}),
                    None => code.line(),
                },
                // An unparseable branch falls back rather than failing the
                // entry: a product in the wrong slot of a version list still
                // beats a product missing from the launcher.
                branch: theme
                    .branch
                    .as_deref()
                    .and_then(|b| b.parse::<Branch>().ok())
                    .unwrap_or_else(|| code.branch()),
                kind: crate::product::Kind::of(&code),
                label: theme.label.clone(),
                code,
            }
        }))
    }
}

/// Where the manifest and its art are read from.
#[derive(Debug, Clone)]
pub enum Origin {
    /// Base URL holding `manifest.json`, e.g. `https://assets.tuxcore.pro/wow/`.
    Http(String),
    /// Local fixture directory, used until the endpoint is live.
    Dir(PathBuf),
}

impl Origin {
    /// The manifest's own location.
    fn manifest(&self) -> String {
        match self {
            Self::Http(base) => join(base, MANIFEST_FILE),
            Self::Dir(dir) => dir.join(MANIFEST_FILE).display().to_string(),
        }
    }
}

/// An image the UI can render, and whether this run fetched it.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub path: PathBuf,
    /// `true` when bytes moved this run; `false` on a `304` or a fixture read.
    pub fetched: bool,
    /// The manifest's height hint, carried through unverified — see
    /// [`AssetRef::height`]. The UI decides what is a sane value.
    pub height: Option<f32>,
}

/// A product's resolved branding: a label plus two files on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Branding {
    pub label: String,
    pub logo: Asset,
    pub background: Asset,
    /// `None` when the theme declares no icon, or when one is declared but
    /// isn't resolvable — a missing mark costs a fallback label, not a theme.
    pub icon: Option<Asset>,
}

/// The cache directory, and the naming rule within it.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Cache file name for a URL.
    ///
    /// Keyed by the URL rather than by product and slot, so two products
    /// sharing one image share one cached file — and so a rename upstream
    /// lands as a new file instead of overwriting art that other products may
    /// still be pointing at.
    pub fn cache_name(url: &str) -> String {
        let digest = Sha256::digest(url.as_bytes());
        let stem: String = hex(&digest)[..16].to_owned();
        match extension(url) {
            Some(ext) => format!("{stem}.{ext}"),
            None => stem,
        }
    }

    pub fn path_for(&self, url: &str) -> PathBuf {
        self.dir.join(Self::cache_name(url))
    }

    fn cache_for(&self, url: &str) -> ETagCache {
        ETagCache::new(self.path_for(url))
    }

    /// The manifest's own cache entry, sibling to the art it names.
    fn manifest_cache(&self) -> ETagCache {
        ETagCache::new(self.dir.join(MANIFEST_FILE))
    }
}

/// The pipeline: an origin, a cache, and the rules connecting them.
#[derive(Debug, Clone)]
pub struct Pipeline {
    origin: Origin,
    store: Store,
}

impl Pipeline {
    pub fn new(origin: Origin, store: Store) -> Self {
        Self { origin, store }
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Fetch the manifest, falling back to the cached copy on `304`.
    ///
    /// A fixture origin reads the file directly and caches nothing: the
    /// fixture *is* the cache, and copying it would only create a second copy
    /// to go stale.
    pub async fn manifest(&self, http: &Http) -> Result<Manifest, AssetError> {
        match &self.origin {
            Origin::Dir(dir) => {
                let path = dir.join(MANIFEST_FILE);
                Manifest::parse(&read(&path)?)
            }
            Origin::Http(_) => {
                let url = self.origin.manifest();
                let cache = self.store.manifest_cache();
                match http.get_bytes_cached(&url, &cache).await? {
                    Some(body) => Manifest::parse(&body),
                    None => Manifest::parse(&cache.read()?),
                }
            }
        }
    }

    /// Resolve one product's branding: manifest, then both images.
    pub async fn resolve(
        &self,
        http: &Http,
        code: &ProductCode,
    ) -> Result<Branding, AssetError> {
        let manifest = self.manifest(http).await?;
        self.resolve_with(http, &manifest, code).await
    }

    /// Resolve against a manifest already in hand — the GUI fetches it once
    /// and themes every product from it.
    pub async fn resolve_with(
        &self,
        http: &Http,
        manifest: &Manifest,
        code: &ProductCode,
    ) -> Result<Branding, AssetError> {
        let theme = manifest
            .theme(code)
            .ok_or_else(|| AssetError::UnknownProduct(code.as_str().to_owned()))?;

        // The two images are independent requests against the same host, so
        // they go out together: on a warm cache this halves the theming pass
        // to a single round trip, and on a cold one it halves the transfer.
        let (logo, background) = futures_util::future::try_join(
            self.asset(http, &theme.logo, Slot::Logo),
            self.asset(http, &theme.background, Slot::Background),
        )
        .await?;

        Ok(Branding {
            label: theme.label.clone(),
            logo,
            background,
            icon: self.icon(http, manifest, code).await?,
        })
    }

    /// Resolve just a product's icon.
    ///
    /// Separate from [`Pipeline::resolve_with`] because the product bar wants
    /// every configured product's mark at once, and pulling three images per
    /// product to use one of them would make switching products cost five
    /// times what it needs to.
    ///
    /// An icon that fails to fetch or verify is `None` rather than an error:
    /// the bar has a fallback, and one bad mark must not stop the others.
    pub async fn icon(
        &self,
        http: &Http,
        manifest: &Manifest,
        code: &ProductCode,
    ) -> Result<Option<Asset>, AssetError> {
        let Some(icon) = manifest.theme(code).and_then(|t| t.icon.as_ref()) else {
            return Ok(None);
        };
        Ok(self.asset(http, icon, Slot::Icon).await.ok())
    }

    /// The cache-only half of [`Pipeline::icon`].
    pub fn cached_icon(&self, manifest: &Manifest, code: &ProductCode) -> Option<Asset> {
        let icon = manifest.theme(code).and_then(|t| t.icon.as_ref())?;
        self.cached_asset(icon, Slot::Icon)
    }

    /// Resolve branding from what is already on disk, touching no network.
    ///
    /// A warm cache holds the exact bytes the UI is about to draw, so making
    /// it wait on a revalidation round trip before painting buys nothing but
    /// a few hundred milliseconds of unthemed dashboard. This hands back the
    /// last good art at once; [`Pipeline::resolve_with`] follows behind and
    /// swaps in whatever actually changed.
    ///
    /// `None` whenever the cache cannot fully answer — a missing manifest, an
    /// unthemed product, an image not fetched yet, or one that no longer
    /// matches its checksum. Partial branding is not a thing the UI can draw,
    /// so a half-warm cache simply waits for the network like a cold one.
    ///
    /// Blocking file IO: call it off the thread that is drawing.
    pub fn cached(&self, code: &ProductCode) -> Option<Branding> {
        let manifest = self.cached_manifest()?;
        let theme = manifest.theme(code)?;

        Some(Branding {
            label: theme.label.clone(),
            logo: self.cached_asset(&theme.logo, Slot::Logo)?,
            background: self.cached_asset(&theme.background, Slot::Background)?,
            // Optional, so a `?` here would throw away a perfectly good theme
            // over a mark the bar can do without.
            icon: theme.icon.as_ref().and_then(|i| self.cached_asset(i, Slot::Icon)),
        })
    }

    /// The manifest from the cache alone, touching no network.
    ///
    /// The product bar needs it separately from [`Pipeline::cached`]: that
    /// resolves one product, the bar draws every one the manifest names.
    pub fn cached_manifest(&self) -> Option<Manifest> {
        let bytes = match &self.origin {
            Origin::Dir(dir) => read(&dir.join(MANIFEST_FILE)).ok()?,
            Origin::Http(_) => self.store.manifest_cache().read().ok()?,
        };
        Manifest::parse(&bytes).ok()
    }

    /// One image from the cache, verified the same way the network path
    /// verifies it — bytes that would be rejected after a fetch must not be
    /// painted just because they arrived before one.
    fn cached_asset(&self, asset: &AssetRef, slot: Slot) -> Option<Asset> {
        let path = match &self.origin {
            Origin::Dir(dir) => resolve_fixture(dir, &asset.url, slot).ok()?,
            Origin::Http(base) => self.store.path_for(&join(base, &asset.url)),
        };
        verify(&read(&path).ok()?, asset, slot, &asset.url).ok()?;
        Some(Asset { path, fetched: false, height: asset.height })
    }

    /// Fetch (or revalidate) one image and verify it.
    ///
    /// The checksum is checked on the cached bytes too, not only on a fresh
    /// download — a file truncated on disk after the fact would otherwise be
    /// served forever, since a `304` never looks at the body.
    async fn asset(
        &self,
        http: &Http,
        asset: &AssetRef,
        slot: Slot,
    ) -> Result<Asset, AssetError> {
        match &self.origin {
            Origin::Dir(dir) => {
                let path = resolve_fixture(dir, &asset.url, slot)?;
                verify(&read(&path)?, asset, slot, &asset.url)?;
                Ok(Asset { path, fetched: false, height: asset.height })
            }
            Origin::Http(base) => {
                let url = join(base, &asset.url);
                let cache = self.store.cache_for(&url);
                let path = self.store.path_for(&url);

                let (bytes, fetched) = match http.get_bytes_cached(&url, &cache).await? {
                    Some(body) => (body, true),
                    None => (cache.read()?, false),
                };

                if let Err(e) = verify(&bytes, asset, slot, &url) {
                    // Don't leave bytes that failed verification behind as a
                    // cache hit; the next launch must re-fetch unconditionally.
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(format!("{}.etag", path.display()));
                    return Err(e);
                }
                Ok(Asset { path, fetched, height: asset.height })
            }
        }
    }
}

/// Resolve a fixture asset path, refusing a remote URL.
///
/// A fixture manifest that points at `https://…` would silently theme from
/// somewhere other than the fixture, which defeats the point of testing
/// against one.
fn resolve_fixture(dir: &Path, url: &str, slot: Slot) -> Result<PathBuf, AssetError> {
    if url.contains("://") {
        return Err(AssetError::RemoteInFixture {
            slot,
            url: url.to_owned(),
        });
    }
    Ok(dir.join(url))
}

fn verify(bytes: &[u8], asset: &AssetRef, slot: Slot, url: &str) -> Result<(), AssetError> {
    let Some(expected) = asset.sha256.as_deref() else {
        return Ok(());
    };
    let actual = hex(&Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(AssetError::Checksum {
            slot,
            url: url.to_owned(),
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, AssetError> {
    std::fs::read(path).map_err(|source| AssetError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolve `rel` against `base`, the way a browser would for our shapes:
/// absolute URLs pass through, root-relative paths keep scheme and host, and
/// everything else hangs off the base's directory.
fn join(base: &str, rel: &str) -> String {
    if rel.contains("://") {
        return rel.to_owned();
    }
    if let Some(rest) = rel.strip_prefix('/') {
        if let Some(scheme_end) = base.find("://") {
            let after = scheme_end + 3;
            let authority_end = base[after..].find('/').map_or(base.len(), |i| after + i);
            return format!("{}/{rest}", &base[..authority_end]);
        }
    }
    let dir = match base.rfind('/') {
        Some(i) => &base[..=i],
        None => return format!("{base}/{rel}"),
    };
    format!("{dir}{rel}")
}

/// Lowercase file extension of a URL path, ignoring query and fragment.
fn extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?;
    let ext = name.rsplit_once('.')?.1;
    (!ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| ext.to_ascii_lowercase())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error(transparent)]
    Net(#[from] NetError),

    #[error("could not parse the asset manifest")]
    Parse(#[source] serde_json::Error),

    #[error("manifest declares schema {found}; this build understands {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },

    #[error("the manifest has no theme for {0} and no usable default")]
    UnknownProduct(String),

    #[error("{slot} at {url} hashed to {actual}, expected {expected}")]
    Checksum {
        slot: Slot,
        url: String,
        expected: String,
        actual: String,
    },

    #[error("fixture manifest points {slot} at the network ({url})")]
    RemoteInFixture { slot: Slot, url: String },

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

    const FIXTURE: &str = r#"{
      "schema": 1,
      "default": "wow",
      "products": {
        "wow":             { "label": "Retail",
                             "logo":       { "url": "retail-logo.png" },
                             "background": { "url": "retail-bg.jpg" } },
        "wow_classic_era": { "label": "Classic Era",
                             "logo":       { "url": "era-logo.png" },
                             "background": { "url": "era-bg.jpg" } }
      }
    }"#;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-assets-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest() -> Manifest {
        Manifest::parse(FIXTURE.as_bytes()).unwrap()
    }

    #[test]
    fn labels_are_per_product_and_stable() {
        let m = manifest();
        assert_eq!(m.theme(&ProductCode::Retail).unwrap().label, "Retail");
        assert_eq!(m.theme(&ProductCode::ClassicEra).unwrap().label, "Classic Era");
    }

    #[test]
    fn unnamed_products_fall_back_to_the_default() {
        let m = manifest();
        // PTR and a user-added custom product both theme as Retail.
        assert_eq!(m.theme(&ProductCode::Ptr).unwrap().label, "Retail");
        let custom = ProductCode::Custom("turtle_wow".into());
        assert_eq!(m.theme(&custom).unwrap().label, "Retail");
    }

    #[test]
    fn a_default_naming_a_missing_product_is_not_a_theme() {
        let mut m = manifest();
        m.default = Some("nonexistent".into());
        assert!(m.theme(&ProductCode::Beta).is_none());
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_guessed_at() {
        let body = FIXTURE.replace("\"schema\": 1", "\"schema\": 2");
        assert!(matches!(
            Manifest::parse(body.as_bytes()),
            Err(AssetError::UnsupportedSchema { found: 2, supported: 1 })
        ));
    }

    #[test]
    fn unknown_manifest_fields_are_tolerated() {
        // Future metadata must not break older clients, which is the whole
        // reason the URLs live in a manifest instead of being hard-coded.
        let body = FIXTURE.replace("\"schema\": 1", "\"schema\": 1, \"banners\": []");
        assert!(Manifest::parse(body.as_bytes()).is_ok());
    }

    #[test]
    fn missing_sha256_is_optional() {
        let m = manifest();
        assert!(m.theme(&ProductCode::Retail).unwrap().logo.sha256.is_none());
    }

    #[test]
    fn cache_names_are_url_derived_and_keep_the_extension() {
        let a = Store::cache_name("https://cdn.example/a/retail-logo.png");
        let b = Store::cache_name("https://cdn.example/b/retail-logo.png");
        assert_ne!(a, b, "same file name in different directories must not collide");
        assert!(a.ends_with(".png"));
        assert_eq!(a.len(), 16 + ".png".len());
        // An extension the app does not itself understand still survives to
        // the cache file. It has to: the decoder picks its format from that
        // extension, so an `.ico` cached without one cannot be read back.
        assert!(Store::cache_name("https://cdn.example/retail-icon.ico").ends_with(".ico"));
        assert!(Store::cache_name("https://cdn.example/retail-icon.ICO").ends_with(".ico"));

        // Query strings are not part of the extension, and it is normalized.
        assert!(Store::cache_name("https://cdn.example/bg.JPG?v=2").ends_with(".jpg"));
    }

    #[test]
    fn urls_join_the_way_a_browser_would() {
        let base = "https://assets.tuxcore.pro/wow/manifest.json";
        assert_eq!(join(base, "logo.png"), "https://assets.tuxcore.pro/wow/logo.png");
        assert_eq!(join(base, "/art/logo.png"), "https://assets.tuxcore.pro/art/logo.png");
        assert_eq!(join(base, "https://other/logo.png"), "https://other/logo.png");
        // A base given as a directory keeps its trailing segment.
        assert_eq!(
            join("https://assets.tuxcore.pro/wow/", "logo.png"),
            "https://assets.tuxcore.pro/wow/logo.png"
        );
    }

    #[test]
    fn a_checksum_mismatch_is_an_error_not_a_cache_entry() {
        let asset = AssetRef {
            url: "logo.png".into(),
            sha256: Some("00".repeat(32)),
            height: None,
        };
        let err = verify(b"bytes", &asset, Slot::Logo, "logo.png").unwrap_err();
        assert!(matches!(err, AssetError::Checksum { slot: Slot::Logo, .. }));
    }

    #[test]
    fn a_matching_checksum_passes_in_either_case() {
        let bytes = b"artwork";
        let digest = hex(&Sha256::digest(bytes));
        let asset = AssetRef {
            url: "logo.png".into(),
            sha256: Some(digest.to_uppercase()),
            height: None,
        };
        assert!(verify(bytes, &asset, Slot::Logo, "logo.png").is_ok());
    }

    #[tokio::test]
    async fn a_fixture_resolves_without_touching_the_network() {
        let dir = tmpdir("fixture");
        let logo = b"PNG-logo".to_vec();
        std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
        std::fs::write(dir.join("retail-logo.png"), &logo).unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir.clone()), Store::new(tmpdir("fixture-cache")));
        let http = Http::new().unwrap();
        let branding = pipeline.resolve(&http, &ProductCode::Retail).await.unwrap();

        assert_eq!(branding.label, "Retail");
        assert_eq!(branding.logo.path, dir.join("retail-logo.png"));
        assert!(!branding.logo.fetched);
        assert_eq!(std::fs::read(&branding.logo.path).unwrap(), logo);
    }

    #[test]
    fn a_manifest_states_the_catalog_over_the_builtin_one() {
        let body = r#"{
            "schema": 1,
            "products": {
              "wow_midnight_ptr": {
                "label": "Midnight PTR",
                "line": "retail",
                "branch": "ptr",
                "logo": { "url": "l.png" },
                "background": { "url": "b.jpg" }
              }
            }
        }"#;
        let catalog = Manifest::parse(body.as_bytes()).unwrap().catalog();

        // A code this build never heard of, placed in an existing line.
        let code = ProductCode::Custom("wow_midnight_ptr".into());
        let entry = catalog.get(&code).unwrap();
        assert_eq!(entry.line, crate::product::ProductLine::Retail);
        assert_eq!(entry.branch, Branch::Ptr);
        assert_eq!(entry.label, "Midnight PTR");

        // And it joins retail's version list rather than standing alone —
        // which is the whole point of stating it.
        let retail = catalog.branches(&crate::product::ProductLine::Retail);
        assert!(retail.iter().any(|e| e.code == code));
        assert!(retail.iter().any(|e| e.code == ProductCode::Retail));

        // Everything the binary knows is still there, unmentioned.
        assert!(catalog.get(&ProductCode::ClassicEra).is_some());
    }

    #[test]
    fn a_branch_the_client_cannot_render_falls_back_instead_of_vanishing() {
        let body = r#"{
            "schema": 1,
            "products": {
              "wowt": {
                "label": "Retail PTR",
                "branch": "pre-alpha",
                "logo": { "url": "l.png" },
                "background": { "url": "b.jpg" }
              }
            }
        }"#;
        let catalog = Manifest::parse(body.as_bytes()).unwrap().catalog();
        let entry = catalog.get(&ProductCode::Ptr).unwrap();
        // The stated branch is nonsense, so the code's own answer stands.
        assert_eq!(entry.branch, Branch::Ptr);
        assert_eq!(entry.label, "Retail PTR");
    }

    #[test]
    fn a_theme_without_an_icon_is_still_a_theme() {
        let dir = tmpdir("no-icon");
        std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("no-icon-c")));
        let branding = pipeline.cached(&ProductCode::Retail).unwrap();
        assert!(branding.icon.is_none());
    }

    #[test]
    fn a_declared_icon_resolves_beside_the_rest() {
        let dir = tmpdir("icon");
        let body = FIXTURE.replace(
            "\"background\": { \"url\": \"retail-bg.jpg\" }",
            "\"background\": { \"url\": \"retail-bg.jpg\" }, \"icon\": { \"url\": \"retail-icon.png\" }",
        );
        std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();
        std::fs::write(dir.join("retail-icon.png"), b"PNG-icon").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir.clone()), Store::new(tmpdir("icon-c")));
        let manifest = pipeline.cached_manifest().unwrap();

        let icon = pipeline.cached_icon(&manifest, &ProductCode::Retail).unwrap();
        assert_eq!(icon.path, dir.join("retail-icon.png"));
        assert_eq!(
            pipeline.cached(&ProductCode::Retail).unwrap().icon,
            Some(icon)
        );
    }

    #[test]
    fn a_declared_icon_that_is_missing_costs_only_the_icon() {
        let dir = tmpdir("icon-gone");
        let body = FIXTURE.replace(
            "\"background\": { \"url\": \"retail-bg.jpg\" }",
            "\"background\": { \"url\": \"retail-bg.jpg\" }, \"icon\": { \"url\": \"retail-icon.png\" }",
        );
        std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();
        // No icon file: the bar falls back, the theme survives.

        let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("icon-gone-c")));
        let branding = pipeline.cached(&ProductCode::Retail).unwrap();
        assert_eq!(branding.label, "Retail");
        assert!(branding.icon.is_none());
    }

    #[test]
    fn a_logo_height_hint_survives_the_round_trip() {
        let dir = tmpdir("logo-height");
        let body = FIXTURE.replace(
            "\"url\": \"retail-logo.png\"",
            "\"url\": \"retail-logo.png\", \"height\": 150.0",
        );
        std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("logo-height-c")));
        let branding = pipeline.cached(&ProductCode::Retail).unwrap();

        assert_eq!(branding.logo.height, Some(150.0));
        // A background has no use for one, and none was given.
        assert_eq!(branding.background.height, None);
    }

    #[test]
    fn a_manifest_without_height_hints_still_parses() {
        let manifest = Manifest::parse(FIXTURE.as_bytes()).unwrap();
        let theme = manifest.theme(&ProductCode::Retail).unwrap();
        assert_eq!(theme.logo.height, None);
    }

    #[test]
    fn the_cache_paints_before_the_network_is_asked() {
        let dir = tmpdir("cached-hit");
        std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir.clone()), Store::new(tmpdir("cached-hit-c")));
        let branding = pipeline.cached(&ProductCode::Retail).unwrap();

        assert_eq!(branding.label, "Retail");
        assert_eq!(branding.logo.path, dir.join("retail-logo.png"));
        assert!(!branding.logo.fetched);
    }

    #[test]
    fn half_a_cache_is_no_cache() {
        let dir = tmpdir("cached-partial");
        std::fs::write(dir.join(MANIFEST_FILE), FIXTURE).unwrap();
        // Logo only: the background was never fetched.
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("cached-partial-c")));
        assert!(pipeline.cached(&ProductCode::Retail).is_none());
    }

    #[test]
    fn the_cache_refuses_bytes_a_fetch_would_have_refused() {
        let dir = tmpdir("cached-checksum");
        // A checksum the files cannot match, standing in for truncation.
        let body = FIXTURE.replace(
            "\"url\": \"retail-logo.png\"",
            "\"url\": \"retail-logo.png\", \"sha256\": \"00\"",
        );
        std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();
        std::fs::write(dir.join("retail-logo.png"), b"PNG-logo").unwrap();
        std::fs::write(dir.join("retail-bg.jpg"), b"JPG-bg").unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("cached-checksum-c")));
        assert!(pipeline.cached(&ProductCode::Retail).is_none());
    }

    #[test]
    fn an_empty_cache_is_not_an_error() {
        let pipeline = Pipeline::new(
            Origin::Http("https://assets.example/wow/".to_owned()),
            Store::new(tmpdir("cached-cold")),
        );
        assert!(pipeline.cached(&ProductCode::Retail).is_none());
    }

    #[tokio::test]
    async fn a_fixture_pointing_at_the_network_is_refused() {
        let dir = tmpdir("fixture-remote");
        let body = FIXTURE.replace("\"url\": \"retail-logo.png\"", "\"url\": \"https://x/l.png\"");
        std::fs::write(dir.join(MANIFEST_FILE), body).unwrap();

        let pipeline = Pipeline::new(Origin::Dir(dir), Store::new(tmpdir("fixture-remote-cache")));
        let http = Http::new().unwrap();
        let err = pipeline.resolve(&http, &ProductCode::Retail).await.unwrap_err();
        assert!(matches!(err, AssetError::RemoteInFixture { .. }));
    }
}
