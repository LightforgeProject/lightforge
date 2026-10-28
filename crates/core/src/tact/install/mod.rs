//! Driving a whole install: version service -> plan -> download -> store.
//!
//! Shared by both front ends, reporting through [`Event`]s. The order matters:
//!
//! 1. Archive indices are cached first. Nothing can be *located* without them.
//! 2. What the store already holds is subtracted, so a download resumes.
//! 3. `.build.info` is written **last, and only on a complete run**:
//!    [`crate::state::Local`] reads it as proof, and a partial run would offer
//!    PLAY on a broken game. See [`Summary::complete`].

mod execute;
mod locale;
mod prepare;
#[cfg(test)]
mod tests;

pub use execute::{execute, run};
pub use locale::{current_locale, locale_name, LOCALES};
pub use prepare::{offerings, prepare, Offerings, Prepared, StoreProbe};

use std::path::{Path, PathBuf};

use crate::tact::download::{self, Progress};
use crate::tact::indices::Step;
use crate::tact::version::Region;
use crate::tact::writer::Summary as StoreSummary;
// The tests reach the locale rules directly.
#[cfg(test)]
use locale::*;

/// How a run is tuned. The limits are for development: any of them makes the
/// run partial, and [`Summary::complete`] then refuses `.build.info`.
#[derive(Debug, Clone)]
pub struct Options {
    pub region: Region,
    /// Client locale, e.g. `enUS`. Selects which of a build's files are wanted.
    pub locale: String,
    /// Optional content to opt in to, by tag name, e.g. `HighRes`. See
    /// [`crate::tact::manifest::TagSet::with_optional`].
    pub optional_tags: Vec<String>,
    /// Flavor directory the install files are extracted into, e.g. `_retail_`.
    /// Empty: the one the product config names, else the code's
    /// ([`crate::product::ProductCode::install_dir`]). An existing install's
    /// is passed, so an update never moves it.
    pub flavor: String,
    pub concurrency: usize,
    /// Cap on download speed in bytes per second; `0` is no cap.
    pub limit_bytes_per_second: u64,
    /// Ranges this far apart are merged into one request.
    pub gap_bytes: u32,
    /// Upper bound on the checkpoint interval; scaled down for small products.
    pub checkpoint_bytes: u64,
    /// Extract the install manifest's files (the game executable and friends).
    pub install_files: bool,
    /// Pre-download: the build `/bgdl` lists, into the store only. No install
    /// files, no `.build.info` and no `.product.db`, so the installed build
    /// stays what launches; a complete run is recorded in [`crate::tact::staged`].
    pub stage: bool,
    /// Stop after this many archive indices, development only.
    pub limit_archives: Option<usize>,
    /// Truncate the plan to this many coalesced runs, development only.
    pub limit_runs: Option<usize>,
    /// Skip the loose (unarchived) files, development only.
    pub skip_loose: bool,
    /// The install's path as Windows sees it, e.g.
    /// `C:/Program Files (x86)/World of Warcraft`. `None` skips `.product.db`.
    pub windows_path: Option<String>,
    /// Agent's directory, whose `product.db` decides whether Battle.net shows
    /// Play or Install. `None` skips registering (no Battle.net, or a CLI run
    /// against a bare directory).
    pub agent_dir: Option<PathBuf>,
    /// Account and GeoIP country, as Agent would record them. Empty is fine.
    pub account_country: String,
    pub geo_country: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            region: Region::default(),
            locale: "enUS".to_owned(),
            optional_tags: Vec::new(),
            flavor: String::new(),
            // 32 measured 2.4x the throughput of 8 on a real product: these
            // transfers are request-latency-bound, not bandwidth-bound.
            concurrency: 32,
            limit_bytes_per_second: 0,
            gap_bytes: 64 * 1024,
            checkpoint_bytes: 4096 * 1024 * 1024,
            install_files: true,
            stage: false,
            limit_archives: None,
            limit_runs: None,
            skip_loose: false,
            windows_path: Some(crate::paths::WINDOWS_WOW_PATH.to_owned()),
            agent_dir: None,
            account_country: String::new(),
            geo_country: String::new(),
        }
    }
}

impl Options {
    /// The tag expression a `.build.info` carries for this machine.
    pub fn tag_column(&self) -> String {
        self.tag_column_for(&self.locale)
    }

    /// [`Options::tag_column`] for a locale other than the one asked for,
    /// see [`shipped_locale`].
    fn tag_column_for(&self, locale: &str) -> String {
        let r = self.region.as_str().to_uppercase();
        format!("Windows x86_64 {r} {locale} speech:Windows x86_64 {r} {locale} text")
    }

