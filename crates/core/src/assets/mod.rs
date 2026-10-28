//! Dynamic theming asset pipeline.
//!
//! Hybrid manifest + per-asset ETags against Tuxcore-hosted branding:
//! `manifest.json` is revalidated with `If-None-Match` on each launch, and the
//! logo/background images it names carry their own independent ETags, so a
//! metadata-only manifest change never re-downloads unchanged art.
//!
//! Cached under [`crate::paths::AppDirs::asset_cache_dir`], where each asset's
//! cache file *is* the image the UI renders, the ETag body and the artwork are
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

mod manifest;
mod pipeline;
mod store;
#[cfg(test)]
mod tests;

pub use manifest::{AssetRef, Manifest, Slot, Theme};
pub use pipeline::Pipeline;
pub use store::{Asset, Branding, Origin, Store};

use std::path::{Path, PathBuf};

use crate::net::NetError;
// The tests reach the pipeline's checksum and path helpers directly.
#[cfg(test)]
use pipeline::*;

/// Manifest schema this build understands. A newer one is refused: the last
/// good cached art beats half-understood theming.
///
/// **2** made `logo` and `background` optional, so a Battle.net hand-off can
/// be themed with just its square mark. Not merely additive: a schema-1 client
/// fails to parse the whole document when an entry omits `logo`, so it must
/// be told to keep its cache.
pub const SCHEMA_VERSION: u32 = 2;

/// File name of the manifest within its base URL or fixture directory.
pub const MANIFEST_FILE: &str = "manifest.json";

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

    #[error("{0} is themed with a mark only — it has no logo or background")]
    NoArtwork(String),

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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> AssetError + '_ {
    move |source| AssetError::Io { path: path.to_path_buf(), source }
}
