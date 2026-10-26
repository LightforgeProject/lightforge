//! Scanning, repairing, uninstalling and freeing space.

use super::*;

/// What a scan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scanned {
    pub checked: u64,
    pub damaged: u64,
    /// Entries dropped from the indices, which the next download refetches.
    pub removed: usize,
}

/// Verify the store's entries against their own headers, and drop the bad ones.
///
/// Repair rewrites the indices a running client memory-maps, so the prefix
/// has to be quiet first. A game or an open Battle.net is the user's to
/// close; anything else, a `wineserver` waiting out its timeout or a launch's
/// leftovers, is put to rest here exactly as a launch's pre-flight would.
pub async fn scan_and_repair(
    layout: Layout,
    code: ProductCode,
    tx: tokio::sync::watch::Sender<lightforge_core::tact::verify::Progress>,
) -> Result<Scanned, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::tact::storage::Storage;
        use lightforge_core::tact::verify;

        use lightforge_core::preflight;

        if let Some(game) = preflight::games_running(&layout).first() {
            return Err(format!("close {} first — the store is in use", game.name));
        }
        if preflight::client_running(&layout) {
            return Err("close Battle.net first — the store is in use".to_owned());
        }
        if !preflight::scan(&layout).is_empty() {
            let runner = runner_for(&layout, &code, Runs::Game)?;
            preflight::shutdown(&runner, &layout, preflight::DEFAULT_GRACE)
                .map_err(|e| chain(&e))?;
        }

        let install = layout.wow_install(&code);
        tracing::info!("scan and repair {} at {}", code.as_str(), install.display());
        let store = Storage::open(install.join("Data")).map_err(|e| chain(&e))?;
        let report = verify::scan(&store, |progress| {
            let _ = tx.send(progress);
        });
        tracing::info!("scan checked {} entries, {} damaged", report.checked, report.damaged.len());

        let removed = match report.is_healthy() {
            true => 0,
            false => verify::repair(&install.join("Data").join("data"), &report.damaged)
                .map_err(|e| chain(&e))?,
        };
        Ok(Scanned {
            checked: report.checked,
            damaged: report.damaged.len() as u64,
            removed,
        })
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Hash every stored byte against its chunk table and key.
///
/// Only reads, so unlike [`scan_and_repair`] it does not wait for the game to
/// close; the caller holds the download queue for as long as it runs, and
/// the core refuses a result from a store that changed underneath it anyway.
pub async fn deep_scan(
    layout: Layout,
    code: ProductCode,
    cancel: Cancel,
    tx: tokio::sync::watch::Sender<lightforge_core::tact::verify::Progress>,
) -> Result<lightforge_core::tact::verify::Report, String> {
    tokio::task::spawn_blocking(move || {
        tracing::info!("deep scan of {} started", code.as_str());
        let report = lightforge_core::tact::verify::deep_scan(layout.wow_install(&code), &cancel, |progress| {
            let _ = tx.send(progress);
        })
        .map_err(|e| chain(&e));
        match &report {
            Ok(r) => tracing::info!("deep scan of {} found {} damaged", code.as_str(), r.damaged.len()),
            Err(e) => tracing::error!("deep scan of {} failed: {e}", code.as_str()),
        }
        report
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Why a repair did not happen.
#[derive(Debug, Clone)]
pub enum Unrepaired {
    /// Something is alive in the prefix; the repair is still owed.
    InUse,
    /// Anything else, including a store that moved since the scan, whose
    /// findings are no longer worth acting on.
    Failed(String),
}

/// Drop what a deep scan found damaged, if nothing is using the store and it
/// is still the store that was scanned.
pub async fn repair(
    layout: Layout,
    code: ProductCode,
    report: lightforge_core::tact::verify::Report,
) -> Result<usize, Unrepaired> {
    tokio::task::spawn_blocking(move || {
        if !lightforge_core::preflight::scan(&layout).is_empty() {
            return Err(Unrepaired::InUse);
        }
        let data_dir = layout.wow_install(&code).join("Data").join("data");
        let repaired = lightforge_core::tact::verify::repair_unchanged(&data_dir, &report)
            .map_err(|e| Unrepaired::Failed(chain(&e)));
        match &repaired {
            Ok(n) => tracing::info!("repair of {} removed {n} damaged entries", code.as_str()),
            Err(e) => tracing::error!("repair of {} failed: {e:?}", code.as_str()),
        }
        repaired
    })
    .await
    .map_err(|e| Unrepaired::Failed(chain(&e)))?
}

/// What uninstalling a product would do, before anything is touched.
#[derive(Debug, Clone)]
pub struct UninstallPreview {
    pub plan: lightforge_core::tact::uninstall::Plan,
    /// The shared store's space it would give back. `None` when the whole
    /// store goes anyway; an error when what it shares could not be checked,
    /// in which case the store is left as it is.
    pub reclaim: Option<Result<lightforge_core::tact::reclaim::Reclaim, String>>,
}

/// Plan the store reclaim for `code`, unless the whole store is going.
pub(crate) async fn reclaim_plan(
    http: &Http,
    install: &std::path::Path,
    code: &ProductCode,
    plan: &lightforge_core::tact::uninstall::Plan,
) -> Option<Result<lightforge_core::tact::reclaim::Reclaim, String>> {
    use lightforge_core::tact::reclaim;
    if plan.is_whole_install() || !plan.has_row {
        return None;
    }
    let departing = lightforge_core::buildinfo::BuildInfo::load(install.join(".build.info"))
        .map_err(|e| chain(&e))
        .and_then(|info| reclaim::Build::installed(&info, code).map_err(|e| chain(&e)));
    Some(match departing {
        Ok(build) => reclaim::plan(http, install, &build).await.map_err(|e| chain(&e)),
        Err(e) => Err(e),
    })
}

/// What uninstalling `code` would delete and free. Nothing is touched.
pub async fn uninstall_plan(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
) -> Result<UninstallPreview, String> {
    let install = layout.wow_install(&code);
    let plan = {
        let (install, code) = (install.clone(), code.clone());
        tokio::task::spawn_blocking(move || {
            lightforge_core::tact::uninstall::plan(&install, &code).map_err(|e| chain(&e))
        })
        .await
        .map_err(|e| chain(&e))??
    };
    let reclaim = match &http {
        Some(http) => reclaim_plan(http, &install, &code, &plan).await,
        None => Some(Err(NO_HTTP.to_owned())),
    };
    Ok(UninstallPreview { plan, reclaim })
}

/// Uninstall `code`: its files, its `.build.info` row and `.product.db`
/// record, the shared store's data only it used (or the whole store if it
/// was the last product), and its entry in Battle.net's registry. Returns
/// the bytes freed.
///
/// Refuses while anything is alive in the prefix, the same gate repair uses:
/// deleting files a running client has open, or a registry Agent rewrites on
/// exit, is not something to race.
///
/// Everything is planned again here rather than trusted from when the dialog
/// opened. The reclaim is planned *before* the product's row goes, since the
/// row is what names the build whose files it was.
pub async fn uninstall(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    copied: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<u64, Problem> {
    use lightforge_core::tact::{productdb, reclaim, uninstall};

    let alive = layout.clone();
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&alive))
        .await
        .map_err(|e| Problem::of(&e))?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name).into());
    }

    let install = layout.wow_install(&code);
    let windows_path = layout
        .library(&code)
        .map(|library| library.windows_wow_path())
        .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned());
    let plan = uninstall::plan(&install, &code).map_err(|e| Problem::of(&e))?;
    tracing::info!("uninstalling {} from {}", code.as_str(), install.display());
    // A reclaim that cannot be planned is skipped, not fatal: the uninstall
    // itself is still exactly what was asked for.
    let reclaim = match &http {
        Some(http) => reclaim_plan(http, &install, &code, &plan).await.and_then(Result::ok),
        None => None,
    };

    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        let removed = uninstall::execute(&install, &code, &plan, &windows_path)
            .map_err(|e| Problem::of(&e))?;
        let reclaimed = match &reclaim {
            Some(reclaim) => reclaim::execute(&install, reclaim, |bytes| {
                copied.store(bytes, std::sync::atomic::Ordering::Relaxed)
            })
            .map_err(|e| Problem::of(&e))?,
            None => 0,
        };
        productdb::unregister(&layout.agent_dir(), code.as_str()).map_err(|e| Problem::of(&e))?;
        tracing::info!("uninstalled {}: {} B removed, {reclaimed} B reclaimed", code.as_str(), removed.bytes);
        Ok(removed.bytes + reclaimed)
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What deleting one optional download's files would free, for `code`.
/// Nothing is touched.
pub async fn optional_plan(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    name: String,
    remaining: Vec<String>,
) -> Result<lightforge_core::tact::reclaim::Reclaim, String> {
    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let install = layout.wow_install(&code);
    lightforge_core::tact::reclaim::plan_optional(&http, &install, &code, &name, &remaining)
        .await
        .map_err(|e| chain(&e))
}

