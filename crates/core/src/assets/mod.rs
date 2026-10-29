//! Dynamic theming asset pipeline.
//!
//! `manifest.json` is revalidated by ETag each launch, and each image it names
//! has its own ETag, so a manifest change never re-downloads unchanged art.
//! The cache file under [`crate::paths::AppDirs::asset_cache_dir`] is the
//! image itself: a `304` costs one `stat`.
//!
//! Labels are per product (`Retail`), never per expansion.
//!
//! [`Origin::Dir`] reads a local fixture directory through the same parsing
//! and checksum checks.

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
/// **2** made `logo` and `background` optional; a schema-1 client cannot
/// parse an entry without `logo`.
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
