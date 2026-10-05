//! Driving [`lightforge_core::addons`] from the GUI.
//!
//! Scanning is a disk read and goes off-thread; importing and updating are
//! network work that publishes into a `watch` channel the dashboard samples on
//! its timer, the same way downloads and provisioning do.

use std::path::PathBuf;

use lightforge_core::addons::catalog::{Catalog, Request};
use lightforge_core::addons::manage::{self, Event, Options, Report};
use lightforge_core::addons::{self, Channel, Collection, Installed, Ledger, Record};
use lightforge_core::config::Config;
use lightforge_core::net::Http;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::ProductCode;

/// One row of the addon list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub folder: String,
    pub title: String,
    pub version: String,
    /// Set when Lightforge installed it — only these can be updated or removed.
    pub managed: Option<Managed>,
    /// Folders belonging to a managed addon other than its first are shown as
    /// children rather than as separate addons.
    pub child_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    pub provider: String,
    pub id: String,
    /// This addon's own channel, or `None` when it follows the default.
    pub channel: Option<Channel>,
    /// Left out of everything that updates in bulk.
    pub ignored: bool,
    /// This addon's own auto-update choice, or `None` when it follows the
    /// app-wide default.
    pub auto_update: Option<bool>,
    /// The catalog file id installed, which is what a pin would hold.
    pub file_id: Option<String>,
    /// The file id this addon is held at, when it is pinned.
    pub pinned: Option<String>,
    /// The day this build was installed, `YYYY-MM-DD`. `None` for anything
    /// installed before Lightforge started recording it.
    pub installed_at: Option<String>,
}

/// What the addon screen shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub rows: Vec<Row>,
    /// Ledger entries whose folders are actually on disk.
    pub managed: usize,
    /// Ledger entries whose folders are gone. Reconciled away on every scan
    /// of a readable directory, so this is only ever non-zero for a library
    /// we cannot reach — where claiming to manage something that is not there
    /// is how "0 installed · 3 managed" happens.
    pub stale: usize,
    pub dir: PathBuf,
}

/// Scan a product's addons and fold in what the ledger knows.
pub async fn list(layout: Layout, code: ProductCode) -> Listing {
    tokio::task::spawn_blocking(move || {
        let dir = layout.addons_dir(&code);
        let installed = addons::scan(&dir).unwrap_or_default();
        let path = AppDirs::discover()
            .ok()
            .map(|dirs| manage::ledger_path(dirs.data_dir(), &dir, &code));
        let mut ledger = path
            .as_ref()
            .and_then(|path| Ledger::load(path).ok())
            .unwrap_or_default();

        // An addon whose folders are all gone is not installed, whatever the
        // ledger says, and offering to update it is a promise we cannot keep.
        // Only ever reconciled against a directory we could read.
        if dir.is_dir() {
            let dropped = ledger.prune_missing(&dir);
            if !dropped.is_empty() {
                if let Some(path) = &path {
                    let _ = ledger.save(path);
                }
                crate::trace::mark(format_args!(
                    "dropped from the addon ledger, folders gone: {}",
                    dropped.join(", ")
                ));
            }
        }

        let present: std::collections::BTreeSet<&str> =
            installed.iter().map(|a| a.folder.as_str()).collect();
        let managed = ledger
            .addons
            .iter()
            .filter(|record| record.folders.iter().any(|f| present.contains(f.as_str())))
            .count();

        Listing {
            managed,
            stale: ledger.addons.len() - managed,
            rows: rows_from(&installed, &ledger),
            dir,
        }
    })
    .await
    .unwrap_or_default()
}

