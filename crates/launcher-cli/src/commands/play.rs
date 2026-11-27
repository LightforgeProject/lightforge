//! Starting a game, and the pre-flight in front of it.

use super::{beside_gui, owe_gui_client, print_preflight, proton_runner, warn_on_downgrade, windows_wow_path};
use crate::*;

pub(crate) async fn play(ctx: Ctx, code: ProductCode, proton: Option<String>, dry_run: bool) -> Result<()> {
    use lightforge_core::backup;
    use lightforge_core::launch::{DirectLaunch, LaunchError};
    use lightforge_core::preflight::{NoDownloads, Preflight};

    let Ctx { layout, config } = ctx;
    let config = beside_gui(config);
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    // Before the plan: it picks the build the update leaves behind.
    if config.update_on_play && !dry_run {
        update_game(&config, &layout, &code).await?;
        if config.addon_manager {
            super::addons::addon_pass_before_play(&config, &layout, &dirs, &code).await;
        }
    }
    let launch = match DirectLaunch::plan(&config, &layout, &dirs, &code, proton.as_deref()) {
        Err(e @ LaunchError::NoProton) => anyhow::bail!("{e}: run `lightforge proton-install`"),
        planned => planned?,
    };

    println!("using {} ({})", launch.build.version, launch.build.path.display());
    warn_on_downgrade(&layout, &launch.build);
    println!("running {}", launch.exe.display());
    if !launch.wrappers.is_empty() {
        let names: Vec<String> = launch.wrappers.iter().map(|w| w.display().to_string()).collect();
        println!("through  {}", names.join(" "));
    }
    if lightforge_core::launch_options::script(&launch.launch_options).is_some() {
        println!("options  {}", launch.launch_options.trim());
    }
    if dry_run {
        return Ok(());
    }

    let report = Preflight::new(config.prefix_user.clone())
        .with_managed(config.managed_products())
        .keeping_client(config.keeps_battle_net(true))
        .with_background_client(config.battle_net_in_background)
        .run(&launch.runner, &layout, Some(&code), &mut NoDownloads)
        .context("pre-flight cleanup")?;
    print_preflight(&report);
    owe_gui_client(&config, &report);

    // Never fatal, as in the GUI: the first launch on a new build gets one.
    match backup::Plan::for_product(&layout, &dirs, &code, config.backup_keep).before_launch() {
        Ok(Some(taken)) => println!("backup          {}", taken.path.display()),
        Ok(None) => {}
        Err(e) => eprintln!("warning: settings not backed up: {}", lightforge_core::error::chain(&e)),
    }

    let status = launch.command().status().context("launching the game")?;
    match status.code() {
        Some(0) | None => println!("exited"),
        Some(code) => println!("exited with code {code}"),
    }
    Ok(())
}

/// Bring an installed game up to the build its login region serves, as the
/// GUI's Update button does. Unreachable version service: the installed build
/// starts, as PLAY does offline.
async fn update_game(config: &Config, layout: &Layout, code: &ProductCode) -> Result<()> {
    use lightforge_core::net::Http;
    use lightforge_core::preflight::{self, ProcKind};
    use lightforge_core::state::Local;
    use lightforge_core::tact::download::Cancel;
    use lightforge_core::tact::install::{self, Event, Options as InstallOptions};
    use lightforge_core::tact::version::{self, Status};
    use std::time::{Duration, Instant};

    // Not installed is the launch plan's to report.
    let local = Local::inspect(layout, code);
    if !local.is_installed() {
        return Ok(());
    }
    let region = lightforge_core::portal::build_region(layout, code, config.region);
    let http = Http::new()?;
    let versions = match version::versions(&http, region, code.as_str()).await {
        Ok(versions) => versions,
        Err(e) => {
            eprintln!("warning: not checking for an update: {}", lightforge_core::error::chain(&e));
            return Ok(());
        }
    };
    let Some(remote) = version::for_region(&versions, region, |v| &v.region) else {
        return Ok(());
    };
    let Status::Behind { local_version, remote_version, .. } =
        Status::compare(local.build_key.as_deref(), local.version.as_deref(), remote)
    else {
        println!("game            up to date");
        return Ok(());
    };

    // Agent writes this same store, and beside it a download can corrupt both
    // (GlitchTip #41). The GUI waits for it to close; nobody waits here.
    if preflight::scan(layout).iter().any(|p| matches!(p.kind, ProcKind::BattleNet | ProcKind::Agent)) {
        anyhow::bail!("{code} needs an update, but Battle.net is running in the prefix and writes the same files: close it and run this again");
    }

    println!("update          {} -> {remote_version}", local_version.as_deref().unwrap_or("unknown"));
    let (account_country, geo_country) = lightforge_core::tact::productdb::countries(&layout.agent_dir());
    let options = InstallOptions {
        region,
        locale: config.locale_for(code).to_owned(),
        account_country,
        geo_country,
        flavor: layout.known_flavor(code).unwrap_or_default().to_owned(),
        optional_tags: lightforge_core::product::offered_only(
            config.product(code).map(|p| p.profile.optional_content.as_slice()).unwrap_or(&[]),
        ),
        limit_bytes_per_second: config.download_limit_bytes(),
        agent_dir: Some(layout.agent_dir()),
        windows_path: Some(windows_wow_path(layout, code)),
        ..InstallOptions::default()
    };

    let mut last = Instant::now();
    let on_event = move |event: Event| match event {
        Event::Planned { bytes, .. } => println!("fetching        {:.2} GiB", bytes as f64 / 1024f64.powi(3)),
        Event::Progress(p) if last.elapsed() >= Duration::from_secs(5) => {
            last = Instant::now();
            println!("  {:>5.1}%", p.fraction() * 100.0);
        }
        _ => {}
    };
    let summary = install::run(&http, &layout.wow_install(code), code, &options, &Cancel::new(), on_event)
        .await
        .with_context(|| format!("updating {code}"))?;
    if !summary.complete {
        anyhow::bail!("the update of {code} did not finish, run this again to continue: {}", summary.reasons.join("; "));
    }
    println!("updated         {}", summary.version.as_deref().unwrap_or(&remote_version));
    Ok(())
}

pub(crate) fn preflight(ctx: Ctx, proton: Option<String>, code: Option<ProductCode>, dry_run: bool, grace: u64) -> Result<()> {
    let Ctx { layout, config } = ctx;
    let config = beside_gui(config);
    use lightforge_core::preflight::{self, NoDownloads, Preflight};

    if dry_run {
        let found = preflight::scan(&layout);
        if found.is_empty() {
            println!("processes       none in {}", layout.prefix().display());
        }
        for proc in &found {
            println!("would signal    {:<24} pid {}  {:?}", proc.name, proc.pid, proc.kind);
        }
        for dir in [layout.agent_cache_dir(), layout.battle_net_staging()] {
            if dir.is_dir() {
                println!("would delete    {}", dir.display());
            }
        }
        return Ok(());
    }

    let (runner, build) = proton_runner(&layout, proton.as_deref())?;
    println!("using {} ({})", build.version, build.path.display());

    let report = Preflight::new(config.prefix_user.clone()).with_managed(config.managed_products())
        .with_grace(std::time::Duration::from_secs(grace))
        .with_background_client(config.battle_net_in_background)
        .run(&runner, &layout, code.as_ref(), &mut NoDownloads)
        .context("pre-flight cleanup")?;
    print_preflight(&report);
    owe_gui_client(&config, &report);
    Ok(())
}
