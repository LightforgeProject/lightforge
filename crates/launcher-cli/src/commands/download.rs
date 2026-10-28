//! Planning and downloading builds from the CDN.

use crate::*;

// One parameter per command-line flag.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download(dest: PathBuf, product: String, region: String, locale: String, flavor: String, dry_run: bool, optional: Vec<String>, limit_archives: Option<usize>, limit_runs: Option<usize>, skip_loose: bool, install_files: bool, concurrency: usize, gap_kib: u32, checkpoint_mib: u64, cancel_after: Option<u64>, show_install: bool) -> Result<()> {
    use lightforge_core::net::Http;
    use lightforge_core::tact::download::Cancel;
    use lightforge_core::tact::install::{self, Event, Options as InstallOptions};
    use lightforge_core::tact::version::Region;
    use std::time::Instant;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let region: Region = region.parse().context("parsing --region")?;
    let gib = |b: u64| b as f64 / 1024f64.powi(3);

    let options = InstallOptions {
        region,
        locale,
        flavor,
        concurrency,
        optional_tags: optional,
        gap_bytes: gap_kib * 1024,
        checkpoint_bytes: checkpoint_mib * 1024 * 1024,
        install_files,
        limit_archives,
        limit_runs,
        skip_loose,
        ..InstallOptions::default()
    };

    // Event printing, shared by the dry run and the real one. The
    // index prelude and the transfer are both far too chatty to print
    // per event, so both are throttled here rather than in core.
    let start = Instant::now();
    let mut last_index = 0usize;
    let mut last_report = Instant::now();
    let mut on_event = move |event: Event| match event {
        Event::Build { versions_name, build_config, archives } => {
            println!("build           {versions_name} ({build_config})");
            println!("archives        {archives}");
        }
        Event::Repaired { dir } => {
            println!("repaired        added owner write to {}", dir.display())
        }
        Event::Encoding { entries } => println!("encoding        {entries} entries"),
        Event::Manifests { download, install, unknown_tags } => {
            println!("download        {download} entries");
            println!("install         {install} entries");
            println!("tags            {unknown_tags:?}");
        }
        Event::Wanted { files, bytes } => {
            println!("wanted          {files} files, {:.1} GiB", gib(bytes))
        }
        Event::Indices(step) => {
            if step.done - last_index >= 100 || step.done == step.total {
                last_index = step.done;
                println!("  indices {}/{}", step.done, step.total);
            }
        }
        Event::IndicesDone { cached, downloaded, bytes } => println!(
            "indices         {cached} cached, {downloaded} downloaded ({:.1} MiB)",
            bytes as f64 / 1048576.0
        ),
        Event::Optional { names } => println!(
            "optional        {}",
            match names.is_empty() {
                true => "none offered".to_owned(),
                false => names.join(", "),
            }
        ),
        Event::Loose { files } => println!("loose files     {files}"),
        Event::Located { located, wanted } => {
            println!("located         {located} of {wanted}")
        }
        Event::Planned { requests, files, bytes, padding, already_present, missing } => {
            println!(
                "\nplan            {requests} requests, {files} files, {:.2} GiB ({:.2} GiB padding)",
                gib(bytes),
                gib(padding)
            );
            println!("already present {already_present}");
            if missing > 0 {
                println!("UNLOCATABLE     {missing} keys");
            }
        }
        Event::Progress(p) => {
            if last_report.elapsed().as_secs() >= 2 || p.requests_done == p.requests_total {
                last_report = Instant::now();
                let mb = p.bytes_stored as f64 / 1e6;
                println!(
                    "  {:>5.1}%  {}/{} requests  {} files  {:.0} MB  {:.1} MB/s",
                    p.fraction() * 100.0,
                    p.requests_done,
                    p.requests_total,
                    p.files_stored,
                    mb,
                    mb / start.elapsed().as_secs_f64().max(0.001)
                );
            }
        }
        Event::InstallFile { path, bytes } => {
            println!("  {} ({:.1} MiB)", path.display(), bytes as f64 / 1048576.0)
        }
        Event::Extracted { files, bytes } => println!(
            "extracted       {files} files, {:.1} MiB",
            bytes as f64 / 1048576.0
        ),
        Event::BuildInfo { path, version } => {
            println!("\n.build.info     {} ({version})", path.display())
        }
        Event::ProductDb { path } => {
            println!(".product.db     {}", path.display())
        }
        Event::Registered { code } => {
            println!("registered      {code} in Agent's product.db")
        }
        Event::Incomplete { reasons } => {
            println!("\n.build.info     NOT written — this run is not a complete install");
            for reason in reasons {
                println!("                ({reason})");
            }
        }
    };

    let http = Http::new()?;
    let cancel = Cancel::new();

    if let Some(secs) = cancel_after {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            println!("\n--- cancelling at {secs}s ---");
            cancel.cancel();
        });
    }

    if dry_run {
        let prepared = install::prepare(&http, &dest, &code, &options, &mut on_event).await?;
        if show_install {
            let selected = prepared.install.select(&prepared.tags);
            println!("\ninstall files   {} selected of {}", selected.len(), prepared.install.len());
            for entry in &selected {
                println!("  {}", entry.path);
            }
        }
        println!("\n--dry-run: nothing fetched");
        return Ok(());
    }

    let t = Instant::now();
    let summary = install::run(&http, &dest, &code, &options, &cancel, on_event).await?;

    println!(
        "\nstored          {} files, {:.2} GiB in {:.1?}",
        summary.outcome.files_stored,
        gib(summary.outcome.bytes_stored),
        t.elapsed()
    );
    println!(
        "unverified      {} (encrypted or not in encoding)",
        summary.outcome.unverified
    );
    for failure in summary.outcome.failed.iter().take(10) {
        eprintln!(
            "  FAILED {} — {}",
            lightforge_core::text::hex(&failure.ekey),
            failure.reason
        );
    }
    if summary.outcome.failed.len() > 10 {
        eprintln!("  ... and {} more", summary.outcome.failed.len() - 10);
    }

    println!(
        "\nstore           {} archives, {} files, {:.2} GiB",
        summary.store.archives,
        summary.store.files,
        gib(summary.store.bytes)
    );
    println!("indices         {} written", summary.store.indices.len());
    match summary.complete {
        true => println!("\nverify with:  lightforge casc {}", dest.display()),
        false => println!("\nre-run to continue; what is stored is kept"),
    }
    Ok(())
}

