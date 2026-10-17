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
use crate::backup::{BackupError, Reason};
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
    /// Install even when the ledger says the addon is already on this file,
    /// replacing the folders it recorded rather than writing over them.
    ///
    /// What "re-install" means: the ledger can only say which *file* is
    /// installed, never whether the folders it wrote are still intact, so
    /// repairing one has to be something the user can ask for directly.
    pub reinstall: bool,
    /// Which builds to accept for an addon that has no opinion of its own.
    pub channel: super::Channel,
    /// Back up the game's settings before the first installed addon is
    /// replaced. `None` for callers with no game to protect, or none to ask.
    pub backup: Option<crate::backup::Plan>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Resolving { count: usize },
    Resolved { ok: usize, unavailable: usize },
    Downloading { name: String },
    Installed { name: String, folders: Vec<String> },
    /// Already at this version; nothing fetched.
    UpToDate { name: String },
    /// Could not be installed, and why, opt-out, missing, or a failed fetch.
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
    // Read once to decide what to do. The save at the end reads it again:
    // a pass takes minutes, and a pin or an ignore set meanwhile must not be
    // undone by writing this copy back over it.
    let mut ledger = Ledger::load(&options.ledger_path)?;
    let mut written: Vec<(Record, Option<String>)> = Vec::new();
    let mut report = Report::default();

    // Pins only make sense when reproducing an export; otherwise ask for the
    // newest file the flavor has, on whichever channel this addon accepts.
    let requests: Vec<Request> = requests
        .iter()
        .map(|r| Request {
            version_id: options.pinned.then(|| r.version_id.clone()).flatten(),
            channel: Some(effective_channel(&ledger, r, options)),
            ..r.clone()
        })
        .collect();

    on_event(Event::Resolving { count: requests.len() });
    let resolved = resolve_everywhere(http, catalog, &requests, &options.flavor).await?;
    let unavailable = resolved.iter().filter(|(_, r)| r.downloadable().is_none()).count();
    on_event(Event::Resolved {
        ok: resolved.len() - unavailable,
        unavailable,
    });

    std::fs::create_dir_all(&options.addons_dir).map_err(|source| ManageError::Io {
        path: options.addons_dir.clone(),
        source,
    })?;

    let mut backup = options.backup.clone();
    let mut failed = None;
    for (provider, entry) in &resolved {
        let asked = requests.iter().find(|r| r.provider == *provider && r.id == entry.id);
        let provider = provider.clone();
        // A request naming a file is someone reproducing an exact setup, and
        // that is a pin: it has to survive the install, or the next pass
        // moves the addon straight off the version they asked for.
        let pin = asked.and_then(|r| r.version_id.clone());
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
        // Being on the right file is the whole question for an update and
        // beside the point for a repair.
        if !options.reinstall && !ledger.needs_update(entry, &provider) {
            on_event(Event::UpToDate { name: name.clone() });
            report.up_to_date.push(name);
            continue;
        }

        // Once per run, and only when something already installed is about
        // to change: a fresh install has no saved data to migrate. Without a
        // backup nothing is replaced, which is the point of taking one.
        if ledger.find(&provider, &entry.id).is_some() {
            if let Some(plan) = backup.take() {
                let made = tokio::task::spawn_blocking(move || plan.create(Reason::AddonUpdate))
                    .await
                    .map_err(|e| ManageError::Join(e.to_string()))
                    .and_then(|made| made.map_err(ManageError::from));
                // What was installed before this point is on disk and has to
                // be recorded, so the pass stops here rather than returning
                // past the save below.
                if let Err(e) = made {
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

    save_merged(&options.ledger_path, written, options.pinned)?;
    match failed {
        Some(e) => Err(e),
        None => Ok(report),
    }
}

/// Write what a pass installed into the ledger as it is *now*, not as it was
/// when the pass started.
///
/// Each record this pass wrote replaces its row, but the choices that are the
/// user's, channel, ignore, auto-update and pin, are taken from the current
/// row when there is one: they may have changed while the pass ran.
fn save_merged(path: &std::path::Path, written: Vec<(Record, Option<String>)>, importing: bool) -> Result<(), ManageError> {
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

/// Resolve a mixed batch, sending each provider's entries where they belong.
///
/// CurseForge goes through the catalog proxy because it needs a key; the rest
/// are resolved client-side. That split is the point: with no catalog
/// configured, or with the proxy down, every keyless provider still works.
///
/// Each result comes back with the provider of the request it answers: ids
/// are only unique within a provider, and CurseForge and WoWInterface both
/// number theirs, so an id alone can match the wrong request.
pub async fn resolve_everywhere(
    http: &Http,
    catalog: Option<&Catalog>,
    requests: &[Request],
    flavor: &str,
) -> Result<Vec<(String, Resolved)>, ManageError> {
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
        let answers = match (&provider, catalog) {
            (super::Provider::Curse, Some(catalog)) => catalog.resolve(http, flavor, &batch).await?,
            (super::Provider::Curse, None) => providers::unsupported(
                &batch,
                "no addon catalog is configured, so CurseForge addons cannot be resolved",
            ),
            _ => providers::resolve(http, &provider, &batch, flavor).await,
        };
        // The provider exactly as the request spelled it, which is what the
        // ledger keys its records on.
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
    let package = http.get_bytes(url).await?;

    verify_md5(&package, entry.md5.as_deref(), name)?;

    // An update or a repair replaces every folder the record names, so one an
    // older build shipped and this one does not goes too, rather than staying
    // loaded in game with nothing left that owns it. Not a folder another
    // record also claims. It happens inside the extract's swap, so a failure
    // leaves the addon as it was rather than half gone.
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

    // Trust what the package contains over what the catalog said it contains:
    // the folders are what removal will delete, so they have to be real.
    let extracted = install::extract(&package, &options.addons_dir, &retire)?;
    // An install replaces the record wholesale, so everything the user chose
    // about this addon is read off the old one first and carried across.
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
        // Stamped now, not carried over: the question it answers is when
        // *this* build arrived, and a new build arriving is this line.
        installed_at: Some(install::today()),
        // Taken from the reply that was just acted on rather than carried
        // across: a project can move its repository, and the record should
        // say where it is now. An older proxy answers none of these, which
        // costs the menu entries and nothing else.
        website_url: entry.website_url.clone(),
        author_url: entry.author_url.clone(),
        source_url: entry.source_url.clone(),
    });
    Ok(extracted.folders)
}

/// What an addon is pinned to once a file has been installed over it.
///
/// `asked` is the file this request named, which only counts as a pin when
/// the run is reproducing an export (`importing`). Otherwise whatever pin was
/// already held carries over, but either way only while it still describes
/// what is on disk: an explicit install of a different file has moved the
/// addon off its pin, and a pin naming a file that is not there would be a
/// lie the next pass would act on.
fn pin_after_install(
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
/// Only WoWInterface publishes one today. A mismatch means a truncated or
/// tampered package, and extracting it would overwrite a working addon folder
/// with a broken one, so it fails before anything is written.
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
    /// What kind of build is installed, as recorded at install time.
    pub installed_build: Option<super::Channel>,
    /// What kind of build is on offer.
    pub available_build: Option<super::Channel>,
    /// How the offered build calls itself, a version, not a file id, for
    /// showing to someone.
    pub available_name: Option<String>,
    /// When the offered build was published, as the provider states it.
    pub available_date: Option<String>,
    /// Why there is no answer, if there isn't one.
    pub problem: Option<String>,
}

impl UpdateStatus {
    /// True when the offered build is a different *kind* from the installed
    /// one, moving between channels rather than forward on one.
    ///
    /// Calling that an update is how "Update" ends up installing an older
    /// version: switching back to stable from an alpha is a different file,
    /// which is all `has_update` can see.
    pub fn is_channel_switch(&self) -> bool {
        match (self.installed_build, self.available_build) {
            (Some(installed), Some(available)) => installed != available,
            // One side unknown: an addon installed before builds were
            // recorded. Treat it as an ordinary update rather than guessing.
            _ => false,
        }
    }

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

/// What a bulk update or update check asks for: every tracked addon that has
/// not been ignored.
///
/// Both front-ends and both operations share this, because the set is a rule
/// rather than a detail, an ignored addon that one of them still asked about
/// would be ignored everywhere except the one place it mattered.
///
/// An ignored addon can still be asked for by name. `install_all` takes
/// explicit requests, and an explicit request is the user asking.
pub fn update_requests(ledger: &Ledger, default_channel: super::Channel) -> Vec<Request> {
    requests_for(ledger, default_channel, install::Record::tracks_updates)
}

/// What an unattended pass asks for: every addon that updates itself.
///
/// Separate from [`update_requests`] because the two answer different
/// questions. Pressing "Update all" is the user updating everything they have
/// not excluded; this is Lightforge updating what it was told it may.
pub fn auto_update_requests(
    ledger: &Ledger,
    default_channel: super::Channel,
    default_auto: bool,
) -> Vec<Request> {
    requests_for(ledger, default_channel, |record| record.updates_itself(default_auto))
}

fn requests_for(
    ledger: &Ledger,
    default_channel: super::Channel,
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
            // install_all recomputes this from the ledger; set here too so a
            // caller reading these requests sees what will be asked for.
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
    // No request for an addon held still either, ignored or pinned: there is
    // nothing to be done with the answer, and every round-trip costs
    // someone's bandwidth.
    let requests = update_requests(&ledger, options.channel);
    if requests.is_empty() {
        return Ok(Vec::new());
    }

    let resolved = resolve_everywhere(http, catalog, &requests, &options.flavor).await?;

    Ok(ledger
        .addons
        .iter()
        .map(|record| {
            // Nothing was asked for an addon that does not track updates, so
            // nothing is offered: the row still exists, it just never reads
            // as outdated.
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

/// Which builds one request accepts: the addon's own choice, else the default.
fn effective_channel(ledger: &Ledger, request: &Request, options: &Options) -> super::Channel {
    ledger
        .find(&request.provider, &request.id)
        .and_then(|record| record.channel)
        .or(request.channel)
        .unwrap_or(options.channel)
}

/// Set or clear one addon's channel override.
///
/// Clearing it (`None`) returns the addon to following the global default,
/// which is not the same as setting it to stable.
pub fn set_channel(
    options: &Options,
    provider: &str,
    id: &str,
    channel: Option<super::Channel>,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.channel = channel;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}

/// Leave one addon out of everything that updates in bulk, or stop doing so.
///
/// Deliberately not a toggle: the caller knows which state it is showing, and
/// a toggle would flip the wrong way against a ledger changed under it.
pub fn set_ignored(
    options: &Options,
    provider: &str,
    id: &str,
    ignored: bool,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.ignored = ignored;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}

/// One addon that was recognised and is now managed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adopted {
    pub name: String,
    pub folders: Vec<String>,
}

/// Recognise hand-installed addons and take them into the ledger.
///
/// An addon Lightforge did not install has no provider and no id, so it can
/// be listed and nothing else, no update, no channel, no removal. Hashing
/// each unmanaged folder the way CurseForge does (see
/// [`crate::addons::fingerprint`]) and asking the catalog whose it is turns
/// it into an ordinary managed addon.
///
/// Conservative in both directions. A folder already owned by a record is
/// left alone, so this cannot take an addon away from the entry that
/// installed it. And a match is only written for folders that are *actually
/// present*: the catalog answers with the whole package, and recording a
/// folder that is not there would have removal promise to delete it.
pub async fn adopt(
    http: &Http,
    catalog: Option<&Catalog>,
    options: &Options,
) -> Result<Vec<Adopted>, ManageError> {
    use crate::addons::fingerprint;

    let Some(catalog) = catalog else {
        return Ok(Vec::new());
    };
    let mut ledger = Ledger::load(&options.ledger_path)?;

    // Folders a record already claims. Not just the ones on disk: a record
    // whose folders are missing is still that addon's, and adopting over it
    // would leave two records fighting for one name.
    let mut claimed: std::collections::BTreeSet<String> = ledger
        .addons
        .iter()
        .flat_map(|record| record.folders.iter().cloned())
        .collect();

    let present: Vec<String> = super::scan(&options.addons_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|addon| addon.folder)
        .collect();

    // Fingerprint to folder, so the catalog's answer, which is keyed by the
    // hashes we sent, can be read back as "this folder is that addon".
    let mut by_print: std::collections::BTreeMap<u32, String> = Default::default();
    for folder in present.iter().filter(|f| !claimed.contains(*f)) {
        if let Some(print) = fingerprint::of_folder(&options.addons_dir.join(folder)) {
            by_print.insert(print, folder.clone());
        }
    }
    if by_print.is_empty() {
        return Ok(Vec::new());
    }

    let prints: Vec<u32> = by_print.keys().copied().collect();
    let matches = catalog.match_fingerprints(http, &prints).await?;

    let on_disk: std::collections::BTreeSet<&str> =
        present.iter().map(String::as_str).collect();

    let by_project = group_by_project(&matches, &by_print);
    let mut adopted = Vec::new();
    for (id, (found, matched)) in by_project {
        // The package's folders, limited to what is here and not already
        // somebody else's, plus every folder that actually matched, which
        // is the stronger evidence of the two.
        let mut folders: Vec<String> = found
            .folders
            .iter()
            .filter(|f| on_disk.contains(f.as_str()) && !claimed.contains(*f))
            .cloned()
            .chain(matched)
            .filter(|f| !claimed.contains(f))
            .collect();
        folders.sort();
        folders.dedup();
        if folders.is_empty() {
            continue;
        }
        // Taken now, so a second project in the same answer cannot claim it.
        claimed.extend(folders.iter().cloned());

        // Already managed under this id: a folder an older build left behind,
        // most likely. It joins that record, which keeps everything the user
        // chose about the addon; a fresh record would have replaced it.
        if let Some(existing) = ledger.find_mut("curse", &id) {
            existing.folders.extend(folders.iter().cloned());
            existing.folders.sort();
            existing.folders.dedup();
            adopted.push(Adopted { name: existing.name.clone(), folders });
            continue;
        }

        let name = found
            .name
            .clone()
            .or_else(|| folders.first().cloned())
            .unwrap_or_else(|| id.clone());
        ledger.record(Record {
            provider: "curse".to_owned(),
            id,
            name: name.clone(),
            file_id: found.file_id.clone(),
            folders: folders.clone(),
            // Adopted, not installed: the build on disk is whatever the user
            // put there, and the day it arrived is not ours to claim.
            ..Record::default()
        });
        adopted.push(Adopted { name, folders });
    }

    if !adopted.is_empty() {
        ledger.save(&options.ledger_path)?;
    }
    adopted.sort_by_key(|a| a.name.to_lowercase());
    Ok(adopted)
}

/// Gather fingerprint matches into one entry per project.
///
/// Every folder of a package matches separately, BigWigs answers twelve
/// times, EllesmereUI twenty-two, and each answer names the same project.
/// Writing one record per match records one addon a dozen times over, and
/// reports it a dozen times to the user, which is how this was noticed.
fn group_by_project(
    matches: &std::collections::BTreeMap<String, super::catalog::Match>,
    by_print: &std::collections::BTreeMap<u32, String>,
) -> std::collections::BTreeMap<String, (super::catalog::Match, Vec<String>)> {
    let mut out: std::collections::BTreeMap<String, (super::catalog::Match, Vec<String>)> =
        Default::default();
    for (print, found) in matches {
        let Ok(print) = print.parse::<u32>() else { continue };
        let Some(folder) = by_print.get(&print) else { continue };

        out.entry(found.id.clone())
            .or_insert_with(|| (found.clone(), Vec::new()))
            .1
            .push(folder.clone());
    }
    out
}

/// Set or clear one addon's auto-update choice.
///
/// Clearing it (`None`) returns the addon to following the app-wide default,
/// which is not the same as turning auto-update off.
pub fn set_auto_update(
    options: &Options,
    provider: &str,
    id: &str,
    auto_update: Option<bool>,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.auto_update = auto_update;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}

/// Hold one addon at a file id, or let it track its channel again.
///
/// The caller passes the file to hold rather than a flag, because a pin is a
/// statement about a particular file: re-pinning after an update means
/// pinning to the *new* file, which a toggle could not express.
pub fn set_pinned(
    options: &Options,
    provider: &str,
    id: &str,
    file_id: Option<String>,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.pinned = file_id;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
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

    #[error("could not back up the game's settings first, so nothing was replaced")]
    Backup(#[from] BackupError),

    #[error("the settings backup stopped unexpectedly: {0}")]
    Join(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::Collection;

    #[test]
    fn a_pass_saving_keeps_choices_made_while_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("addons.json");
        let record = |file: &str| Record {
            provider: "curse".into(),
            id: "1".into(),
            name: "BigWigs".into(),
            file_id: Some(file.into()),
            folders: vec!["BigWigs".into()],
            ..Default::default()
        };
        // The pass read this, then took minutes installing file 101.
        let mut before = Ledger::default();
        before.record(record("100"));
        before.save(&path).unwrap();

        // Meanwhile the user ignored it and pinned it to what is installed.
        let mut now = before.clone();
        let row = now.find_mut("curse", "1").unwrap();
        row.ignored = true;
        row.pinned = Some("101".into());
        now.save(&path).unwrap();

        save_merged(&path, vec![(record("101"), None)], false).unwrap();
        let saved = Ledger::load(&path).unwrap();
        let row = saved.find("curse", "1").unwrap();
        assert_eq!(row.file_id.as_deref(), Some("101"), "the install is recorded");
        assert!(row.ignored, "and the ignore set meanwhile survives");
        assert_eq!(row.pinned.as_deref(), Some("101"));
    }

    const TOKEN: &str = concat!(
        "eyJjb2xsZWN0aW9uX25hbWUiOiJXb3dVcF9leHBvcnRfMSIsImNsaWVudF90eXBlIjoiUmV0YWlsIiwi",
        "YWRkb25zIjpbeyJpZCI6IjIzODIiLCJuYW1lIjoiQmlnV2lncyIsInByb3ZpZGVyX25hbWUiOiJDdXJz",
        "ZSIsInZlcnNpb25faWQiOiI4ODY2NTA4In0seyJpZCI6IjEzMzQ0IiwibmFtZSI6IkJ1Z0dyYWJiZXIi",
        "LCJwcm92aWRlcl9uYW1lIjoiQ3Vyc2UiLCJ2ZXJzaW9uX2lkIjoiODkwNzU4NyJ9XX0"
    );

    fn options_with(channel: crate::addons::Channel, ledger: &Path) -> Options {
        Options {
            flavor: "retail".to_owned(),
            addons_dir: PathBuf::from("/nowhere"),
            ledger_path: ledger.to_path_buf(),
            pinned: false,
            reinstall: false,
            channel,
            backup: None,
        }
    }

    #[test]
    fn an_addons_own_channel_beats_the_default_and_none_means_follow_it() {
        use crate::addons::Channel;

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: None,
            folders: vec!["BigWigs".into()],
            channel: Some(Channel::Alpha),
            ..Default::default()
        });
        ledger.record(Record {
            provider: "curse".into(),
            id: "13344".into(),
            name: "BugGrabber".into(),
            file_id: None,
            folders: vec!["BugGrabber".into()],
            channel: None,
            ..Default::default()
        });

        let options = options_with(Channel::Beta, Path::new("/nowhere/ledger.json"));
        let ask = |id: &str| Request {
            provider: "curse".to_owned(),
            id: id.to_owned(),
            version_id: None,
            channel: None,
        };

        assert_eq!(effective_channel(&ledger, &ask("2382"), &options), Channel::Alpha);
        // No opinion of its own, so it moves with the default.
        assert_eq!(effective_channel(&ledger, &ask("13344"), &options), Channel::Beta);
        // And an addon the ledger has never seen takes the default too.
        assert_eq!(effective_channel(&ledger, &ask("999"), &options), Channel::Beta);
    }

    #[test]
    fn an_ignored_addon_is_left_out_of_bulk_work_but_can_still_be_asked_for() {
        use crate::addons::Channel;

        let dir = std::env::temp_dir().join(format!("lf-ignore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.json");

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: Some("100".into()),
            folders: vec!["BigWigs".into()],
            ..Default::default()
        });
        ledger.record(Record {
            provider: "curse".into(),
            id: "13344".into(),
            name: "BugGrabber".into(),
            file_id: Some("200".into()),
            folders: vec!["BugGrabber".into()],
            ..Default::default()
        });
        ledger.save(&path).unwrap();

        let options = options_with(Channel::Stable, &path);
        set_ignored(&options, "curse", "13344", true).unwrap();

        let reloaded = Ledger::load(&path).unwrap();
        assert!(!reloaded.find("curse", "2382").unwrap().ignored);
        assert!(reloaded.find("curse", "13344").unwrap().ignored);

        // What "update everything" would ask for: the one addon still in.
        let requests = update_requests(&reloaded, Channel::Stable);
        let asked: Vec<&str> = requests.iter().map(|request| request.id.as_str()).collect();
        assert_eq!(asked, vec!["2382"], "an ignored addon is never asked about");

        // Ignoring is not a one-way door.
        set_ignored(&options, "curse", "13344", false).unwrap();
        assert!(!Ledger::load(&path).unwrap().find("curse", "13344").unwrap().ignored);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_update_follows_the_default_until_an_addon_says_otherwise() {
        use crate::addons::Channel;

        let mut ledger = Ledger::default();
        let mut add = |id: &str, auto: Option<bool>, ignored: bool| {
            ledger.record(Record {
                provider: "curse".into(),
                id: id.into(),
                name: id.into(),
                folders: vec![id.into()],
                auto_update: auto,
                ignored,
                ..Default::default()
            });
        };
        add("follows", None, false);
        add("opted-in", Some(true), false);
        add("opted-out", Some(false), false);
        add("ignored-but-opted-in", Some(true), true);

        let asked = |default_auto: bool| {
            let mut ids: Vec<String> = auto_update_requests(&ledger, Channel::Stable, default_auto)
                .iter()
                .map(|request| request.id.clone())
                .collect();
            ids.sort();
            ids
        };

        // Ignoring beats auto-updating: "leave this alone" is the stronger
        // of the two statements, whichever way the default points.
        assert_eq!(asked(true), vec!["follows", "opted-in"]);
        assert_eq!(asked(false), vec!["opted-in"]);

        // And "Update all" is a different question: it asks for everything
        // not ignored, whatever each addon does on its own.
        let mut all: Vec<String> = update_requests(&ledger, Channel::Stable)
            .iter()
            .map(|request| request.id.clone())
            .collect();
        all.sort();
        assert_eq!(all, vec!["follows", "opted-in", "opted-out"]);
    }

    #[test]
    fn setting_an_auto_update_choice_survives_a_reload_and_can_be_cleared() {
        use crate::addons::Channel;

        let dir = std::env::temp_dir().join(format!("lf-auto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.json");

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            folders: vec!["BigWigs".into()],
            ..Default::default()
        });
        ledger.save(&path).unwrap();

        let options = options_with(Channel::Stable, &path);
        set_auto_update(&options, "curse", "2382", Some(false)).unwrap();
        let record = Ledger::load(&path).unwrap().find("curse", "2382").unwrap().clone();
        assert_eq!(record.auto_update, Some(false));
        assert!(!record.updates_itself(true), "its own choice beats the default");

        set_auto_update(&options, "curse", "2382", None).unwrap();
        let record = Ledger::load(&path).unwrap().find("curse", "2382").unwrap().clone();
        assert_eq!(record.auto_update, None, "cleared means follow the default");
        assert!(record.updates_itself(true));
        assert!(!record.updates_itself(false));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_folder_of_a_package_matches_but_it_is_still_one_addon() {
        use crate::addons::catalog::Match;

        let bigwigs = Match {
            id: "2382".to_owned(),
            name: Some("BigWigs".to_owned()),
            file_id: Some("9017404".to_owned()),
            folders: vec!["BigWigs".to_owned(), "BigWigs_Core".to_owned()],
        };
        let other = Match {
            id: "1477613".to_owned(),
            name: Some("EllesmereUI".to_owned()),
            file_id: Some("9069177".to_owned()),
            folders: vec!["EllesmereUI".to_owned()],
        };

        // Three hashes, three answers, two addons, which is exactly the
        // shape that produced twelve BigWigs rows before this was grouped.
        let matches = std::collections::BTreeMap::from([
            ("10".to_owned(), bigwigs.clone()),
            ("20".to_owned(), bigwigs),
            ("30".to_owned(), other),
        ]);
        let by_print = std::collections::BTreeMap::from([
            (10, "BigWigs".to_owned()),
            (20, "BigWigs_Core".to_owned()),
            (30, "EllesmereUI".to_owned()),
            (40, "NotMatched".to_owned()),
        ]);

        let grouped = group_by_project(&matches, &by_print);
        assert_eq!(grouped.len(), 2, "one entry per project, not per folder");
        assert_eq!(grouped["2382"].1, ["BigWigs", "BigWigs_Core"]);
        assert_eq!(grouped["1477613"].1, ["EllesmereUI"]);

        // A hash nobody claimed contributes nothing rather than an entry
        // with no project.
        assert!(grouped.values().all(|(_, folders)| !folders.contains(&"NotMatched".to_owned())));
    }

    #[test]
    fn adoption_only_claims_folders_that_are_free_and_present() {
        // The decision adoption makes, without the network: given what the
        // catalog recognised, which folders may a new record claim?
        let claimed: std::collections::BTreeSet<String> =
            ["BigWigs".to_owned(), "BigWigs_Core".to_owned()].into_iter().collect();
        let on_disk: std::collections::BTreeSet<&str> =
            ["BigWigs", "BigWigs_Core", "Details", "Details_Streamer"].into_iter().collect();

        let claim = |matched: &str, package: &[&str]| -> Vec<String> {
            let mut folders: Vec<String> = package
                .iter()
                .filter(|f| on_disk.contains(*f) && !claimed.contains(**f))
                .map(|f| (*f).to_owned())
                .collect();
            if !folders.iter().any(|f| f == matched) {
                folders.push(matched.to_owned());
            }
            folders.sort();
            folders.dedup();
            folders
        };

        // The whole package, because a match on one folder identifies all of
        // it, adopting Details and leaving Details_Streamer unmanaged would
        // be worse than adopting neither.
        assert_eq!(claim("Details", &["Details", "Details_Streamer"]), ["Details", "Details_Streamer"]);

        // A folder the catalog lists but that is not installed is not
        // claimed: recording it would make Remove promise to delete it.
        assert_eq!(claim("Details", &["Details", "Details_Nope"]), ["Details"]);

        // And nothing is taken from a record that already owns it, which is
        // what stops adoption competing with the entry that installed it.
        assert_eq!(claim("Details", &["Details", "BigWigs_Core"]), ["Details"]);
    }

    #[test]
    fn a_pinned_addon_is_asked_about_by_nothing() {
        use crate::addons::Channel;

        let dir = std::env::temp_dir().join(format!("lf-pin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.json");

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: Some("100".into()),
            folders: vec!["BigWigs".into()],
            ..Default::default()
        });
        ledger.save(&path).unwrap();

        let options = options_with(Channel::Stable, &path);
        set_pinned(&options, "curse", "2382", Some("100".to_owned())).unwrap();

        let reloaded = Ledger::load(&path).unwrap();
        let record = reloaded.find("curse", "2382").unwrap();
        assert_eq!(record.pinned.as_deref(), Some("100"));
        // Pinning and ignoring are different statements that reach the same
        // place: an addon held still is not worth asking a provider about.
        assert!(!record.tracks_updates());
        assert!(!record.updates_itself(true));
        assert!(update_requests(&reloaded, Channel::Stable).is_empty());
        assert!(auto_update_requests(&reloaded, Channel::Stable, true).is_empty());

        // Unpinning is not a one-way door either.
        set_pinned(&options, "curse", "2382", None).unwrap();
        let reloaded = Ledger::load(&path).unwrap();
        assert!(reloaded.find("curse", "2382").unwrap().tracks_updates());
        assert_eq!(update_requests(&reloaded, Channel::Stable).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pin_describes_a_file_and_is_dropped_once_it_no_longer_does() {
        let resolve = |asked: Option<&str>, import: bool, held: Option<&str>, got: &str| {
            pin_after_install(
                asked.map(str::to_owned),
                import,
                held.map(str::to_owned),
                Some(got),
            )
        };

        // Importing someone's exact setup records the pin they asked for.
        assert_eq!(resolve(Some("100"), true, None, "100").as_deref(), Some("100"));
        // The same token imported as a list of addons pins nothing.
        assert_eq!(resolve(Some("100"), false, None, "100"), None);
        // An ordinary reinstall at the pinned file keeps the pin.
        assert_eq!(resolve(None, false, Some("100"), "100").as_deref(), Some("100"));
        // Installing a different file has moved the addon off the pin, and a
        // pin naming a file that is not there would be a lie.
        assert_eq!(resolve(None, false, Some("100"), "101"), None);
    }

    #[test]
    fn setting_a_channel_survives_a_reload_and_clearing_it_returns_to_the_default() {
        use crate::addons::Channel;

        let dir = std::env::temp_dir().join(format!("lf-channel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.json");

        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: None,
            folders: vec!["BigWigs".into()],
            channel: None,
            ..Default::default()
        });
        ledger.save(&path).unwrap();

        let options = options_with(Channel::Stable, &path);
        set_channel(&options, "curse", "2382", Some(Channel::Beta)).unwrap();
        assert_eq!(
            Ledger::load(&path).unwrap().find("curse", "2382").unwrap().channel,
            Some(Channel::Beta)
        );

        set_channel(&options, "curse", "2382", None).unwrap();
        assert_eq!(
            Ledger::load(&path).unwrap().find("curse", "2382").unwrap().channel,
            None,
            "cleared means follow the default, not stable"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

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
    fn a_channel_change_is_a_switch_rather_than_an_update() {
        use crate::addons::Channel;

        let status = |installed_build, available_build| UpdateStatus {
            provider: "curse".into(),
            id: "1682733".into(),
            name: "Naowh Smart Reminders".into(),
            // Different files, so `has_update` is true either way, which is
            // exactly why the label cannot be decided from that alone.
            installed: Some("8986554".into()),
            available: Some("8986546".into()),
            installed_build,
            available_build,
            available_name: Some("NaowhSmartReminders-1.4.26.zip".into()),
            available_date: None,
            problem: None,
        };

        // The real case: an alpha was installed, stable is now what is wanted,
        // and the stable file is *older*. Offering "Update" would be a lie.
        let back_to_stable = status(Some(Channel::Alpha), Some(Channel::Stable));
        assert!(back_to_stable.has_update());
        assert!(back_to_stable.is_channel_switch());

        // Opting into a pre-release is a switch too, not an update.
        assert!(status(Some(Channel::Stable), Some(Channel::Alpha)).is_channel_switch());

        // Moving forward on the same channel is an ordinary update.
        assert!(!status(Some(Channel::Stable), Some(Channel::Stable)).is_channel_switch());

        // Installed before builds were recorded: do not guess.
        assert!(!status(None, Some(Channel::Stable)).is_channel_switch());
        assert!(!status(Some(Channel::Beta), None).is_channel_switch());
    }

    #[test]
    fn an_unknown_remote_version_is_not_an_update() {
        let status = |installed: Option<&str>, available: Option<&str>| UpdateStatus {
            provider: "curse".into(),
            id: "1".into(),
            name: "Thing".into(),
            installed: installed.map(str::to_owned),
            available: available.map(str::to_owned),
            installed_build: None,
            available_build: None,
            available_name: None,
            available_date: None,
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
