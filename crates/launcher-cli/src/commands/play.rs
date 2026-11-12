//! Starting a game, and the pre-flight in front of it.

use super::{beside_gui, owe_gui_client, print_preflight, proton_runner, warn_on_downgrade};
use crate::*;

pub(crate) fn play(ctx: Ctx, code: ProductCode, proton: Option<String>, dry_run: bool) -> Result<()> {
    use lightforge_core::backup;
    use lightforge_core::launch::{DirectLaunch, LaunchError};
    use lightforge_core::preflight::{NoDownloads, Preflight};

    let Ctx { layout, config } = ctx;
    let config = beside_gui(config);
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
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
