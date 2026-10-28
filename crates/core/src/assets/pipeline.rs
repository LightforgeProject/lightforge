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
    /// files went.
    ///
    /// Call it on a *fresh* manifest: that is the only moment the complete set
    /// of live URLs is known, and pruning against a partial view would delete
    /// art that is still in use.
    pub fn prune_cache(&self, manifest: &Manifest) -> usize {
        self.store.prune(&manifest.urls(&self.origin.manifest()))
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

    /// Resolve against a manifest already in hand, the GUI fetches it once
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

        // Since schema 2 a theme may carry nothing but a mark, which is all a
        // Battle.net hand-off needs. [`Branding`] means drawable art, so a
        // theme without it is not partial branding. It is none, and the
        // caller is asking the wrong question.
        let (Some(logo_ref), Some(background_ref)) = (
            manifest.slot(code, |t| t.logo.as_ref()),
            manifest.slot(code, |t| t.background.as_ref()),
        ) else {
            return Err(AssetError::NoArtwork(code.as_str().to_owned()));
        };

        // The two images are independent requests against the same host, so
        // they go out together: on a warm cache this halves the theming pass
        // to a single round trip, and on a cold one it halves the transfer.
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
    /// The warm cache paints at once; [`Pipeline::resolve_with`] follows and
    /// swaps in whatever changed. `None` unless the cache can answer fully
    /// (manifest, theme, every image present and matching its checksum):
    /// partial branding cannot be drawn. Blocking IO: call it off the UI thread.
    pub fn cached(&self, code: &ProductCode) -> Option<Branding> {
        let manifest = self.cached_manifest()?;
        let theme = manifest.theme(code)?;

        Some(Branding {
            label: theme.label.clone(),
            logo: self.cached_asset(manifest.slot(code, |t| t.logo.as_ref())?, Slot::Logo)?,
            background: self
                .cached_asset(manifest.slot(code, |t| t.background.as_ref())?, Slot::Background)?,
            // Optional, so a `?` here would throw away a perfectly good theme
            // over a mark the bar can do without. The product's *own* mark,
            // as [`Pipeline::icon`] reads it: a theme borrowed from its line
            // carries the line head's icon, and answering with that here
            // while the revalidation answers with none made the two
            // brandings differ, so the page swapped its art a second or two
            // after opening.
            icon: self.cached_icon(&manifest, code),
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
    /// verifies it, bytes that would be rejected after a fetch must not be
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
    /// download, a file truncated on disk after the fact would otherwise be
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
/// The gate on pruning. Anything else in that directory was not put there by
/// this module and is not this module's to delete.
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
