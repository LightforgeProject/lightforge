use super::updates::{effective_channel, update_requests};
use super::{io, Event, ManageError, Options, Report, Unavailable};
use crate::addons::catalog::{Catalog, Request, Resolved};
use crate::addons::install::{self, Ledger, Record};
use crate::addons::providers;
use crate::backup::Reason;
use crate::net::Http;

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
    // Read once to decide. The save reads it again: a pin set meanwhile survives.
    let mut ledger = Ledger::load(&options.ledger_path)?;
    let mut written: Vec<(Record, Option<String>)> = Vec::new();
    let mut report = Report::default();

    // Pins only when reproducing an export; otherwise the newest on the channel.
    let requests: Vec<Request> = requests
        .iter()
        .map(|r| Request {
            version_id: options.pinned.then(|| r.version_id.clone()).flatten(),
            channel: Some(effective_channel(&ledger, r, options)),
            ..r.clone()
        })
        .collect();

    on_event(Event::Resolving { count: requests.len() });
    tracing::info!(
        "addons: resolving {} for {} (reinstall {}, pinned {})",
        requests.len(),
        options.flavor,
        options.reinstall,
        options.pinned
    );
    let resolved = resolve_everywhere(http, catalog, &requests, &options.flavor).await?;
    let unavailable = resolved.iter().filter(|(_, r)| r.downloadable().is_none()).count();
    on_event(Event::Resolved {
        ok: resolved.len() - unavailable,
        unavailable,
    });

    std::fs::create_dir_all(&options.addons_dir).map_err(io(&options.addons_dir))?;

    let mut backup = options.backup.clone();
    let mut failed = None;
    for (provider, entry) in &resolved {
        let asked = requests.iter().find(|r| r.provider == *provider && r.id == entry.id);
        let provider = provider.clone();
        // A request naming a file reproduces an exact setup: it becomes a pin.
        let pin = asked.and_then(|r| r.version_id.clone());
        let name = entry.name.clone().unwrap_or_else(|| entry.id.clone());

        if let Some(reason) = entry.unavailable_reason() {
            tracing::warn!("addons: {name} ({provider}/{}) is unavailable: {reason}", entry.id);
            on_event(Event::Unavailable { name: name.clone(), reason: reason.clone() });
            report.unavailable.push(Unavailable {
                name,
                reason,
                website_url: entry.website_url.clone(),
                github: entry.github_repo(),
            });
            continue;
        }
        // A repair reinstalls even the right file.
        if !options.reinstall && !ledger.needs_update(entry, &provider) {
            on_event(Event::UpToDate { name: name.clone() });
            report.up_to_date.push(name);
            continue;
        }

        // Once per run, before something installed changes. No backup, no change.
        if ledger.find(&provider, &entry.id).is_some() {
            if let Some(plan) = backup.take() {
                let made = tokio::task::spawn_blocking(move || plan.create(Reason::AddonUpdate))
                    .await
                    .map_err(|e| ManageError::Join(e.to_string()))
                    .and_then(|made| made.map_err(ManageError::from));
                // Stop here, still saving what was installed so far.
                if let Err(e) = made {
                    tracing::error!("addons: the settings backup before updating failed, stopping: {}", crate::error::chain(&e));
                    failed = Some(e);
                    break;
                }
            }
        }

        match fetch_and_install(http, entry, &provider, &name, pin.clone(), options, &mut ledger).await {
            Ok(folders) => {
                if let Some(record) = ledger.find(&provider, &entry.id) {
                    written.push((record.clone(), pin));
                }
                tracing::info!("addons: installed {name} ({provider}/{}): {folders:?}", entry.id);
                on_event(Event::Installed { name: name.clone(), folders });
                report.installed.push(name);
            }
            Err(e) => {
                let reason = e.to_string();
                tracing::error!(
                    "addons: could not install {name} ({provider}/{}): {}",
                    entry.id,
                    crate::error::chain(&e)
                );
                on_event(Event::Unavailable { name: name.clone(), reason: reason.clone() });
                report.unavailable.push(Unavailable {
                    name,
                    reason,
                    website_url: entry.website_url.clone(),
                    github: None,
                });
            }
        }
    }

    tracing::info!(
        "addons: {} installed, {} up to date, {} unavailable",
        report.installed.len(),
        report.up_to_date.len(),
        report.unavailable.len()
    );
    save_merged(&options.ledger_path, written, options.pinned)?;
    match failed {
        Some(e) => Err(e),
        None => Ok(report),
    }
}

/// Write what a pass installed into the ledger as it is now. The user's
/// choices (channel, ignore, auto-update, pin) come from the current row.
pub(super) fn save_merged(path: &std::path::Path, written: Vec<(Record, Option<String>)>, importing: bool) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(path)?;
    for (mut record, asked) in written {
        if let Some(current) = ledger.find(&record.provider, &record.id) {
            record.channel = current.channel;
            record.ignored = current.ignored;
            record.auto_update = current.auto_update;
            record.pinned =
                pin_after_install(asked, importing, current.pinned.clone(), record.file_id.as_deref());
        }
        ledger.record(record);
    }
    ledger.save(path)?;
    Ok(())
}

