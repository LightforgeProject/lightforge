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
use crate::product::ProductCode;

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
    pub logo: AssetRef,
    pub background: AssetRef,
}

/// One image: where it lives and, optionally, what it must hash to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRef {
    /// Absolute, or relative to the manifest's own location.
    pub url: String,
    /// Lowercase hex SHA-256 of the image bytes. Optional, but when present a
    /// mismatch discards rather than caches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Which of a theme's two images, for error messages and cache naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Logo,
    Background,
}

impl Slot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Background => "background",
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
}

/// A product's resolved branding: a label plus two files on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Branding {
    pub label: String,
    pub logo: Asset,
    pub background: Asset,
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

        Ok(Branding {
            label: theme.label.clone(),
            logo: self.asset(http, &theme.logo, Slot::Logo).await?,
            background: self
                .asset(http, &theme.background, Slot::Background)
                .await?,
        })
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
                Ok(Asset { path, fetched: false })
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
                Ok(Asset { path, fetched })
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