/// Carry out a plan from [`optional_plan`], returning the bytes freed.
///
/// Behind the same gate as repair and uninstall: the files and the indices
/// being rewritten are ones a running client has open.
pub async fn optional_remove(
    layout: Layout,
    code: ProductCode,
    plan: lightforge_core::tact::reclaim::Reclaim,
) -> Result<u64, Problem> {
    let alive = layout.clone();
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&alive))
        .await
        .map_err(|e| Problem::of(&e))?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name).into());
    }
    let install = layout.wow_install(&code);
    tokio::task::spawn_blocking(move || {
        lightforge_core::tact::reclaim::execute(&install, &plan, |_| {})
            .map_err(|e| Problem::of(&e))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What freeing a store's unused game data would do: data no installed or
/// pre-installed version uses, plus dead space older passes left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unused {
    /// Bytes of disk it would free.
    pub freeable: u64,
    /// Bytes of data still in use it copies first.
    pub moved: u64,
}

/// Plan freeing the unused data in the store under `install`. Changes nothing.
pub async fn unused_plan(http: Option<Http>, install: PathBuf) -> Result<Unused, String> {
    use lightforge_core::tact::{compact, reclaim};
    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let prune = reclaim::plan_unused(&http, &install).await.map_err(|e| chain(&e))?;
    // Archives the prune already rewrites are counted once, by the prune.
    let rewritten: HashSet<u16> = prune.archives.iter().copied().collect();
    let (dead, _) =
        tokio::task::spawn_blocking(move || compact::plan_where(&install, |a| !rewritten.contains(&a)))
            .await
            .map_err(|e| chain(&e))?
            .map_err(|e| chain(&e))?;
    Ok(Unused { freeable: prune.freed + dead.freed, moved: prune.moved + dead.moved })
}

/// Free the unused data in the store under `install`: drop what no version
/// uses, then compact. Planned again here rather than trusted from when the
/// page opened. Returns the bytes freed; `copied` hears the copy's progress.
pub async fn free_unused(
    http: Option<Http>,
    layout: Layout,
    install: PathBuf,
    copied: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<u64, String> {
    use lightforge_core::tact::{compact, reclaim};
    use std::sync::atomic::Ordering;

    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&layout))
        .await
        .map_err(|e| chain(&e))?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name));
    }

    let prune = reclaim::plan_unused(&http, &install).await.map_err(|e| chain(&e))?;
    tokio::task::spawn_blocking(move || {
        let pruned = reclaim::execute(&install, &prune, |bytes| copied.store(bytes, Ordering::Relaxed))
            .map_err(|e| chain(&e))?;
        let base = prune.moved;
        let (dead, snapshot) = compact::plan(&install).map_err(|e| chain(&e))?;
        let compacted = compact::execute(&install, &dead, &snapshot, |bytes| {
            copied.store(base + bytes, Ordering::Relaxed)
        })
        .map_err(|e| chain(&e))?;
        Ok(pruned + compacted)
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Measure Settings > Storage. Walks the whole game install.
pub async fn storage(layout: Layout, dirs: AppDirs) -> lightforge_core::storage::Report {
    let fallback = lightforge_core::storage::Report {
        libraries: Vec::new(),
        app: Vec::new(),
        app_root: dirs.data_dir().to_path_buf(),
        app_disk: None,
    };
    tokio::task::spawn_blocking(move || lightforge_core::storage::report(&layout, &dirs))
        .await
        .unwrap_or(fallback)
}

/// Clear what one library holds that is only a cache; returns bytes freed.
pub async fn clear_library(
    layout: Layout,
    library: lightforge_core::library::Library,
) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::storage::clear_library(&layout, &library).map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Clear Lightforge's own cache, shader caches aside; returns bytes freed.
pub async fn clear_cache(dirs: AppDirs) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || lightforge_core::storage::clear_cache(&dirs))
        .await
        .map_err(|e| chain(&e))?
        .map_err(|e| chain(&e))
}