    /// True when any option truncates the plan.
    pub fn limited(&self) -> bool {
        self.limit_archives.is_some() || self.limit_runs.is_some() || self.skip_loose
    }
}

/// What the run is doing, as it happens.
#[derive(Debug, Clone)]
pub enum Event {
    Build {
        versions_name: String,
        build_config: String,
        archives: usize,
    },
    Encoding {
        entries: usize,
    },
    Manifests {
        download: usize,
        install: usize,
        unknown_tags: Vec<String>,
    },
    Wanted {
        files: usize,
        bytes: u64,
    },
    /// One step of the archive-index prelude; `done`/`total` count indices.
    Indices(Step),
    IndicesDone {
        cached: usize,
        downloaded: usize,
        bytes: u64,
    },
    /// The install root refused a file and its owner-write bit was added:
    /// a change to the user's directory, so it is reported.
    Repaired {
        dir: PathBuf,
    },
    /// Optional content this build offers, by tag name.
    Optional {
        names: Vec<String>,
    },
    Loose {
        files: usize,
    },
    Located {
        located: usize,
        wanted: usize,
    },
    Planned {
        requests: usize,
        files: usize,
        bytes: u64,
        padding: u64,
        already_present: usize,
        missing: usize,
    },
    /// Transfer progress, once per stored file: coalesce before redrawing.
    Progress(Progress),
    InstallFile {
        path: PathBuf,
        bytes: u64,
    },
    Extracted {
        files: usize,
        bytes: u64,
    },
    BuildInfo {
        path: PathBuf,
        version: String,
    },
    /// A pre-download finished: `version` is in the store, waiting for release.
    Staged {
        version: String,
    },
    ProductDb {
        path: PathBuf,
    },
    /// The product was merged into Agent's registry, so Battle.net lists it
    /// as installed without being asked to look for it.
    Registered {
        code: String,
    },
    /// The run finished short of a complete install, and why.
    Incomplete {
        reasons: Vec<String>,
    },
}

/// What a run achieved.
#[derive(Debug, Clone)]
pub struct Summary {
    pub outcome: download::Outcome,
    pub store: StoreSummary,
    /// Keys the plan could not locate in any archive or the file index.
    pub missing: usize,
    /// True only if this run produced a whole, launchable install, the same
    /// condition that gates `.build.info`.
    pub complete: bool,
    /// Why the run is not complete, each with the first cause where there is
    /// one, e.g. `3 files failed (first: connection reset)`. Empty when it is.
    pub reasons: Vec<String>,
    /// Present when `complete`; the version `.build.info` now names. `None`
    /// after a pre-download, which names none.
    pub version: Option<String>,
    /// The folder the game went into, and its binary's name when the product
    /// config said: what the front end records for the paths to use.
    pub flavor: String,
    pub executable: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Version(#[from] crate::tact::version::VersionError),

    #[error(transparent)]
    Cdn(#[from] crate::tact::cdn::CdnError),

    #[error(transparent)]
    Config(#[from] crate::tact::config::ConfigError),

    #[error(transparent)]
    Manifest(#[from] crate::tact::manifest::ManifestError),

    #[error(transparent)]
    Encoding(#[from] crate::tact::encoding::EncodingError),

    #[error(transparent)]
    Indices(#[from] crate::tact::indices::IndexError),

    #[error(transparent)]
    Archive(#[from] crate::tact::archive::ArchiveError),

    #[error(transparent)]
    Download(#[from] crate::tact::download::DownloadError),

    #[error(transparent)]
    Writer(#[from] crate::tact::writer::WriteError),

    #[error(transparent)]
    BuildInfo(#[from] crate::buildinfo::BuildInfoError),

    #[error(transparent)]
    ProductDb(#[from] crate::tact::productdb::ProductDbError),

    #[error(transparent)]
    Staged(#[from] crate::tact::staged::StagedError),

    #[error("the {0} table is empty for this region")]
    EmptyTable(&'static str),

    #[error("Blizzard lists no build to pre-download in this region")]
    NothingStaged,

    #[error("the build config has no {0} manifest")]
    MissingManifest(&'static str),

    /// Downloaded and extracted; only the bookkeeping failed. Its own variant,
    /// so the message says which file in the install root it was.
    #[error("{dir} will not accept new files, so this install cannot be finished")]
    Unwritable {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("the download finished but {what} could not be updated")]
    Metadata {
        what: &'static str,
        #[source]
        source: crate::buildinfo::BuildInfoError,
    },

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io { path: path.to_path_buf(), source }
}