/// Turn a scan plus a ledger into display rows.
///
/// The ledger owns the grouping: a package's first folder is the addon, and its
/// other folders are children of it. Without that, BigWigs renders as eleven
/// separate addons with eleven Remove buttons, each of which would delete the
/// other ten.
pub fn rows_from(installed: &[Installed], ledger: &Ledger) -> Vec<Row> {
    // The record itself, plus which folder heads the package it came in —
    // empty for the package's own first folder. Carrying the record rather
    // than a widening tuple keeps a new per-addon setting to one line below.
    type Owner<'a> = (&'a Record, &'a str);
    let mut owner: std::collections::BTreeMap<&str, Owner<'_>> = Default::default();
    for record in &ledger.addons {
        for (position, folder) in record.folders.iter().enumerate() {
            let head = match position {
                0 => "",
                _ => record.folders[0].as_str(),
            };
            owner.insert(folder, (record, head));
        }
    }

    installed
        .iter()
        .map(|addon: &Installed| {
            let known = owner.get(addon.folder.as_str());
            Row {
                folder: addon.folder.clone(),
                title: addon.title.clone().unwrap_or_else(|| addon.folder.clone()),
                version: addon.version.clone().unwrap_or_else(|| "—".to_owned()),
                managed: known.map(|(record, _)| Managed {
                    provider: record.provider.clone(),
                    id: record.id.clone(),
                    channel: record.channel,
                    ignored: record.ignored,
                    auto_update: record.auto_update,
                    file_id: record.file_id.clone(),
                    pinned: record.pinned.clone(),
                    installed_at: record.installed_at.clone(),
                }),
                child_of: known
                    .map(|(_, head)| *head)
                    .filter(|head| !head.is_empty())
                    .map(str::to_owned),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(folder: &str, version: &str) -> Installed {
        Installed {
            folder: folder.to_owned(),
            path: PathBuf::from(folder),
            title: Some(folder.to_owned()),
            version: Some(version.to_owned()),
            author: None,
            ids: Default::default(),
            interfaces: Vec::new(),
        }
    }

    #[test]
    fn a_multi_folder_package_is_one_addon_with_children() {
        let scan = vec![
            installed("BigWigs", "v424.8"),
            installed("BigWigs_Core", "v424.8"),
            installed("BigWigs_Options", "v424.8"),
        ];
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "wowinterface".into(),
            id: "5086".into(),
            name: "BigWigs".into(),
            file_id: Some("v424.8".into()),
            folders: vec!["BigWigs".into(), "BigWigs_Core".into(), "BigWigs_Options".into()],
            channel: None,
            ..Default::default()
        });

        let rows = rows_from(&scan, &ledger);
        let heads: Vec<&Row> = rows.iter().filter(|r| r.child_of.is_none()).collect();

        // One addon on screen, not three.
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].folder, "BigWigs");
        assert!(heads[0].managed.is_some());
        // The other two are children of it, so they get no Remove button.
        assert_eq!(rows[1].child_of.as_deref(), Some("BigWigs"));
        assert_eq!(rows[2].child_of.as_deref(), Some("BigWigs"));
    }

    #[test]
    fn a_hand_installed_addon_is_listed_but_not_managed() {
        let scan = vec![installed("HandInstalled", "1.0")];
        let rows = rows_from(&scan, &Ledger::default());

        assert_eq!(rows.len(), 1);
        // No managed marker means the UI offers no Remove: Lightforge did not
        // put it there and must not delete it.
        assert!(rows[0].managed.is_none());
        assert!(rows[0].child_of.is_none());
    }

    #[test]
    fn a_rows_channel_comes_from_the_ledger_and_children_do_not_lose_it() {
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: None,
            folders: vec!["BigWigs".into(), "BigWigs_Options".into()],
            channel: Some(Channel::Beta),
            ..Default::default()
        });

        let scan = vec![installed("BigWigs", "1.0"), installed("BigWigs_Options", "1.0")];
        let rows = rows_from(&scan, &ledger);

        let head = rows.iter().find(|r| r.folder == "BigWigs").unwrap();
        assert_eq!(head.managed.as_ref().unwrap().channel, Some(Channel::Beta));
        // The picker is only drawn on the head row, but a child carrying the
        // wrong channel would be a lie waiting to be shown.
        let child = rows.iter().find(|r| r.folder == "BigWigs_Options").unwrap();
        assert_eq!(child.managed.as_ref().unwrap().channel, Some(Channel::Beta));
        assert_eq!(child.child_of.as_deref(), Some("BigWigs"));
    }

    #[test]
    fn an_ignored_addon_reaches_the_row_and_leaves_the_update_set() {
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            folders: vec!["BigWigs".into(), "BigWigs_Core".into()],
            ignored: true,
            ..Default::default()
        });

        let rows = rows_from(&[installed("BigWigs", "1.0"), installed("BigWigs_Core", "1.0")], &ledger);
        // Both folders carry it: the child row is the same addon, so it must
        // not read as managed-and-updating while its parent is ignored.
        assert!(rows.iter().all(|row| row.managed.as_ref().unwrap().ignored));
        assert!(manage::update_requests(&ledger, Channel::Stable).is_empty());
    }

    #[test]
    fn an_addons_own_auto_update_choice_reaches_the_row() {
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            folders: vec!["BigWigs".into()],
            auto_update: Some(false),
            ..Default::default()
        });

        let rows = rows_from(&[installed("BigWigs", "1.0")], &ledger);
        // `None` and `Some(false)` mean different things to the row — one
        // follows the app's default, the other overrides it — so the row has
        // to carry the choice itself rather than a resolved boolean.
        assert_eq!(rows[0].managed.as_ref().unwrap().auto_update, Some(false));
        assert!(manage::auto_update_requests(&ledger, Channel::Stable, true).is_empty());
    }

    #[test]
    fn a_pinned_addon_reaches_the_row_with_the_file_it_is_held_at() {
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "2382".into(),
            name: "BigWigs".into(),
            file_id: Some("100".into()),
            folders: vec!["BigWigs".into()],
            pinned: Some("100".into()),
            ..Default::default()
        });

        let rows = rows_from(&[installed("BigWigs", "1.0")], &ledger);
        let managed = rows[0].managed.as_ref().unwrap();
        // The row needs both: the pin to know it is held, and the installed
        // file so the menu can offer to pin an addon that is not.
        assert_eq!(managed.pinned.as_deref(), Some("100"));
        assert_eq!(managed.file_id.as_deref(), Some("100"));
        assert!(manage::update_requests(&ledger, Channel::Stable).is_empty());
    }

    #[test]
    fn a_stale_ledger_entry_is_not_counted_as_managed() {
        // Seen for real: the game folder was deleted by hand and the screen
        // then read "0 installed · 1 managed", which is a contradiction.
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "wowinterface".into(),
            id: "5086".into(),
            name: "BigWigs".into(),
            file_id: None,
            folders: vec!["BigWigs".into()],
            channel: None,
            ..Default::default()
        });

        let present: std::collections::BTreeSet<&str> = Default::default();
        let managed = ledger
            .addons
            .iter()
            .filter(|r| r.folders.iter().any(|f| present.contains(f.as_str())))
            .count();
        assert_eq!(managed, 0);
        assert_eq!(ledger.addons.len() - managed, 1, "it is stale, not managed");
    }

    #[test]
    fn a_ledger_entry_whose_folders_are_gone_does_not_invent_rows() {
        let mut ledger = Ledger::default();
        ledger.record(Record {
            provider: "curse".into(),
            id: "1".into(),
            name: "Deleted".into(),
            file_id: None,
            folders: vec!["Deleted".into()],
            channel: None,
            ..Default::default()
        });

        // The scan is the truth about what exists on disk.
        assert!(rows_from(&[], &ledger).is_empty());
    }
}