pub(crate) async fn cdn_archive(archive: Option<String>, product: String, region: String, fetch: usize, verify_with: Option<PathBuf>) -> Result<()> {
    use lightforge_core::net::Http;
    use lightforge_core::tact::archive::ArchiveIndex;
    use lightforge_core::tact::cdn::{Client, Kind};
    use lightforge_core::tact::config::{BuildConfig, CdnConfig};
    use lightforge_core::text::hex;
    use lightforge_core::tact::version::{self, Region};

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let region: Region = region.parse().context("parsing --region")?;

    let http = Http::new()?;
    let versions = version::versions(&http, region, code.as_str()).await?;
    let remote = version::for_region(&versions, region, |v| &v.region)
        .context("the version table is empty")?;
    let cdns = version::cdns(&http, region, code.as_str()).await?;
    let cdn = version::for_region(&cdns, region, |c| &c.name)
        .context("the CDN table is empty")?;
    let client = Client::from_cdn(cdn)?;

    let cdn_config = CdnConfig::parse(
        &client.fetch_config(&http, &remote.cdn_config).await
            .context("fetching the CDN config")?,
    )?;
    println!("archives        {}", cdn_config.archives.len());

    let archive = match archive {
        Some(a) => a,
        None => cdn_config.archives.first().cloned().context("no archives listed")?,
    };

    // The index lives beside the archive, at the same hash plus a
    // suffix, and is not itself content-addressed by that name.
    let endpoint = &client.endpoints()[0];
    let url = format!("{}.index", endpoint.url(Kind::Data, &archive)?);
    let raw = http
        .get_bytes(&url, lightforge_core::net::MAX_CDN_BYTES)
        .await
        .context("fetching the archive index")?;
    println!("index           {} bytes from {url}", raw.len());

    let index = ArchiveIndex::parse(&raw, &archive).context("parsing the archive index")?;
    println!("entries         {}", index.len());
    println!("archived        {:.1} MB", index.bytes() as f64 / 1e6);

    // Range-request real files and verify each against its CONTENT
    // key, the encoding key names the bytes but cannot check them.
    let Some(install) = verify_with else {
        println!("\npass --verify-with <install> to fetch and verify entries");
        return Ok(());
    };

    use lightforge_core::tact::content::{Content, Locale};
    use lightforge_core::tact::storage::Storage;
    let local_build = BuildConfig::load(
        install.join("Data").join("config"),
        &remote.build_config,
    )
    .context("loading the build config from the local install")?;
    let mut storage = Storage::open(&install).context("opening the local store")?;
    let content = Content::open(&mut storage, local_build, Locale::Any)
        .context("loading the local manifests")?;

    let (mut ok, mut bad, mut unknown) = (0usize, 0usize, 0usize);
    for located in index.entries().iter().take(fetch) {
        let Some(ckey) = content.ckey_for_ekey(&located.ekey) else {
            unknown += 1;
            continue;
        };
        match client
            .fetch_from_archive(&http, &archive, located, &ckey, None)
            .await
        {
            Ok(decoded) => {
                ok += 1;
                println!(
                    "  {} -> ckey {}  {:>9} bytes decoded  verified",
                    hex(&located.ekey),
                    hex(&ckey),
                    decoded.len()
                );
            }
            Err(e) => {
                bad += 1;
                println!("  {} FAILED: {e}", hex(&located.ekey));
            }
        }
    }
    println!("\nverified {ok}, failed {bad}, not in local encoding {unknown}");
    Ok(())
}

