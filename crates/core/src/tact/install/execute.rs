//! Running a prepared plan into the store, and `.build.info` after it.

use std::path::Path;

use super::prepare::{prepare, Prepared};
use super::{Event, InstallError, Options, Summary};
use crate::buildinfo::BuildInfo;
use crate::net::Http;
use crate::product::ProductCode;
use crate::text::hex;
use crate::tact::download::{
    self, extract_install, Cancel, Catalog, Downloader, Options as DownloadOptions, Source,
};
use crate::tact::writer::StoreWriter;

/// Run a prepared plan: fetch, extract the install files, and finish the store.
pub async fn execute(
    http: &Http,
    dest: &Path,
    code: &ProductCode,
    options: &Options,
    prepared: Prepared,
    cancel: &Cancel,
    on_event: &mut impl FnMut(Event),
) -> Result<Summary, InstallError> {
    let Prepared {
        plan, remote, cdn, client, encoding, install, locations, loose, tags, tag_column, ..
    } = prepared;

    // Before a byte is fetched, not after. The run's very last act is to write
    // `.build.info` *into the install root*, and a root that will not take a
    // new file fails there, hours in, with every downloaded byte already on
    // disk and the update still reported as failed. Writing into `Data/data`
    // and the flavour directory proves nothing about the root: those are
    // separate directories with their own permissions, and an install carried
    // over from another tool or another user is exactly where they differ.
    match crate::paths::ensure_writable(dest) {
        Ok(crate::paths::Repaired::Yes) => on_event(Event::Repaired { dir: dest.to_path_buf() }),
        Ok(crate::paths::Repaired::No) => {}
        Err(source) => return Err(InstallError::Unwritable { dir: dest.to_path_buf(), source }),
    }

    let mut writer = StoreWriter::create(dest.join("Data").join("data"))?;
    // The reader always carries a key service; the downloader must too, or
    // blte cannot even name a missing key properly.
    let keys = download::default_key_service();
    let source = Source { http, client: &client, keys: Some(&keys) };
    let downloader = Downloader::new(DownloadOptions {
        concurrency: options.concurrency,
        limit: download::RateLimit::new(options.limit_bytes_per_second),
        checkpoint_bytes: options.checkpoint_bytes,
    });

    let outcome = downloader
        .run(source, &plan, &mut writer, cancel, |p| on_event(Event::Progress(p)))
        .await?;

    let mut install_complete = true;
    let mut install_failed: Vec<(String, String)> = Vec::new();
    if options.install_files {
        let selected = install.select(&tags);
        let catalog = Catalog { encoding: &encoding, locations: &locations, loose: &loose };
        let report = extract_install(
            source,
            &selected,
            catalog,
            &dest.join(&options.flavor),
            cancel,
            |e| on_event(Event::InstallFile { path: e.path.clone(), bytes: e.bytes }),
        )
        .await?;
        on_event(Event::Extracted { files: report.written.len(), bytes: report.bytes() });
        install_complete = report.is_complete();
        for (path, reason) in &report.failed {
            tracing::warn!("install file {path} failed: {reason}");
        }
        install_failed = report.failed.clone();

        // Agent writes this and the install manifest does not list it; without
        // it the client cannot tell which product it is.
        //
        // Only on a complete extraction, though: a `.flavor.info` sitting in a
        // directory with no game in it claims a product that is not installed.
        // A cancelled run used to leave exactly that behind, and Battle.net
        // scanning the install then sees a flavor it cannot account for.
        if install_complete {
            let flavor_dir = dest.join(&options.flavor);
            download::write_flavor_info(&flavor_dir, code)
                .map_err(|source| InstallError::Io { path: flavor_dir, source })?;
        }
    }

    log_failures(&outcome.failed);
    let mut reasons = Vec::new();
    if !outcome.failed.is_empty() {
        reasons.push(format!(
            "{} files failed (first: {})",
            outcome.failed.len(),
            outcome.failed[0].reason
        ));
    }
    if !plan.missing.is_empty() {
        reasons.push(format!("{} keys unlocatable", plan.missing.len()));
    }
    if outcome.cancelled {
        reasons.push("cancelled".to_owned());
    }
    if options.limited() {
        reasons.push("limit options truncated the plan".to_owned());
    }
    if options.install_files && !install_complete {
        reasons.push(match install_failed.first() {
            Some((path, reason)) => format!(
                "{} install files failed (first: {path}: {reason})",
                install_failed.len()
            ),
            None => "install files incomplete".to_owned(),
        });
    }
    let complete = reasons.is_empty();
    let incomplete_because = reasons.clone();

    let version = if complete {
        let info_path = dest.join(".build.info");
        // Only a missing file starts fresh. One that is there but will not
        // parse holds the other products' rows, and replacing it would drop
        // them from every launcher that reads it.
        let mut info = match BuildInfo::load(&info_path) {
            Ok(info) => info,
            Err(_) if !info_path.exists() => BuildInfo::with_default_schema(),
            Err(source) => return Err(InstallError::Metadata { what: ".build.info", source }),
        };
        info.set_product(
            code,
            &[
                ("Branch", options.region.as_str()),
                ("Active", "1"),
                ("Build Key", &remote.build_config),
                ("CDN Key", &remote.cdn_config),
                ("CDN Path", &cdn.path),
                ("CDN Hosts", &cdn.hosts.join(" ")),
                ("CDN Servers", &cdn.servers.join(" ")),
                ("Tags", &tag_column),
                ("Version", &remote.versions_name),
            ],
        )?;
        info.save(&info_path)
            .map_err(|source| InstallError::Metadata { what: ".build.info", source })?;
        on_event(Event::BuildInfo {
            path: info_path,
            version: remote.versions_name.clone(),
        });

        // Battle.net will not adopt an install without `.product.db`: its
        // Locate flow reads the file and answers error 2800 when it is absent.
        // Written from `.build.info`, so the two can never disagree.
        if let Some(windows_path) = &options.windows_path {
            let record = crate::tact::productdb::ProductInstall::from_build_info(
                &info,
                code,
                windows_path,
            )?
            .with_countries(&options.account_country, &options.geo_country);
            let path = crate::tact::productdb::upsert(dest, std::slice::from_ref(&record))?;
            on_event(Event::ProductDb { path });

            // And into Agent's own registry, which is the list the client
            // builds its library from. Without this the tile reads `Install`
            // until the user asks Battle.net to locate the game, it adopts
            // fine, but only after a scan nobody should have to trigger.
            //
            // Skipped silently when there is no registry to merge into, or
            // when it cannot be read back: see `productdb::register`.
            if let Some(agent_dir) = &options.agent_dir {
                if crate::tact::productdb::register(agent_dir, std::slice::from_ref(&record))? {
                    on_event(Event::Registered { code: code.as_str().to_owned() });
                }
            }
        }
        Some(remote.versions_name.clone())
    } else {
        on_event(Event::Incomplete { reasons });
        None
    };

    let store = writer.finish()?;
    Ok(Summary {
        outcome,
        store,
        missing: plan.missing.len(),
        complete,
        reasons: incomplete_because,
        version,
    })
}