/// Live progress of an import or update.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub line: String,
    pub done: usize,
    pub total: usize,
}

/// The same options the work functions build, for a caller that only needs to
/// read or write the ledger — setting one addon's channel, say.
pub fn manage_options(layout: &Layout, code: &ProductCode, channel: Channel) -> Option<Options> {
    options(layout, code, false, channel)
}

fn options(
    layout: &Layout,
    code: &ProductCode,
    pinned: bool,
    channel: Channel,
) -> Option<Options> {
    let dirs = AppDirs::discover().ok()?;
    Some(Options {
        flavor: addons::flavor_of(code).to_owned(),
        addons_dir: layout.addons_dir(code),
        ledger_path: manage::ledger_path(dirs.data_dir(), &layout.addons_dir(code), code),
        pinned,
        reinstall: false,
        channel,
    })
}

fn catalog() -> Option<Catalog> {
    let dirs = AppDirs::discover().ok()?;
    let config = Config::load_or_default(&dirs.config_file()).ok()?;
    Some(Catalog::new(config.catalog_origin()))
}

/// Install everything a pasted token names.
pub async fn import(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    token: String,
    channel: Channel,
    // Install the exact files the token names and hold them there, rather
    // than taking the newest of each.
    pinned: bool,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let collection = Collection::parse(&token).map_err(|e| e.to_string())?;
    let requests = manage::requests_from(&collection);
    if requests.is_empty() {
        return Err("that token names no addons Lightforge can install".to_owned());
    }
    run(http, layout, code, requests, Mode { pinned, ..Mode::default() }, channel, tx).await
}