#[cfg(test)]
mod uninstall_tests {
    use super::*;
    use lightforge_core::tact::productdb;

    /// Uninstalling one version from a throwaway library built from copies of
    /// the real `.build.info` and Agent registry: only that version goes, and
    /// every other row and record stays. Skipped where there is no real
    /// install to copy from.
    #[test]
    fn uninstalling_one_version_leaves_the_rest() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
        let real = Layout::with_root(home.join("Games/Blizzard"));
        let (Ok(info), Ok(registry)) = (
            std::fs::read(real.build_info(&ProductCode::ClassicEra)),
            std::fs::read(real.agent_dir().join("product.db")),
        ) else {
            return;
        };

        let root = std::env::temp_dir()
            .join(format!("lightforge-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout::with_root(&root);
        let era = layout.product_dir(&ProductCode::ClassicEra);
        std::fs::create_dir_all(era.join("WTF")).unwrap();
        std::fs::write(era.join("WowClassic.exe"), [0u8; 64]).unwrap();
        std::fs::create_dir_all(layout.wow_install(&ProductCode::ClassicEra).join("Data/data")).unwrap();
        std::fs::write(layout.build_info(&ProductCode::ClassicEra), &info).unwrap();
        std::fs::create_dir_all(layout.agent_dir()).unwrap();
        std::fs::write(layout.agent_dir().join("product.db"), &registry).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        // No client: the reclaim is skipped, which is what this test isolates.
        runtime.block_on(uninstall(None, layout.clone(), ProductCode::ClassicEra, Default::default())).unwrap();

        assert!(!era.exists());
        assert!(
            layout.wow_install(&ProductCode::ClassicEra).join("Data").exists(),
            "the shared store stays",
        );
        let left = lightforge_core::buildinfo::BuildInfo::load(layout.build_info(&ProductCode::ClassicEra)).unwrap();
        let before = lightforge_core::buildinfo::BuildInfo::load(real.build_info(&ProductCode::ClassicEra)).unwrap();
        assert_eq!(left.products().count(), before.products().count() - 1);
        assert!(left.products().all(|p| p != ProductCode::ClassicEra));

        let after = std::fs::read(layout.agent_dir().join("product.db")).unwrap();
        let after = productdb::decode(&after);
        let had = productdb::decode(&registry);
        assert_eq!(after.len(), had.len() - 1);
        assert!(after.iter().all(|r| r.product != "wow_classic_era"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