pub(crate) async fn cdn_fetch(product: String, region: String, hash: Option<String>, kind: String, compare: Option<PathBuf>) -> Result<()> {
    use lightforge_core::net::Http;
    use lightforge_core::tact::cdn::{Client, Kind};
    use lightforge_core::tact::version::{self, Region};

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let region: Region = region.parse().context("parsing --region")?;
    let kind = match kind.as_str() {
        "config" => Kind::Config,
        "data" => Kind::Data,
        "patch" => Kind::Patch,
        other => anyhow::bail!("unknown --kind `{other}` (config, data or patch)"),
    };

    let http = Http::new()?;
    let versions = version::versions(&http, region, code.as_str()).await?;
    let remote = version::for_region(&versions, region, |v| &v.region)
        .context("the version table is empty")?;
    let cdns = version::cdns(&http, region, code.as_str()).await?;
    let cdn = version::for_region(&cdns, region, |c| &c.name)
        .context("the CDN table is empty")?;

    let client = Client::from_cdn(cdn)?;
    for endpoint in client.endpoints() {
        println!("endpoint        {}/{}", endpoint.origin, endpoint.path);
    }

    let hash = hash.unwrap_or_else(|| remote.build_config.clone());
    println!("fetching        {hash} ({kind:?})");

    let t = std::time::Instant::now();
    let bytes = client.fetch_md5_verified(&http, kind, &hash).await
        .context("fetching from the CDN")?;
    let elapsed = t.elapsed();
    println!(
        "verified        {} bytes in {:.2}s ({:.1} MB/s)",
        bytes.len(),
        elapsed.as_secs_f64(),
        bytes.len() as f64 / 1e6 / elapsed.as_secs_f64().max(0.001),
    );

    if let Some(local) = compare {
        let want = std::fs::read(&local)
            .with_context(|| format!("reading {}", local.display()))?;
        println!(
            "vs {}  {}",
            local.display(),
            if want == bytes { "identical" } else { "DIFFERS" }
        );
    }
    Ok(())
}