/// Update everything the ledger tracks.
pub async fn update(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    channel: Channel,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let ledger = Ledger::load(&manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code))
        .map_err(|e| e.to_string())?;
    let requests = manage::update_requests(&ledger, channel);

    if requests.is_empty() {
        return Ok(Report::default());
    }
    run(http, layout, code, requests, Mode::default(), channel, tx).await
}

/// Update the addons that update themselves, with nobody watching.
///
/// Returns `None` when there is nothing to do, so the caller can tell "no work"
/// from "worked, and here is what happened" without inspecting a report.
pub async fn auto_update(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    channel: Channel,
    default_auto: bool,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Option<Report>, String> {
    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let ledger = Ledger::load(&manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code))
        .map_err(|e| e.to_string())?;
    let requests = manage::auto_update_requests(&ledger, channel, default_auto);

    if requests.is_empty() {
        return Ok(None);
    }
    run(http, layout, code, requests, Mode::default(), channel, tx).await.map(Some)
}

/// How one run differs from an ordinary install.
///
/// A struct rather than two positional `bool`s: `run(.., false, true, ..)` at
/// the call site says nothing about which is which.
#[derive(Debug, Clone, Copy, Default)]
struct Mode {
    /// Honour the version ids the requests name, instead of taking the newest.
    pinned: bool,
    /// Install over what is there even when the ledger says it is current.
    reinstall: bool,
}

async fn run(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    requests: Vec<Request>,
    mode: Mode,
    channel: Channel,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let options = Options {
        reinstall: mode.reinstall,
        ..options(&layout, &code, mode.pinned, channel)
            .ok_or("no data directory available")?
    };
    let catalog = catalog();
    let http = http.ok_or(crate::tasks::NO_HTTP)?;

    let total = requests.len();
    let mut progress = Progress { total, ..Progress::default() };
    let on_event = move |event: Event| {
        match event {
            Event::Resolving { count } => progress.line = format!("resolving {count} addons…"),
            Event::Resolved { ok, unavailable } => {
                progress.line = format!("{ok} available, {unavailable} not")
            }
            Event::Downloading { name } => progress.line = format!("fetching {name}…"),
            Event::Installed { name, .. } => {
                progress.done += 1;
                progress.line = format!("installed {name}");
            }
            Event::UpToDate { name } => {
                progress.done += 1;
                progress.line = format!("{name} is up to date");
            }
            Event::Unavailable { name, .. } => {
                progress.done += 1;
                progress.line = format!("{name} unavailable");
            }
        }
        let _ = tx.send(progress.clone());
    };

    manage::install_all(&http, catalog.as_ref(), &requests, &options, on_event)
        .await
        .map_err(|e| e.to_string())
}

/// Search every provider that can answer.
pub async fn search(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    query: String,
) -> SearchOutcome {
    use lightforge_core::addons::browse;

    let Ok(dirs) = AppDirs::discover() else {
        return SearchOutcome::default();
    };
    let Some(http) = http else {
        return SearchOutcome::default();
    };
    let _ = &layout;

    let results = browse::search(
        &http,
        catalog().as_ref(),
        dirs.cache_dir(),
        &query,
        addons::flavor_of(&code),
        30,
    )
    .await;

    SearchOutcome {
        query,
        listings: results.listings,
        problems: results.problems,
    }
}

/// A completed search, kept with its query so a late reply cannot overwrite
/// the results of a newer one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    pub query: String,
    pub listings: Vec<lightforge_core::addons::browse::Listing>,
    pub problems: Vec<String>,
}

/// Install one addon by provider and id.
pub async fn install_one(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    provider: String,
    id: String,
    channel: Channel,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    run(
        http,
        layout,
        code,
        // `install_all` fills the channel in from the ledger or the default.
        vec![Request { provider, id, version_id: None, channel: None }],
        Mode::default(),
        channel,
        tx,
    )
    .await
}

/// Recognise hand-installed addons and take them into the ledger.
pub async fn adopt(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    channel: Channel,
) -> Result<Vec<manage::Adopted>, String> {
    let options =
        options(&layout, &code, false, channel).ok_or("no data directory available")?;
    let http = http.ok_or(crate::tasks::NO_HTTP)?;
    manage::adopt(&http, catalog().as_ref(), &options)
        .await
        .map_err(|e| e.to_string())
}

