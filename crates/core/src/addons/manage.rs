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
use super::providers;
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

/// An addon that could not be installed, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub name: String,
    pub reason: String,
    /// The project page. For an opt-out this is the *only* way to get the
    /// addon, so a UI should offer to open it rather than just explaining.
    pub website_url: Option<String>,
}

/// What a run achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub installed: Vec<String>,
    pub up_to_date: Vec<String>,
    pub unavailable: Vec<Unavailable>,
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
    catalog: Option<&Catalog>,
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
    let resolved = resolve_everywhere(http, catalog, &requests, &options.flavor).await?;
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
            report.unavailable.push(Unavailable {
                name,
                reason,
                website_url: entry.website_url.clone(),
            });
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
                report.unavailable.push(Unavailable {
                    name,
                    reason,
                    website_url: entry.website_url.clone(),
                });
            }
        }
    }

    ledger.save(&options.ledger_path)?;
    Ok(report)
}

/// Update everything the ledger tracks.
pub async fn update_all(
    http: &Http,
    catalog: Option<&Catalog>,
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

/// Resolve a mixed batch, sending each provider's entries where they belong.
///
/// CurseForge goes through the catalog proxy because it needs a key; the rest
/// are resolved client-side. That split is the point: with no catalog
/// configured, or with the proxy down, every keyless provider still works.
pub async fn resolve_everywhere(
    http: &Http,
    catalog: Option<&Catalog>,
    requests: &[Request],
    flavor: &str,
) -> Result<Vec<Resolved>, ManageError> {
    use std::collections::BTreeMap;

    let mut grouped: BTreeMap<super::Provider, Vec<Request>> = BTreeMap::new();
    for request in requests {
        grouped
            .entry(super::Provider::parse(&request.provider))
            .or_default()
            .push(request.clone());
    }

    let mut resolved = Vec::with_capacity(requests.len());
    for (provider, batch) in grouped {
        match (&provider, catalog) {
            (super::Provider::Curse, Some(catalog)) => {
                resolved.extend(catalog.resolve(http, flavor, &batch).await?)
            }
            (super::Provider::Curse, None) => resolved.extend(providers::unsupported(
                &batch,
                "no addon catalog is configured, so CurseForge addons cannot be resolved",
            )),
            _ => resolved.extend(providers::resolve(http, &provider, &batch, flavor).await),
        }
    }
    Ok(resolved)
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

    verify_md5(&package, entry.md5.as_deref(), name)?;

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

/// Check a package against a published MD5, when there is one.
///
/// Only WoWInterface publishes one today. A mismatch means a truncated or
/// tampered package, and extracting it would overwrite a working addon folder
/// with a broken one — so it fails before anything is written.
fn verify_md5(package: &[u8], expected: Option<&str>, name: &str) -> Result<(), ManageError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = format!("{:x}", md5::compute(package));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(ManageError::Checksum {
            name: name.to_owned(),
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(())
}

/// What an update check found for one tracked addon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub provider: String,
    pub id: String,
    pub name: String,
    /// File id currently installed.
    pub installed: Option<String>,
    /// Newest the provider offers, when it could be asked.
    pub available: Option<String>,
    /// Why there is no answer, if there isn't one.
    pub problem: Option<String>,
}

impl UpdateStatus {
    pub fn has_update(&self) -> bool {
        match (&self.installed, &self.available) {
            (Some(installed), Some(available)) => installed != available,
            // Never installed-with-no-record vs unknown: without both sides
            // this is "unknown", not "update available". Badging every addon
            // as outdated because a provider is down would be worse than
            // saying nothing.
            _ => false,
        }
    }
}

/// Ask every provider what the tracked addons are at now, without installing.
pub async fn check_updates(
    http: &Http,
    catalog: Option<&Catalog>,
    options: &Options,
) -> Result<Vec<UpdateStatus>, ManageError> {
    let ledger = Ledger::load(&options.ledger_path)?;
    let requests: Vec<Request> = ledger
        .addons
        .iter()
        .map(|record| Request {
            provider: record.provider.clone(),
            id: record.id.clone(),
            version_id: None,
        })
        .collect();
    if requests.is_empty() {
        return Ok(Vec::new());
    }

    let resolved = resolve_everywhere(http, catalog, &requests, &options.flavor).await?;

    Ok(ledger
        .addons
        .iter()
        .map(|record| {
            let found = resolved.iter().find(|r| r.id == record.id);
            UpdateStatus {
                provider: record.provider.clone(),
                id: record.id.clone(),
                name: record.name.clone(),
                installed: record.file_id.clone(),
                available: found.and_then(|r| r.file_id.clone()),
                problem: found.and_then(|r| r.unavailable_reason().or_else(|| r.note.clone())),
            }
        })
        .collect())
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

/// The ledger path for one install of a product.
pub fn ledger_path(
    data_dir: &Path,
    addons_dir: &Path,
    code: &crate::product::ProductCode,
) -> PathBuf {
    Ledger::path_for(data_dir, addons_dir, code)
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

    #[error("{name} downloaded as {actual}, expected {expected} — the package is damaged")]
    Checksum { name: String, expected: String, actual: String },

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
    fn a_published_md5_is_enforced_in_both_directions() {
        let package = b"addon bytes";
        let digest = format!("{:x}", md5::compute(package));

        assert!(verify_md5(package, Some(&digest), "BigWigs").is_ok());
        // Case differences are not corruption.
        assert!(verify_md5(package, Some(&digest.to_uppercase()), "BigWigs").is_ok());
        // A provider that publishes no digest is not an error.
        assert!(verify_md5(package, None, "ElvUI").is_ok());

        let err = verify_md5(b"tampered", Some(&digest), "BigWigs").unwrap_err();
        assert!(matches!(err, ManageError::Checksum { .. }), "{err}");
    }

    #[test]
    fn an_unknown_remote_version_is_not_an_update() {
        let status = |installed: Option<&str>, available: Option<&str>| UpdateStatus {
            provider: "curse".into(),
            id: "1".into(),
            name: "Thing".into(),
            installed: installed.map(str::to_owned),
            available: available.map(str::to_owned),
            problem: None,
        };

        assert!(status(Some("100"), Some("101")).has_update());
        assert!(!status(Some("100"), Some("100")).has_update());
        // Provider down, or an addon with no recorded version: unknown, and
        // unknown must not render as "update available".
        assert!(!status(Some("100"), None).has_update());
        assert!(!status(None, Some("101")).has_update());
    }

    #[test]
    fn a_report_is_complete_only_when_nothing_was_left_out() {
        let mut report = Report {
            installed: vec!["BigWigs".into()],
            ..Report::default()
        };
        assert!(report.is_complete());

        report.unavailable.push(Unavailable {
            name: "EXCore".into(),
            reason: "opted out".into(),
            website_url: Some("https://curseforge.com/x".into()),
        });
        assert!(!report.is_complete());
    }
}
