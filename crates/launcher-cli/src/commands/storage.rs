//! Checking, repairing and reclaiming the store.

use super::{default_install, windows_wow_path};
use crate::*;

pub(crate) fn verify(ctx: Ctx, path: Option<PathBuf>, deep: bool, repair: bool, all: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::tact::storage::Storage;
    use lightforge_core::tact::verify;
    use std::time::{Duration, Instant};
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let install = path.unwrap_or_else(|| default_install(&layout));
    let data_dir = install.join("Data").join("data");

    // Repair rewrites the indices a running client memory-maps. The
    // scan itself only reads, so it is allowed either way.
    if repair {
        let alive = lightforge_core::preflight::scan(&layout);
        if !alive.is_empty() {
            for process in &alive {
                println!("running         {} (pid {})", process.name, process.pid);
            }
            anyhow::bail!("close the game and Battle.net first — the store is in use");
        }
    }

    let store = Storage::open(install.join("Data")).context("opening the store")?;
    let stats = store.stats();
    println!("store           {}", data_dir.display());
    println!("indexed         {} files across {} buckets", stats.files, stats.buckets);

    let started = Instant::now();
    let mut last = Instant::now();
    let report = match deep {
        false => verify::scan(&store, |p| {
            if last.elapsed() >= Duration::from_millis(500) {
                last = Instant::now();
                eprintln!("  {}/{} checked, {} damaged", p.checked, p.total, p.damaged);
            }
        }),
        true => {
            // The deep scan opens the store itself, after snapshotting
            // it, so a write between the two is caught.
            drop(store);
            let cancel = lightforge_core::tact::download::Cancel::new();
            verify::deep_scan(&install, &cancel, |p| {
                if last.elapsed() >= Duration::from_secs(2) {
                    last = Instant::now();
                    eprintln!(
                        "  {:.1}/{:.1} GiB hashed, {} damaged",
                        p.bytes as f64 / GIB,
                        p.bytes_total as f64 / GIB,
                        p.damaged
                    );
                }
            })?
        }
    };

    println!(
        "checked         {} entries in {:.1}s",
        report.checked,
        started.elapsed().as_secs_f64()
    );
    println!(
        "read            {} headers, {:.1} MiB",
        report.headers_read,
        report.bytes_read as f64 / (1024.0 * 1024.0)
    );
    if report.unverifiable > 0 {
        println!(
            "unverifiable    {} raw patch manifests — kept, nothing to check them against",
            report.unverifiable
        );
    }
    if !report.missing_archives.is_empty() {
        println!("missing         {} archive files", report.missing_archives.len());
    }

    if report.is_healthy() {
        println!("\nhealthy — every entry is where its index says, and is what it claims");
        if deep {
            println!("          and every stored byte matches its checksum");
        }
        return Ok(());
    }

    println!("\ndamaged         {} entries", report.damaged.len());
    let mut by_fault: std::collections::BTreeMap<&str, usize> = Default::default();
    for damage in &report.damaged {
        *by_fault.entry(damage.fault.describe()).or_default() += 1;
    }
    for (fault, count) in &by_fault {
        println!("  {count:>8}  {fault}");
    }

    let show = match all {
        true => report.damaged.len(),
        false => 10.min(report.damaged.len()),
    };
    println!();
    for damage in &report.damaged[..show] {
        let key = lightforge_core::text::hex(&damage.entry.key);
        println!(
            "  {key}  data.{:03} +{}  {}",
            damage.entry.archive_id, damage.entry.offset, damage.fault.describe()
        );
    }
    if show < report.damaged.len() {
        println!("  … and {} more (--all to list them)", report.damaged.len() - show);
    }

    match repair {
        false => println!(
            "\nRun again with --repair to drop these from the indices; the next \
             download refetches them."
        ),
        true => {
            // Again, not only at the start: a deep scan runs for
            // minutes, and the game may have been started meanwhile.
            let alive = lightforge_core::preflight::scan(&layout);
            if !alive.is_empty() {
                for process in &alive {
                    println!("running         {} (pid {})", process.name, process.pid);
                }
                anyhow::bail!("close the game and Battle.net first — the store is in use");
            }
            let removed = verify::repair_unchanged(&data_dir, &report)?;
            println!("\nremoved         {removed} entries from the indices");
            println!("Run a download for this product to fetch them again.");
        }
    }
    Ok(())
}