// One parameter per command-line flag.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn plan(dest: PathBuf, product: String, region: String, locale: String, flavor: String, explain: bool, optional: Vec<String>, limit_archives: Option<usize>, concurrency: usize) -> Result<()> {
    use lightforge_core::net::Http;
    use lightforge_core::tact::explain as tact_explain;
    use lightforge_core::tact::storage::Storage;
    use lightforge_core::tact::install::{self, Event, Options as InstallOptions};
    use lightforge_core::tact::version::Region;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let region: Region = region.parse().context("parsing --region")?;
    let gib = |b: u64| b as f64 / 1024f64.powi(3);

    let options = InstallOptions {
        region,
        locale,
        flavor,
        concurrency,
        limit_archives,
        optional_tags: optional,
        ..InstallOptions::default()
    };

    // Quiet by design: the report is the output, and the index
    // prelude would bury it.
    let mut last_index = 0usize;
    let mut on_event = move |event: Event| match event {
        Event::Build { versions_name, build_config, .. } => {
            println!("build           {versions_name} ({build_config})");
        }
        Event::Indices(step)
            if step.done - last_index >= 400 || step.done == step.total =>
        {
            last_index = step.done;
            eprintln!("  indices {}/{}", step.done, step.total);
        }
        _ => {}
    };

    let http = Http::new().context("building the HTTP client")?;
    let prepared =
        install::prepare(&http, &dest, &code, &options, &mut on_event).await?;

    println!(
        "optional        {}",
        match prepared.optional.is_empty() {
            true => "none offered".to_owned(),
            false => prepared.optional.join(", "),
        }
    );
    let wanted_bytes: u64 = prepared.wanted.iter().map(|e| e.size).sum();
    println!("wanted          {} files, {:.2} GiB", prepared.wanted.len(), gib(wanted_bytes));
    println!(
        "already present {} files",
        prepared.plan.already_present
    );
    println!(
        "plan            {} requests, {} files, {:.2} GiB",
        prepared.plan.requests(),
        prepared.plan.files(),
        gib(prepared.plan.bytes())
    );

    if !explain {
        println!("\nPass --explain for the breakdown.");
        return Ok(());
    }

    let store = Storage::open(&dest).ok();
    if !prepared.probe.opened() {
        println!(
            "\nstore           WOULD NOT OPEN at {} — every key counts as absent",
            dest.display()
        );
    }
    let report = tact_explain::explain(
        &prepared.wanted,
        store.as_ref(),
        &prepared.locations,
        &prepared.loose,
        |k| prepared.probe.has(k),
    );

    println!("\nby priority (lower is fetched sooner)");
    println!("{:>8}  {:>12} {:>9}  {:>12} {:>9}", "priority", "present", "GiB", "missing", "GiB");
    for (priority, split) in &report.by_priority {
        println!(
            "{priority:>8}  {:>12} {:>9.2}  {:>12} {:>9.2}",
            split.present.files,
            gib(split.present.bytes),
            split.missing.files,
            gib(split.missing.bytes),
        );
    }

    println!("\nby bucket (missing keys against what the store lists)");
    println!("{:>6}  {:>12}  {:>12}", "bucket", "in store", "missing");
    for bucket in 0u8..16 {
        let held = report.store_by_bucket.get(&bucket).copied().unwrap_or(0);
        let missing = report.missing_by_bucket.get(&bucket).copied().unwrap_or(0);
        let flag = match held {
            0 => "   <- no index loaded",
            _ => "",
        };
        println!("{bucket:>6}  {held:>12}  {missing:>12}{flag}");
    }

    println!("\nmissing keys by source");
    println!("  in an archive   {}", report.sources.archived);
    println!("  loose           {}", report.sources.loose);
    println!("  nowhere         {}  (nothing can fetch these)", report.sources.nowhere);

    let by_tag = tact_explain::by_tag(
        &prepared.download,
        &prepared.tags,
        |k| prepared.probe.has(k),
    );
    println!("\nby tag (what the selection is made of)");
    println!(
        "{:<16} {:>4} {:>6}  {:>12} {:>9}  {:>12} {:>9}  {:>12}",
        "tag", "kind", "named", "selected", "GiB", "missing", "GiB", "covers"
    );
    // Normally only the tags doing work are worth printing. When
    // *nothing* is selected the interesting question is the opposite
    // one, what vocabulary does this manifest even have, and that
    // is exactly the case where the filtered table is empty and says
    // nothing at all.
    let selects_nothing = by_tag.rows.iter().all(|r| r.selected.files == 0);
    for row in by_tag.rows.iter().filter(|r| selects_nothing || r.selected.files > 0) {
        println!(
            "{:<16} {:>4} {:>6}  {:>12} {:>9.2}  {:>12} {:>9.2}  {:>12}",
            row.name,
            row.kind,
            match row.constrained {
                true => "yes",
                false => "NO",
            },
            row.selected.files,
            gib(row.selected.bytes),
            row.missing.files,
            gib(row.missing.bytes),
            row.covered.files,
        );
    }
    match by_tag.unconstrained_kinds.as_slice() {
        [] => println!("  every tag category the manifest defines is constrained"),
        kinds => println!(
            "  UNCONSTRAINED categories: {kinds:?} — every value of these is selected"
        ),
    }

    println!("\nprobe cross-check");
    match report.false_missing {
        0 => println!("  the store agrees: every planned file really is absent"),
        n => {
            println!("  {n} planned files ARE in the store — the probe is wrong");
            for ekey in &report.examples {
                println!("    {}", lightforge_core::text::hex(ekey));
            }
        }
    }
    Ok(())
}
