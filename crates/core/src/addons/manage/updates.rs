use super::fetch::resolve_everywhere;
use super::{ManageError, Options};
use crate::addons::catalog::{Catalog, Request};
use crate::addons::install::{self, Ledger};
use crate::net::Http;

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
    /// What kind of build is installed, as recorded at install time.
    pub installed_build: Option<crate::addons::Channel>,
    /// What kind of build is on offer.
    pub available_build: Option<crate::addons::Channel>,
    /// How the offered build calls itself, a version, not a file id, for
    /// showing to someone.
    pub available_name: Option<String>,
    /// When the offered build was published, as the provider states it.
    pub available_date: Option<String>,
    /// Why there is no answer, if there isn't one.
    pub problem: Option<String>,
}

impl UpdateStatus {
    /// True when the offered build is a different kind from the installed one:
    /// alpha back to stable is a switch, not an update.
    pub fn is_channel_switch(&self) -> bool {
        match (self.installed_build, self.available_build) {
            (Some(installed), Some(available)) => installed != available,
            // One side unknown (an older record): an ordinary update.
            _ => false,
        }
    }

    pub fn has_update(&self) -> bool {
        match (&self.installed, &self.available) {
            (Some(installed), Some(available)) => installed != available,
            // Without both sides it is unknown: a provider being down must not
            // badge every addon outdated.
            _ => false,
        }
    }
}

/// What a bulk update or update check asks for: every tracked addon that has
/// not been ignored.
///
/// Shared by both front ends and both operations. An ignored addon can still
/// be asked for by name.
pub fn update_requests(ledger: &Ledger, default_channel: crate::addons::Channel) -> Vec<Request> {
    requests_for(ledger, default_channel, install::Record::tracks_updates)
}

/// What an unattended pass asks for: every addon that updates itself.
///
/// Not [`update_requests`]: "Update all" covers what is not excluded, this
/// only what was allowed.
pub fn auto_update_requests(
    ledger: &Ledger,
    default_channel: crate::addons::Channel,
    default_auto: bool,
) -> Vec<Request> {
    requests_for(ledger, default_channel, |record| record.updates_itself(default_auto))
}

fn requests_for(
    ledger: &Ledger,
    default_channel: crate::addons::Channel,
    wanted: impl Fn(&install::Record) -> bool,
) -> Vec<Request> {
    ledger
        .addons
        .iter()
        .filter(|record| wanted(record))
        .map(|record| Request {
            provider: record.provider.clone(),
            id: record.id.clone(),
            // Updating means the newest file, never the pinned one.
            version_id: None,
            // `install_all` recomputes it; set so callers see the same.
            channel: Some(record.channel.unwrap_or(default_channel)),
        })
        .collect()
}

/// Ask every provider what the tracked addons are at now, without installing.
pub async fn check_updates(
    http: &Http,
    catalog: Option<&Catalog>,
    options: &Options,
) -> Result<Vec<UpdateStatus>, ManageError> {
    let ledger = Ledger::load(&options.ledger_path)?;
    // No request for an ignored or pinned addon.
    let requests = update_requests(&ledger, options.channel);
    if requests.is_empty() {
        return Ok(Vec::new());
    }

    tracing::info!("addons: checking {} for updates ({})", requests.len(), options.flavor);
    let resolved = resolve_everywhere(http, catalog, &requests, &options.flavor).await?;

    Ok(ledger
        .addons
        .iter()
        .map(|record| {
            // Not asked, so never outdated.
            let found = match record.tracks_updates() {
                true => resolved
                    .iter()
                    .find(|(provider, r)| *provider == record.provider && r.id == record.id)
                    .map(|(_, r)| r),
                false => None,
            };
            UpdateStatus {
                provider: record.provider.clone(),
                id: record.id.clone(),
                name: record.name.clone(),
                installed: record.file_id.clone(),
                available: found.and_then(|r| r.file_id.clone()),
                installed_build: record.build,
                available_build: found.and_then(|r| r.channel),
                available_name: found.and_then(|r| r.file_name.clone()),
                available_date: found.and_then(|r| r.file_date.clone()),
                problem: found.and_then(|r| r.unavailable_reason().or_else(|| r.note.clone())),
            }
        })
        .collect())
}

/// Which builds one request accepts: the addon's own choice, else the default.
pub(super) fn effective_channel(ledger: &Ledger, request: &Request, options: &Options) -> crate::addons::Channel {
    ledger
        .find(&request.provider, &request.id)
        .and_then(|record| record.channel)
        .or(request.channel)
        .unwrap_or(options.channel)
}