pub(crate) fn reindex(ctx: Ctx, path: Option<PathBuf>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::tact::writer::StoreWriter;

    // Never rewrite indices under a live client: it memory-maps them,
    // and this is the exact overlap the whole pre-flight design exists
    // to prevent.
    let alive = lightforge_core::preflight::scan(&layout);
    if !alive.is_empty() {
        for process in &alive {
            println!("running         {} (pid {})", process.name, process.pid);
        }
        anyhow::bail!("close the game and Battle.net first — the store is in use");
    }

    let install = path.unwrap_or_else(|| default_install(&layout));
    let data_dir = install.join("Data").join("data");

    let before: Vec<(String, u64)> = std::fs::read_dir(&data_dir)?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "idx"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().map(|m| m.len()).unwrap_or(0),
            )
        })
        .collect();
    println!("store           {}", data_dir.display());
    println!("indices         {} before", before.len());

    // Loading and finishing rewrites every bucket as a new generation,
    // with the current sizing rules, and prunes the old ones. No
    // archive is opened, so not a byte of game data is touched.
    let summary = StoreWriter::create(&data_dir)?.finish()?;
    println!("rewrote         {} indices", summary.indices.len());
    for path in &summary.indices {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("  {}  {size} bytes", path.file_name().expect("index paths end in a file name").to_string_lossy());
    }
    Ok(())
}

pub(crate) async fn move_game(ctx: Ctx, product: String, to: String, yes: bool) -> Result<()> {
    use lightforge_core::migrate::{self, Moved};
    use lightforge_core::net::Http;
    use lightforge_core::tact::download::Cancel;

    let Ctx { layout, mut config } = ctx;
    let code: ProductCode = product.parse().expect("product codes are infallible");
    let target = config
        .libraries
        .iter()
        .find(|l| l.id == to || l.label == to)
        .cloned()
        .with_context(|| format!("no library named {to}; `lightforge paths` lists them"))?;
    let source = layout.library(&code).cloned().context("the product has no library")?;
    println!("product         {}", code.as_str());
    println!("from            {} ({})", source.label, layout.wow_install(&code).display());
    println!("to              {} ({})", target.label, layout.wow_install_in(&target).display());
    if !yes {
        println!("\nnothing moved; add --yes to move it");
        return Ok(());
    }

    let http = Http::new()?;
    let mut shown = u64::MAX;
    let moved = migrate::copy(&http, &layout, &code, &target, &Cancel::new(), |p| {
        let percent = p.bytes_done * 100 / p.bytes_total.max(1);
        if percent / 5 != shown {
            shown = percent / 5;
            println!("{:<15} {percent}%", if p.verifying { "verifying" } else { "copying" });
        }
    })
    .await?;
    match &moved {
        Moved::Renamed => println!("moved           the install folder, renamed"),
        Moved::Copied { files, bytes } => println!("copied          {files} files, {bytes} bytes, all read back"),
    }

    // The new library is recorded before the old copy goes.
    config
        .product_mut(&code)
        .context("the product is not in the config")?
        .profile
        .library = Some(target.id.clone());
    config.save(&AppDirs::discover()?.config_file())?;

    let freed = migrate::release(&http, &config.layout()?, &code, &source).await?;
    println!("freed           {freed} bytes in {}", source.label);
    Ok(())
}

