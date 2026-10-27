//! Driving a whole install: version service -> plan -> download -> store.
//!
//! This is the orchestration both front-ends need, so it lives here rather than
//! in either of them. It reports through an [`Event`] callback instead of
//! printing: the CLI turns events into lines, and the GUI turns them into
//! messages, without either owning the sequence.
//!
//! The ordering is not arbitrary and is the part worth preserving:
//!
//! 1. Archive indices are cached first. Nothing can be *located* without them.
//! 2. Whatever the store already holds is subtracted from the plan, which is
//!    what makes an interrupted download resume rather than start over.
//! 3. `.build.info` is written **last, and only on a genuinely complete run**.
//!    [`crate::state::Local`] treats its presence as proof that a download
//!    finished, so writing it after a partial run would offer PLAY on a broken
//!    game. Every way a run can fall short is accounted for in
//!    [`Summary::complete`], failures, cancellation, unlocatable keys, an
//!    incomplete install-file extraction, and the limit options, which
//!    truncate the plan deliberately.

mod execute;
mod locale;
mod prepare;
#[cfg(test)]
mod tests;

pub use execute::{execute, run};
pub use locale::{current_locale, locale_name, LOCALES};
pub use prepare::{offerings, prepare, Offerings, Prepared, StoreProbe};

use std::path::PathBuf;

use crate::tact::download::{self, Progress};
use crate::tact::indices::Step;
use crate::tact::version::Region;
use crate::tact::writer::Summary as StoreSummary;
// The tests reach the locale rules directly.
#[cfg(test)]
use locale::*;

/// How a run is tuned. The limits exist for development slices and must never
/// be set by a real install, each of them makes the result deliberately
/// partial, which is why [`Summary::complete`] refuses `.build.info` when any
/// is in play.
#[derive(Debug, Clone)]
pub struct Options {
    pub region: Region,
    /// Client locale, e.g. `enUS`. Selects which of a build's files are wanted.
    pub locale: String,
    /// Optional content to opt in to, by tag name, e.g. `HighRes`.
    ///
    /// Empty matches Battle.net, which installs optional content only when
    /// asked. See [`crate::tact::manifest::TagSet::with_optional`].
    pub optional_tags: Vec<String>,
    /// Flavor directory the install files are extracted into, e.g. `_retail_`.
    pub flavor: String,
    pub concurrency: usize,
    /// Cap on download speed in bytes per second; `0` is no cap.
    ///
    /// Bytes rather than a friendlier unit because this is the layer that
    /// spends them, the UI converts from whatever it shows the user.
    pub limit_bytes_per_second: u64,
    /// Ranges this far apart are merged into one request.
    pub gap_bytes: u32,
    /// Upper bound on the checkpoint interval; scaled down for small products.
    pub checkpoint_bytes: u64,
    /// Extract the install manifest's files (the game executable and friends).
    pub install_files: bool,
    /// Stop after this many archive indices, development only.
    pub limit_archives: Option<usize>,
    /// Truncate the plan to this many coalesced runs, development only.
    pub limit_runs: Option<usize>,
    /// Skip the loose (unarchived) files, development only.
    pub skip_loose: bool,
    /// The install's path as Windows sees it, e.g.
    /// `C:/Program Files (x86)/World of Warcraft`. `None` skips writing
    /// `.product.db`, which only matters for Battle.net adoption.
    pub windows_path: Option<String>,
    /// Agent's own directory, whose `product.db` decides whether Battle.net
    /// shows `Play` or `Install` for a product Lightforge downloaded.
    ///
    /// `None` skips the registration. There is nothing to merge into on a
    /// machine with no Battle.net, and a CLI run against a bare directory has
    /// no prefix to speak of.
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

    /// True when any option deliberately truncates the plan.
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
    /// The install root refused a file and its owner-write bit was added so
    /// the run could proceed. Reported because it is a change to a directory
    /// the user owns, made without being asked.
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
    /// Transfer progress. Called once per stored file, so a UI consumer must
    /// coalesce rather than redraw on each one.
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
    /// Present when `complete`; the version `.build.info` now names.
    pub version: Option<String>,
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

    #[error("the {0} table is empty for this region")]
    EmptyTable(&'static str),

    #[error("the build config has no {0} manifest")]
    MissingManifest(&'static str),

    /// The content is downloaded and extracted and only the bookkeeping is
    /// left. Its own variant because "i/o error at <the install root>" says
    /// nothing about which of the half-dozen things that touch that directory
    /// was being written, and this is the one the user has waited longest for.
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
