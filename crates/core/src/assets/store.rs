use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::pipeline::{extension, is_cache_name, join};
use crate::text::hex;
use super::MANIFEST_FILE;
use crate::net::ETagCache;

/// Where the manifest and its art are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Base URL holding `manifest.json`, e.g. `https://assets.tuxcore.pro/wow/`.
    Http(String),
    /// Local fixture directory, used until the endpoint is live.
    Dir(PathBuf),
}

impl Origin {
    /// The manifest's own location.
    pub(super) fn manifest(&self) -> String {
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
    /// The manifest's height hint, carried through unverified, see
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
    /// isn't resolvable, a missing mark costs a fallback label, not a theme.
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
    /// sharing one image share one cached file, and so a rename upstream
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

    /// Delete cached art that `keep` does not name.
    ///
    /// Art is content-addressed upstream, so every revision arrives under a new
    /// URL and the old cache file would be stranded forever. Narrow on
    /// purpose: direct children only, names [`Store::cache_name`] could have
    /// produced, never the manifest. **Each kept file keeps its `.etag`
    /// sidecar**: deleting those once made every revalidation re-download and
    /// swap unchanged pictures, a flicker after each product switch.
    pub fn prune(&self, keep: &[String]) -> usize {
        let mut wanted: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(keep.len() * 2);
        for url in keep {
            let name = Self::cache_name(url);
            wanted.insert(format!("{name}.etag"));
            wanted.insert(name);
        }
        wanted.insert(format!("{MANIFEST_FILE}.etag"));

        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };

        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == MANIFEST_FILE || wanted.contains(&name) {
                continue;
            }
            if !is_cache_name(&name) {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_file())
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        removed
    }

    pub(super) fn cache_for(&self, url: &str) -> ETagCache {
        ETagCache::new(self.path_for(url))
    }

    /// The manifest's own cache entry, sibling to the art it names.
    pub(super) fn manifest_cache(&self) -> ETagCache {
        ETagCache::new(self.dir.join(MANIFEST_FILE))
    }
}