pub(crate) fn uninstall(ctx: Ctx, product: String, path: Option<PathBuf>, yes: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::tact::uninstall;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let install = path.unwrap_or_else(|| layout.wow_install(&code));
    let plan = uninstall::plan(&install, &code)?;

    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    println!("install         {}", install.display());
    println!("product         {}", code.as_str());

    if plan.is_empty() {
        println!("\nnothing to remove — that product is not installed here");
        return Ok(());
    }

    println!("\nwould delete:");
    for dir in plan.flavor_dir.iter().chain(plan.store.iter()) {
        println!("  {}", dir.display());
    }
    if plan.has_row {
        println!(
            "  .build.info {}",
            match plan.remove_build_info {
                true => "(the whole file — no products left)",
                false => "(this product's row only)",
            }
        );
    }
    println!("\nfrees           {:.2} GiB", gib(plan.bytes));
    match plan.remaining.is_empty() {
        true => println!("remaining       nothing — this removes the whole install"),
        false => println!(
            "remaining       {} (the shared CASC store is kept for them)",
            plan.remaining.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }

    if !yes {
        println!("\nnothing deleted. Re-run with --yes to go ahead.");
        return Ok(());
    }

    // `--path` can name another library than the product's own, as after a
    // move that left a copy behind; the products left there keep its path.
    let windows_path = layout
        .libraries()
        .all()
        .iter()
        .find(|library| layout.wow_install_in(library) == install)
        .map(|library| library.windows_wow_path())
        .unwrap_or_else(|| windows_wow_path(&layout, &code));
    let removed = uninstall::execute(&install, &code, &plan, &windows_path)?;
    println!("\ndeleted         {} paths, {:.2} GiB", removed.paths.len(), gib(removed.bytes));
    Ok(())
}

pub(crate) async fn reclaim(ctx: Ctx, product: String, build_key: Option<String>, region: String, path: Option<PathBuf>, yes: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::buildinfo::BuildInfo;
    use lightforge_core::net::Http;
    use lightforge_core::tact::version::{self, Region};
    use lightforge_core::tact::reclaim;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let install = path.unwrap_or_else(|| layout.wow_install(&code));
    let http = Http::new()?;
    let departing = match build_key {
        Some(build_key) => {
            let region: Region = region.parse().context("parsing --region")?;
            let cdns = version::cdns(&http, region, code.as_str()).await?;
            let cdn = version::for_region(&cdns, region, |c| &c.name)
                .context("the CDN table is empty")?
                .clone();
            reclaim::Build { product: code.clone(), build_key, cdn }
        }
        None => {
            let info = BuildInfo::load(install.join(".build.info"))
                .with_context(|| format!("reading {}/.build.info", install.display()))?;
            reclaim::Build::installed(&info, &code)?
        }
    };

    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    let plan = reclaim::plan(&http, &install, &departing).await?;
    println!("install         {}", install.display());
    println!("departing       {} ({})", code.as_str(), departing.build_key);
    println!("entries         {} used by nothing else installed", plan.entries.len());
    println!("archives        {} compacted or deleted", plan.archives.len());
    println!("copies          {:.2} GiB other versions still use", gib(plan.moved));
    println!("frees           {:.2} GiB", gib(plan.freed));
    println!(
        "stranded        {:.2} GiB left as dead space in shared archives",
        gib(plan.stranded)
    );

    if !yes || plan.entries.is_empty() {
        if !plan.entries.is_empty() {
            println!("\nnothing changed. Re-run with --yes to go ahead.");
        }
        return Ok(());
    }
    if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
        anyhow::bail!("close {} first — the store is in use", process.name);
    }
    let freed = reclaim::execute(&install, &plan, copy_progress(plan.moved))?;
    println!("\nfreed           {:.2} GiB", gib(freed));
    Ok(())
}

