use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{
    io, Asset, AssetError, AssetRef, Branding, Manifest, Origin, Slot, Store, MANIFEST_FILE,
};
use crate::net::Http;
use crate::product::ProductCode;
use crate::text::hex;

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

    /// Drop cached art this manifest no longer names, returning how many
    /// files went. Only with a fresh manifest.
    pub fn prune_cache(&self, manifest: &Manifest) -> usize {
        self.store.prune(&manifest.urls(&self.origin.manifest()))
    }

    /// Fetch the manifest, falling back to the cached copy on `304`. A fixture
    /// is read directly and not cached.
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

    /// Resolve against a manifest already in hand.
    pub async fn resolve_with(
        &self,
        http: &Http,
        manifest: &Manifest,
        code: &ProductCode,
    ) -> Result<Branding, AssetError> {
        let theme = manifest
            .theme(code)
            .ok_or_else(|| AssetError::UnknownProduct(code.as_str().to_owned()))?;

        // A mark-only theme (schema 2) has no drawable [`Branding`].
        let (Some(logo_ref), Some(background_ref)) = (
            manifest.slot(code, |t| t.logo.as_ref()),
            manifest.slot(code, |t| t.background.as_ref()),
        ) else {
            return Err(AssetError::NoArtwork(code.as_str().to_owned()));
        };

        // The two images are fetched together: one round trip.
        let (logo, background) = futures_util::future::try_join(
            self.asset(http, logo_ref, Slot::Logo),
            self.asset(http, background_ref, Slot::Background),
        )
        .await?;

        Ok(Branding {
            label: theme.label.clone(),
            logo,
            background,
            icon: self.icon(http, manifest, code).await?,
        })
    }

    /// Resolve just a product's icon, for the product bar. A failure is
    /// `None`: the bar has a fallback.
    pub async fn icon(
        &self,
        http: &Http,
        manifest: &Manifest,
        code: &ProductCode,
    ) -> Result<Option<Asset>, AssetError> {
        let Some(icon) = manifest.own_theme(code).and_then(|t| t.icon.as_ref()) else {
            return Ok(None);
        };
        Ok(self.asset(http, icon, Slot::Icon).await.ok())
    }

    /// The cache-only half of [`Pipeline::icon`].
    pub fn cached_icon(&self, manifest: &Manifest, code: &ProductCode) -> Option<Asset> {
        let icon = manifest.own_theme(code).and_then(|t| t.icon.as_ref())?;
        self.cached_asset(icon, Slot::Icon)
    }

    /// Resolve branding from what is already on disk, touching no network.
    ///
    /// Paints at once; [`Pipeline::resolve_with`] follows. `None` unless the
    /// cache answers fully. Blocking: off the UI thread.
    pub fn cached(&self, code: &ProductCode) -> Option<Branding> {
        let manifest = self.cached_manifest()?;
        let theme = manifest.theme(code)?;

        Some(Branding {
            label: theme.label.clone(),
            logo: self.cached_asset(manifest.slot(code, |t| t.logo.as_ref())?, Slot::Logo)?,
            background: self
                .cached_asset(manifest.slot(code, |t| t.background.as_ref())?, Slot::Background)?,
            // Optional, and the product's own mark as [`Pipeline::icon`] reads
            // it, or cache and revalidation differ and the art swaps on open.
            icon: self.cached_icon(&manifest, code),
        })
    }

    /// The manifest from the cache alone, touching no network.
    pub fn cached_manifest(&self) -> Option<Manifest> {
        let bytes = match &self.origin {
            Origin::Dir(dir) => read(&dir.join(MANIFEST_FILE)).ok()?,
            Origin::Http(_) => self.store.manifest_cache().read().ok()?,
        };
        Manifest::parse(&bytes).ok()
    }

    /// One image from the cache, verified as a fetch would be.
    fn cached_asset(&self, asset: &AssetRef, slot: Slot) -> Option<Asset> {
        let path = match &self.origin {
            Origin::Dir(dir) => resolve_fixture(dir, &asset.url, slot).ok()?,
            Origin::Http(base) => self.store.path_for(&join(base, &asset.url)),
        };
        verify(&read(&path).ok()?, asset, slot, &asset.url).ok()?;
        Some(Asset { path, fetched: false, height: asset.height })
    }

    /// Fetch (or revalidate) one image and verify it, cached bytes too: a
    /// `304` never looks at the body.
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
                    // Failed bytes must not become a cache hit.
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
/// A fixture must not theme from the network.
pub(super) fn resolve_fixture(dir: &Path, url: &str, slot: Slot) -> Result<PathBuf, AssetError> {
    if url.contains("://") {
        return Err(AssetError::RemoteInFixture {
            slot,
            url: url.to_owned(),
        });
    }
    Ok(dir.join(url))
}

pub(super) fn verify(bytes: &[u8], asset: &AssetRef, slot: Slot, url: &str) -> Result<(), AssetError> {
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

pub(super) fn read(path: &Path) -> Result<Vec<u8>, AssetError> {
    std::fs::read(path).map_err(io(path))
}

/// Resolve `rel` against `base`, the way a browser would for our shapes:
/// absolute URLs pass through, root-relative paths keep scheme and host, and
/// everything else hangs off the base's directory.
pub(super) fn join(base: &str, rel: &str) -> String {
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

/// Whether a file name is one [`Store::cache_name`] would have produced:
/// sixteen lowercase hex characters, optionally followed by an extension.
///
/// The gate on pruning: nothing else there is ours to delete.
pub(super) fn is_cache_name(name: &str) -> bool {
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    stem.len() == 16 && stem.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Lowercase file extension of a URL path, ignoring query and fragment.
pub(super) fn extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?;
    let ext = name.rsplit_once('.')?.1;
    (!ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| ext.to_ascii_lowercase())
}
