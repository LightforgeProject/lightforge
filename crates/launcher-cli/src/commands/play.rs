//! Starting a game, and the pre-flight in front of it.

use crate::*;

pub(crate) fn play(ctx: Ctx, product: String, proton: Option<String>, dry_run: bool) -> Result<()> {
    let Ctx { layout, config } = ctx;
    use lightforge_core::config::Config;
    use lightforge_core::preflight::{NoDownloads, Preflight};

    let code: ProductCode = product.parse().expect("product codes are infallible");

    // The game is launched through the symlink, so Wine sees the
    // conventional Windows path, the same thing that keeps CEF happy
    // for the in-game store.
    let flavor_dir = layout.wow_symlink().join(&*code.install_dir());
    let exe = lightforge_core::paths::game_executable(&flavor_dir, &code);
    if !exe.is_file() {
        anyhow::bail!(
            "no game at {} — is the symlink pointing at an install?",
            exe.display()
        );
    }

    // Whatever the symlink resolves to has to be visible inside the
    // prefix, and it is not necessarily under the games root.
    let target = std::fs::read_link(layout.wow_symlink())
        .unwrap_or_else(|_| layout.wow_install(&code));

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let (runner, build) = proton_runner(&layout, proton.as_deref())?;
    let mut runner = runner
        .with_compat_mount(&target)
        // Per-product shader cache: the prefix is shared, so pooling
        // it would mean a Proton swap for one product discarding
        // another's accumulated shaders.
        .with_shader_cache(dirs.shader_cache_dir(&code));

    // The same per-product settings the GUI's Game Optimizations panel
    // writes. A launcher that honoured them from one entry point and
    // not the other would make every measurement unreproducible.
    let tweaks = Config::load_or_default(&dirs.config_file())
        .ok()
        .and_then(|config| config.product(&code).map(|p| p.profile.tweaks.clone()))
        .unwrap_or_default();
    let tools = lightforge_core::tweaks::HostTools::detect();
    for (key, value) in tweaks.environment(&tools) {
        runner = runner.with_env(key, value);
    }

    println!("using {} ({})", build.version, build.path.display());
    println!("install {}", target.display());
    println!("running {}", exe.display());
    let wrappers = tweaks.wrappers(&tools);
    if !wrappers.is_empty() {
        let names: Vec<String> =
            wrappers.iter().map(|w| w.display().to_string()).collect();
        println!("through  {}", names.join(" "));
    }
    if dry_run {
        return Ok(());
    }

    let report = Preflight::new(config.prefix_user.clone()).with_managed(managed_products())
        .run(&runner, &layout, Some(&code), &mut NoDownloads)
        .context("pre-flight cleanup")?;
    print_preflight(&report);

    // After pre-flight, with the client closed: it rewrites Config.wtf
    // from memory on exit, so anything set once is not still set.
    match lightforge_core::cvars::reconcile(
        &layout.wtf_dir(&code).join("Config.wtf"),
        &tweaks.cvars(),
        lightforge_core::tweaks::RETIRED,
    ) {
        Ok(done) => {
            if done.changed > 0 {
                println!("applied {} client setting(s)", done.changed);
            }
            if !done.yielded.is_empty() {
                println!("left as set in the game: {}", done.yielded.join(", "));
            }
        }
        Err(e) => eprintln!("could not apply client settings: {e}"),
    }

    let status = lightforge_core::tweaks::wrapped(
        runner.command("run", &exe, Vec::<String>::new()),
        &wrappers,
    )
    .status()
    .context("launching the game")?;
    match status.code() {
        Some(0) | None => println!("exited"),
        Some(code) => println!("exited with code {code}"),
    }
    Ok(())
}

pub(crate) fn preflight(ctx: Ctx, proton: Option<String>, product: Option<String>, dry_run: bool, grace: u64) -> Result<()> {
    let Ctx { layout, config } = ctx;
    use lightforge_core::preflight::{self, NoDownloads, Preflight};

    let code: Option<ProductCode> =
        product.map(|p| p.parse().expect("product codes are infallible"));

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

    let report = Preflight::new(config.prefix_user.clone()).with_managed(managed_products())
        .with_grace(std::time::Duration::from_secs(grace))
        .run(&runner, &layout, code.as_ref(), &mut NoDownloads)
        .context("pre-flight cleanup")?;
    print_preflight(&report);
    Ok(())
}