/// Update everything the ledger tracks.
pub async fn update_all(
    http: &Http,
    catalog: Option<&Catalog>,
    options: &Options,
    on_event: impl FnMut(Event),
) -> Result<Report, ManageError> {
    let ledger = Ledger::load(&options.ledger_path)?;
    let requests = update_requests(&ledger, options.channel);

    let options = Options { pinned: false, reinstall: false, ..options.clone() };
    install_all(http, catalog, &requests, &options, on_event).await
}

/// Resolve a mixed batch: CurseForge through the proxy (it needs a key), the
/// rest client-side, so they work without one. Each result carries its
/// request's provider: ids are only unique per provider.
pub async fn resolve_everywhere(
    http: &Http,
    catalog: Option<&Catalog>,
    requests: &[Request],
    flavor: &str,
) -> Result<Vec<(String, Resolved)>, ManageError> {
    use std::collections::BTreeMap;

    let mut grouped: BTreeMap<crate::addons::Provider, Vec<Request>> = BTreeMap::new();
    for request in requests {
        grouped
            .entry(crate::addons::Provider::parse(&request.provider))
            .or_default()
            .push(request.clone());
    }

    let mut resolved = Vec::with_capacity(requests.len());
    for (provider, batch) in grouped {
        let answers = match (&provider, catalog) {
            (crate::addons::Provider::Curse, Some(catalog)) => catalog.resolve(http, flavor, &batch).await?,
            (crate::addons::Provider::Curse, None) => providers::unsupported(
                &batch,
                "no addon catalog is configured, so CurseForge addons cannot be resolved",
            ),
            _ => providers::resolve(http, &provider, &batch, flavor).await,
        };
        // As the request spelled it: the ledger's key.
        let named = batch.first().map(|r| r.provider.clone()).unwrap_or_default();
        resolved.extend(answers.into_iter().map(|answer| (named.clone(), answer)));
    }
    Ok(resolved)
}

/// Fetch one package and write it, updating the ledger entry.
async fn fetch_and_install(
    http: &Http,
    entry: &Resolved,
    provider: &str,
    name: &str,
    pin: Option<String>,
    options: &Options,
    ledger: &mut Ledger,
) -> Result<Vec<String>, ManageError> {
    let url = entry.downloadable().ok_or(ManageError::NoDownload)?;
    let package = http.get_bytes(url, install::MAX_PACKAGE_BYTES).await?;

    verify_md5(&package, entry.md5.as_deref(), name)?;

    // Folders an older build shipped and this one does not go too (unless
    // another record claims them), inside the extract's swap.
    let retire: Vec<String> = ledger
        .find(provider, &entry.id)
        .map(|r| r.folders.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|folder| {
            !ledger
                .addons
                .iter()
                .any(|r| !(r.provider == provider && r.id == entry.id) && r.folders.contains(folder))
        })
        .collect();

    // The package's folders, not the catalog's: removal deletes these.
    let extracted = install::extract(&package, &options.addons_dir, &retire)?;
    // The user's choices carry over from the old record.
    let chosen = ledger.find(provider, &entry.id).cloned().unwrap_or_default();
    ledger.record(Record {
        provider: provider.to_owned(),
        id: entry.id.clone(),
        name: name.to_owned(),
        file_id: entry.file_id.clone(),
        folders: extracted.folders.clone(),
        channel: chosen.channel,
        build: entry.channel,
        ignored: chosen.ignored,
        auto_update: chosen.auto_update,
        pinned: pin_after_install(pin, options.pinned, chosen.pinned, entry.file_id.as_deref()),
        // When this build arrived.
        installed_at: Some(install::today()),
        // From this reply: a project can move. An older proxy sends none.
        website_url: entry.website_url.clone(),
        author_url: entry.author_url.clone(),
        source_url: entry.source_url.clone(),
    });
    Ok(extracted.folders)
}

/// What an addon is pinned to once a file has been installed over it.
///
/// `asked` pins only when `importing`; otherwise the old pin carries over.
/// Either way only while it names the file now on disk.
pub(super) fn pin_after_install(
    asked: Option<String>,
    importing: bool,
    held: Option<String>,
    installed: Option<&str>,
) -> Option<String> {
    asked
        .filter(|_| importing)
        .or(held)
        .filter(|pin| Some(pin.as_str()) == installed)
}

/// Check a package against a published MD5, when there is one.
///
/// Only WoWInterface publishes one. A mismatch fails before anything is written.
pub(super) fn verify_md5(package: &[u8], expected: Option<&str>, name: &str) -> Result<(), ManageError> {
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