/// Every build the catalogue offers for one addon, newest first.
///
/// Only CurseForge publishes a file list, so this is the one provider a
/// version picker can offer choices for. An empty list is the honest answer
/// for the others rather than an error: they have one build, and that build
/// is what is already installed.
pub async fn versions(
    http: Option<Http>,
    code: ProductCode,
    provider: String,
    id: String,
) -> Result<Vec<lightforge_core::addons::catalog::FileInfo>, String> {
    if addons::Provider::parse(&provider) != addons::Provider::Curse {
        return Ok(Vec::new());
    }
    let (Some(http), Some(catalog)) = (http, catalog()) else {
        return Err("no addon catalog is configured".to_owned());
    };
    catalog
        .files(&http, &id, addons::flavor_of(&code), VERSION_CHOICES)
        .await
        .map_err(|e| e.to_string())
}

/// How many builds back a picker offers.
///
/// The proxy caps it at fifty. Twenty is a year of releases for an addon
/// that ships monthly and a fortnight for one that ships nightly, and past
/// that nobody is choosing a version, they are archaeology.
const VERSION_CHOICES: usize = 20;

/// Install one exact build and hold the addon at it.
///
/// Both halves matter: naming the file is what makes a downgrade possible,
/// and pinning is what stops the next pass undoing it a minute later.
pub async fn install_version(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    provider: String,
    id: String,
    file_id: String,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    run(
        http,
        layout,
        code,
        vec![Request { provider, id, version_id: Some(file_id), channel: None }],
        Mode { pinned: true, reinstall: false },
        // No channel is consulted: naming the file *is* the choice, and the
        // channel only ever decides which file to pick when nobody has.
        Channel::default(),
        tx,
    )
    .await
}

/// Fetch one addon again and replace the folders it wrote.
///
/// A pin is read from the ledger here rather than passed in: repairing a
/// pinned addon has to put back the version it is held at, and the ledger is
/// the only thing that knows that for certain.
pub async fn reinstall_one(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    provider: String,
    id: String,
    channel: Channel,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let ledger = Ledger::load(&manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code))
        .map_err(|e| e.to_string())?;
    let pinned_file = ledger.find(&provider, &id).and_then(|record| record.pinned.clone());

    run(
        http,
        layout,
        code,
        vec![Request {
            provider,
            id,
            version_id: pinned_file.clone(),
            channel: None,
        }],
        Mode { pinned: pinned_file.is_some(), reinstall: true },
        channel,
        tx,
    )
    .await
}

/// A token naming everything Lightforge installed for this product.
///
/// Built from the ledger rather than from the folders on disk: an addon
/// Lightforge did not install has no provider or id to name, so it cannot be
/// written into a token anyone could import.
pub fn export(layout: &Layout, code: &ProductCode) -> Result<String, String> {
    // Reads the ledger and resolves nothing, so the channel is never consulted.
    let options =
        options(layout, code, false, Channel::default()).ok_or("no data directory available")?;
    let ledger = Ledger::load(&options.ledger_path).map_err(|e| e.to_string())?;

    let collection = addons::Collection {
        collection_name: format!("Lightforge · {}", code.as_str()),
        client_type: addons::Collection::client_type_of(code).to_owned(),
        addons: ledger
            .addons
            .iter()
            .map(|record| addons::Entry {
                id: record.id.clone(),
                name: record.name.clone(),
                provider: record.provider.clone(),
                version_id: record.file_id.clone(),
            })
            .collect(),
    };

    collection.to_token().map_err(|e| e.to_string())
}

/// Ask what has updates, without installing anything.
pub async fn check_updates(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    channel: Channel,
) -> Result<Vec<manage::UpdateStatus>, String> {
    let options =
        options(&layout, &code, false, channel).ok_or("no data directory available")?;
    let http = http.ok_or(crate::tasks::NO_HTTP)?;
    manage::check_updates(&http, catalog().as_ref(), &options)
        .await
        .map_err(|e| e.to_string())
}

/// Remove an addon Lightforge installed.
pub async fn remove(
    layout: Layout,
    code: ProductCode,
    provider: String,
    id: String,
) -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(move || {
        // Deletes folders the ledger names; resolves nothing.
        let options = options(&layout, &code, false, Channel::default())
            .ok_or("no data directory available")?;
        manage::remove(&options, &provider, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
