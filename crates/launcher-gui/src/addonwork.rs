//! Driving [`lightforge_core::addons`] from the GUI.
//!
//! Scanning is a disk read and goes off-thread; importing and updating are
//! network work that publishes into a `watch` channel the dashboard samples on
//! its timer, the same way downloads and provisioning do.

use std::path::PathBuf;

use lightforge_core::addons::catalog::{Catalog, Origin, Request};
use lightforge_core::addons::manage::{self, Event, Options, Report};
use lightforge_core::addons::{self, Collection, Installed, Ledger};
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
}

/// What the addon screen shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub rows: Vec<Row>,
    /// Ledger entries whose folders are actually on disk.
    pub managed: usize,
    /// Ledger entries whose folders are gone — deleted by hand, or the game
    /// reinstalled. Counted separately because claiming to manage something
    /// that is not there is how "0 installed · 3 managed" happens.
    pub stale: usize,
    pub dir: PathBuf,
}

/// Scan a product's addons and fold in what the ledger knows.
pub async fn list(layout: Layout, code: ProductCode) -> Listing {
    tokio::task::spawn_blocking(move || {
        let dir = layout.addons_dir(&code);
        let installed = addons::scan(&dir).unwrap_or_default();
        let ledger = AppDirs::discover()
            .ok()
            .map(|dirs| manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code))
            .and_then(|path| Ledger::load(&path).ok())
            .unwrap_or_default();

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
    let mut owner: std::collections::BTreeMap<&str, (&str, &str, &str)> = Default::default();
    for record in &ledger.addons {
        for (position, folder) in record.folders.iter().enumerate() {
            let head = match position {
                0 => "",
                _ => record.folders[0].as_str(),
            };
            owner.insert(folder, (record.provider.as_str(), record.id.as_str(), head));
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
                managed: known.map(|(provider, id, _)| Managed {
                    provider: (*provider).to_owned(),
                    id: (*id).to_owned(),
                }),
                child_of: known
                    .map(|(_, _, head)| *head)
                    .filter(|head| !head.is_empty())
                    .map(str::to_owned),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightforge_core::addons::Record;

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

fn options(layout: &Layout, code: &ProductCode, pinned: bool) -> Option<Options> {
    let dirs = AppDirs::discover().ok()?;
    Some(Options {
        flavor: addons::flavor_of(code).to_owned(),
        addons_dir: layout.addons_dir(code),
        ledger_path: manage::ledger_path(dirs.data_dir(), &layout.addons_dir(code), code),
        pinned,
    })
}

fn catalog() -> Option<Catalog> {
    let dirs = AppDirs::discover().ok()?;
    let config = Config::load_or_default(&dirs.config_file()).ok()?;
    let source = config.addon_catalog?;
    Some(Catalog::new(match source.contains("://") {
        true => Origin::Http(source),
        false => Origin::Dir(PathBuf::from(source)),
    }))
}

/// Install everything a pasted token names.
pub async fn import(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    token: String,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let collection = Collection::parse(&token).map_err(|e| e.to_string())?;
    let requests = manage::requests_from(&collection);
    if requests.is_empty() {
        return Err("that token names no addons Lightforge can install".to_owned());
    }
    run(http, layout, code, requests, false, tx).await
}

/// Update everything the ledger tracks.
pub async fn update(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let ledger = Ledger::load(&manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code))
        .map_err(|e| e.to_string())?;
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
        return Ok(Report::default());
    }
    run(http, layout, code, requests, false, tx).await
}

async fn run(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    requests: Vec<Request>,
    pinned: bool,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    let options = options(&layout, &code, pinned).ok_or("no data directory available")?;
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
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Report, String> {
    run(
        http,
        layout,
        code,
        vec![Request { provider, id, version_id: None }],
        false,
        tx,
    )
    .await
}

/// Ask what has updates, without installing anything.
pub async fn check_updates(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
) -> Result<Vec<manage::UpdateStatus>, String> {
    let options = options(&layout, &code, false).ok_or("no data directory available")?;
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
        let options = options(&layout, &code, false).ok_or("no data directory available")?;
        manage::remove(&options, &provider, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
