//! Install, update and remove addons end to end.
//!
//! The half that needs the network: resolve through the catalog, fetch each
//! package from the CDN, extract it, and record what was written. Reports
//! through an [`Event`] callback like the other orchestrators, so both
//! front-ends drive the same code.
//!
//! One addon failing never stops the rest. A shared list of twenty will
//! routinely contain one project whose author has opted out of third-party
//! downloads and one that no longer exists, and the right outcome is eighteen
//! installed addons plus two clear explanations.

use std::path::{Path, PathBuf};

use super::catalog::{Catalog, CatalogError, Request, Resolved};
use super::install::{self, InstallError, Ledger, Record};
use crate::net::Http;

/// Where an operation writes, and what it is for.
#[derive(Debug, Clone)]
pub struct Options {
    /// Catalog flavor: `retail`, `classic`, `classic_era`.
    pub flavor: String,
    /// The product's `Interface/AddOns` directory.
    pub addons_dir: PathBuf,
    /// Where this product's ledger lives.
    pub ledger_path: PathBuf,
    /// Honour the `version_id` an import token pins, instead of taking the
    /// newest file. Reproduces someone's exact setup rather than tracking them.
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub enum Event {
    Resolving { count: usize },
    Resolved { ok: usize, unavailable: usize },
    Downloading { name: String },
    Installed { name: String, folders: Vec<String> },
    /// Already at this version; nothing fetched.
    UpToDate { name: String },
    /// Could not be installed, and why — opt-out, missing, or a failed fetch.
    Unavailable { name: String, reason: String },
}

/// What a run achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub installed: Vec<String>,
    pub up_to_date: Vec<String>,
    /// Name and reason, for anything that could not be installed.
    pub unavailable: Vec<(String, String)>,
}

impl Report {
    /// True when every addon asked for is now present at the wanted version.
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_empty()
    }
}

/// Install or update a set of addons.
///
/// `requests` is what to fetch; the ledger decides what can be skipped.
pub async fn install_all(
    http: &Http,
    catalog: &Catalog,
    requests: &[Request],
    options: &Options,
    mut on_event: impl FnMut(Event),
) -> Result<Report, ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    let mut report = Report::default();

    // Pins only make sense when reproducing an export; otherwise ask for the
    // newest file the flavor has.
    let requests: Vec<Request> = requests
        .iter()
        .map(|r| Request {
            version_id: options.pinned.then(|| r.version_id.clone()).flatten(),
            ..r.clone()
        })
        .collect();

    on_event(Event::Resolving { count: requests.len() });
    let resolved = catalog.resolve(http, &options.flavor, &requests).await?;
    let unavailable = resolved.iter().filter(|r| r.downloadable().is_none()).count();
    on_event(Event::Resolved {
        ok: resolved.len() - unavailable,
        unavailable,
    });

    std::fs::create_dir_all(&options.addons_dir).map_err(|source| ManageError::Io {
        path: options.addons_dir.clone(),
        source,
    })?;

    for entry in &resolved {
        let provider = requests
            .iter()
            .find(|r| r.id == entry.id)
            .map(|r| r.provider.clone())
            .unwrap_or_else(|| "curse".to_owned());
        let name = entry.name.clone().unwrap_or_else(|| entry.id.clone());

        if let Some(reason) = entry.unavailable_reason() {
            on_event(Event::Unavailable { name: name.clone(), reason: reason.clone() });
            report.unavailable.push((name, reason));
            continue;
        }
        if !ledger.needs_update(entry, &provider) {
            on_event(Event::UpToDate { name: name.clone() });
            report.up_to_date.push(name);
            continue;
        }

        match fetch_and_install(http, entry, &provider, &name, options, &mut ledger).await {
            Ok(folders) => {
                on_event(Event::Installed { name: name.clone(), folders });
                report.installed.push(name);
            }
            Err(e) => {
                let reason = e.to_string();
                on_event(Event::Unavailable { name: name.clone(), reason: reason.clone() });
                report.unavailable.push((name, reason));
            }
        }
    }

    ledger.save(&options.ledger_path)?;
    Ok(report)
}

/// Update everything the ledger tracks.
pub async fn update_all(
    http: &Http,
    catalog: &Catalog,
    options: &Options,
    on_event: impl FnMut(Event),
) -> Result<Report, ManageError> {
    let ledger = Ledger::load(&options.ledger_path)?;
    let requests: Vec<Request> = ledger
        .addons
        .iter()
        .map(|record| Request {
            provider: record.provider.clone(),
            id: record.id.clone(),
            // Updating means the newest file, never the pinned one.
            version_id: None,
        })
        .collect();

    let options = Options { pinned: false, ..options.clone() };
    install_all(http, catalog, &requests, &options, on_event).await
}