pub(crate) async fn prune(ctx: Ctx, path: Option<PathBuf>, yes: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::net::Http;
    use lightforge_core::tact::reclaim;

    let install = path.unwrap_or_else(|| default_install(&layout));
    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    let http = Http::new()?;
    let builds = reclaim::builds(&install)?;
    let plan = reclaim::plan_unused(&http, &install).await?;
    let unused: u64 = plan.entries.iter().map(|e| u64::from(e.size)).sum();
    println!("install         {}", install.display());
    println!(
        "keeps           {}",
        builds.iter().map(|b| b.product.as_str()).collect::<Vec<_>>().join(", ")
    );
    println!("unused          {} files, {:.2} GiB", plan.entries.len(), gib(unused));
    println!("archives        {} compacted or deleted", plan.archives.len());
    println!("copies          {:.2} GiB still in use", gib(plan.moved));
    println!("frees           {:.2} GiB", gib(plan.freed));
    println!("stranded        {:.2} GiB left as dead space", gib(plan.stranded));

    if !yes || plan.entries.is_empty() {
        if !plan.entries.is_empty() {
            println!("\nnothing changed. Re-run with --yes to go ahead.");
        }
        return Ok(());
    }
    if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
        anyhow::bail!("close {} first — the store is in use", process.name);
    }
    let freed = reclaim::execute(&install, &plan, copy_progress(plan.moved))?;
    println!("\nfreed           {:.2} GiB", gib(freed));
    Ok(())
}

pub(crate) async fn drop_optional(
    ctx: Ctx,
    product: String,
    tag: String,
    keep: Vec<String>,
    path: Option<PathBuf>,
    yes: bool,
) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::net::Http;
    use lightforge_core::tact::reclaim;

    let install = path.unwrap_or_else(|| default_install(&layout));
    let product: lightforge_core::product::ProductCode = product.parse()?;
    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    let http = Http::new()?;
    let plan = reclaim::plan_optional(&http, &install, &product, &tag, &keep).await?;
    let files: u64 = plan.entries.iter().map(|e| u64::from(e.size)).sum();
    println!("install         {}", install.display());
    println!("option          {tag} of {product}");
    println!("files           {} files, {:.2} GiB", plan.entries.len(), gib(files));
    println!("archives        {} compacted or deleted", plan.archives.len());
    println!("copies          {:.2} GiB still in use", gib(plan.moved));
    println!("frees           {:.2} GiB", gib(plan.freed));
    println!("stranded        {:.2} GiB left as dead space", gib(plan.stranded));

    if !yes || plan.entries.is_empty() {
        if !plan.entries.is_empty() {
            println!("\nnothing changed. Re-run with --yes to go ahead.");
        }
        return Ok(());
    }
    if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
        anyhow::bail!("close {} first — the store is in use", process.name);
    }
    let freed = reclaim::execute(&install, &plan, copy_progress(plan.moved))?;
    println!("\nfreed           {:.2} GiB", gib(freed));
    Ok(())
}

pub(crate) fn compact(ctx: Ctx, path: Option<PathBuf>, yes: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::tact::compact;

    let install = path.unwrap_or_else(|| default_install(&layout));
    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    let (plan, snapshot) = compact::plan(&install)?;
    println!("install         {}", install.display());
    println!("archives        {} to rewrite or delete", plan.archives.len());
    println!("copies          {:.2} GiB", gib(plan.moved));
    println!("frees           {:.2} GiB", gib(plan.freed));
    println!("left            {:.2} GiB of dead space not worth rewriting", gib(plan.left));

    if !yes || plan.archives.is_empty() {
        if !plan.archives.is_empty() {
            println!("\nnothing changed. Re-run with --yes to go ahead.");
        }
        return Ok(());
    }
    if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
        anyhow::bail!("close {} first — the store is in use", process.name);
    }
    let freed = compact::execute(&install, &plan, &snapshot, copy_progress(plan.moved))?;
    println!("\nfreed           {:.2} GiB", gib(freed));
    Ok(())
}

/// A progress callback for a compaction copying `total` bytes: one line per
/// whole percent, overwritten in place.
fn copy_progress(total: u64) -> impl FnMut(u64) {
    let mut shown = None;
    move |copied| {
        let percent = (copied * 100).checked_div(total).unwrap_or(100).min(100);
        if shown != Some(percent) {
            shown = Some(percent);
            eprint!("\rcopying         {percent:3}%");
            if percent == 100 {
                eprintln!();
            }
        }
    }
}