/// Prepare and execute in one call, the ordinary path.
pub async fn run(
    http: &Http,
    dest: &Path,
    code: &ProductCode,
    options: &Options,
    cancel: &Cancel,
    mut on_event: impl FnMut(Event),
) -> Result<Summary, InstallError> {
    tracing::info!(
        "install starting: {} region={} locale={} optional={:?} into {} (limit {} B/s)",
        code.as_str(),
        options.region.as_str(),
        options.locale,
        options.optional_tags,
        dest.display(),
        options.limit_bytes_per_second,
    );
    let started = std::time::Instant::now();
    let mut heartbeat = Heartbeat::new();
    let mut on_event = move |event: Event| {
        log_event(&event, &mut heartbeat);
        on_event(event);
    };
    let result = async {
        let prepared = prepare(http, dest, code, options, &mut on_event).await?;
        execute(http, dest, code, options, prepared, cancel, &mut on_event).await
    }
    .await;
    match &result {
        Ok(s) => tracing::info!(
            "install ended after {:.0}s: complete={} stored={} files / {} B, failed={}, cancelled={}, unlocatable={}",
            started.elapsed().as_secs_f64(),
            s.complete,
            s.outcome.files_stored,
            s.outcome.bytes_stored,
            s.outcome.failed.len(),
            s.outcome.cancelled,
            s.missing,
        ),
        Err(e) => tracing::error!(
            "install failed after {:.0}s: {}",
            started.elapsed().as_secs_f64(),
            crate::error::chain(e)
        ),
    }
    result
}

/// How often a running transfer writes a progress line.
const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(30);

/// Rate-limits the progress line, so a multi-hour download leaves a few
/// hundred lines of "still going, this far" and not one per file.
struct Heartbeat {
    last: std::time::Instant,
    bytes: u64,
}

impl Heartbeat {
    fn new() -> Self {
        Self { last: std::time::Instant::now(), bytes: 0 }
    }
}

/// Write an event to the log. Per-file events are left out: a retail install
/// has a million of them.
fn log_event(event: &Event, heartbeat: &mut Heartbeat) {
    match event {
        Event::Progress(p) => {
            let since = heartbeat.last.elapsed();
            if since < HEARTBEAT {
                return;
            }
            let rate = p.bytes_stored.saturating_sub(heartbeat.bytes) as f64 / since.as_secs_f64();
            tracing::info!(
                "progress: {}/{} B, {}/{} files, {}/{} requests, {:.1} MiB/s",
                p.bytes_stored,
                p.bytes_total,
                p.files_stored,
                p.files_total,
                p.requests_done,
                p.requests_total,
                rate / (1024.0 * 1024.0),
            );
            *heartbeat = Heartbeat { last: std::time::Instant::now(), bytes: p.bytes_stored };
        }
        Event::InstallFile { .. } => {}
        Event::Incomplete { reasons } => tracing::warn!("install incomplete: {}", reasons.join(", ")),
        Event::Repaired { dir } => {
            tracing::warn!("{} would not take a file; added owner write", dir.display());
        }
        other => tracing::info!("{other:?}"),
    }
}

/// Write what a run could not fetch. The first few are named with their
/// cause, the rest only counted: a dead mirror fails thousands of files for
/// the same reason, and the reason is what matters.
fn log_failures(failed: &[download::Failure]) {
    const NAMED: usize = 20;
    for failure in failed.iter().take(NAMED) {
        tracing::warn!("failed {}: {}", hex(&failure.ekey), failure.reason);
    }
    if failed.len() > NAMED {
        tracing::warn!("... and {} more failed files", failed.len() - NAMED);
    }
}