/// Fetch one package and write it, updating the ledger entry.
async fn fetch_and_install(
    http: &Http,
    entry: &Resolved,
    provider: &str,
    name: &str,
    options: &Options,
    ledger: &mut Ledger,
) -> Result<Vec<String>, ManageError> {
    let url = entry.downloadable().ok_or(ManageError::NoDownload)?;
    let package = http.get_bytes(url).await?;

    // Trust what the package contains over what the catalog said it contains:
    // the folders are what removal will delete, so they have to be real.
    let extracted = install::extract(&package, &options.addons_dir)?;
    ledger.record(Record {
        provider: provider.to_owned(),
        id: entry.id.clone(),
        name: name.to_owned(),
        file_id: entry.file_id.clone(),
        folders: extracted.folders.clone(),
    });
    Ok(extracted.folders)
}

/// Remove an addon Lightforge installed.
pub fn remove(options: &Options, provider: &str, id: &str) -> Result<Vec<String>, ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    let removed = install::remove(&mut ledger, &options.addons_dir, provider, id)?;
    ledger.save(&options.ledger_path)?;
    Ok(removed)
}

/// The requests an import token implies.
pub fn requests_from(collection: &super::Collection) -> Vec<Request> {
    collection
        .addons
        .iter()
        .filter(|entry| entry.provider().is_supported())
        .map(Request::from)
        .collect()
}

/// Entries of a token Lightforge cannot resolve at all, for reporting.
pub fn unsupported(collection: &super::Collection) -> Vec<&super::Entry> {
    collection
        .addons
        .iter()
        .filter(|entry| !entry.provider().is_supported())
        .collect()
}

/// The ledger path for a product under Lightforge's data directory.
pub fn ledger_path(data_dir: &Path, code: &crate::product::ProductCode) -> PathBuf {
    Ledger::path_for(data_dir, code)
}

#[derive(Debug, thiserror::Error)]
pub enum ManageError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),

    #[error(transparent)]
    Install(#[from] InstallError),

    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error("the catalog reported this addon as available but gave no download URL")]
    NoDownload,

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
    use crate::addons::Collection;

    const TOKEN: &str = concat!(
        "eyJjb2xsZWN0aW9uX25hbWUiOiJXb3dVcF9leHBvcnRfMSIsImNsaWVudF90eXBlIjoiUmV0YWlsIiwi",
        "YWRkb25zIjpbeyJpZCI6IjIzODIiLCJuYW1lIjoiQmlnV2lncyIsInByb3ZpZGVyX25hbWUiOiJDdXJz",
        "ZSIsInZlcnNpb25faWQiOiI4ODY2NTA4In0seyJpZCI6IjEzMzQ0IiwibmFtZSI6IkJ1Z0dyYWJiZXIi",
        "LCJwcm92aWRlcl9uYW1lIjoiQ3Vyc2UiLCJ2ZXJzaW9uX2lkIjoiODkwNzU4NyJ9XX0"
    );

    #[test]
    fn a_token_becomes_requests_that_keep_the_pin() {
        let collection = Collection::parse(TOKEN).unwrap();
        let requests = requests_from(&collection);

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].provider, "curse");
        assert_eq!(requests[0].version_id.as_deref(), Some("8866508"));
        assert!(unsupported(&collection).is_empty());
    }

    #[test]
    fn an_unsupported_provider_is_reported_rather_than_requested() {
        let json = br#"{"collection_name":"x","client_type":"Retail","addons":[
            {"id":"1","name":"Known","provider_name":"Curse"},
            {"id":"2","name":"Odd","provider_name":"SomethingElse"}]}"#;
        let collection: Collection = serde_json::from_slice(json).unwrap();

        assert_eq!(requests_from(&collection).len(), 1);
        let skipped = unsupported(&collection);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "Odd");
    }

    #[test]
    fn a_report_is_complete_only_when_nothing_was_left_out() {
        let mut report = Report {
            installed: vec!["BigWigs".into()],
            ..Report::default()
        };
        assert!(report.is_complete());

        report.unavailable.push(("EXCore".into(), "opted out".into()));
        assert!(!report.is_complete());
    }
}
