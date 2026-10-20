//! Battle.net: installing, configuring and running it, and Agent's registry.

use crate::*;

pub(crate) async fn battlenet_fetch() -> Result<()> {
    use lightforge_core::battlenet;
    use lightforge_core::net::Http;
    use std::io::Write;

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let http = Http::new()?;
    let start = std::time::Instant::now();
    let path = battlenet::fetch_installer(&http, dirs.cache_dir(), |done, total| {
        let mb = done as f64 / 1_048_576.0;
        match total {
            Some(t) => print!("\r  {:.1}/{:.1} MB  ", mb, t as f64 / 1_048_576.0),
            None => print!("\r  {mb:.1} MB  "),
        }
        let _ = std::io::stdout().flush();
    })
    .await
    .context("downloading Battle.net-Setup.exe")?;
    println!(
        "\n{} ({:.1}s)",
        path.display(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

pub(crate) fn battlenet_setup(ctx: Ctx, proton: Option<String>, locale: String, installer: Option<PathBuf>, timeout: u64) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::battlenet::{self, Installer};
    use lightforge_core::config::Config;

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let installer_path =
        installer.unwrap_or_else(|| dirs.cache_dir().join(battlenet::INSTALLER_FILE));
    let user = Config::default().prefix_user;

    let (runner, build) = proton_runner(&layout, battle_net_pin(proton.as_deref(), None))?;
    println!("using {} ({})", build.version, build.path.display());
    warn_on_downgrade(&layout, &build);
    println!("installer {}", installer_path.display());

    let setup = Installer::new(&layout, &installer_path)
        .with_locale(&locale)
        .with_timeout(std::time::Duration::from_secs(timeout));
    let mut container = None;
    if !setup.is_installed() {
        let steam = lightforge_core::steam::SteamRoot::discover();
        container = lightforge_core::runtime::installer_container(&build.path, steam.as_ref())?;
        if let Some(entry) = &container {
            println!("no 32-bit gnutls here, running the installer in {}", entry.display());
        }
        println!("running setup — this takes a few minutes");
    }

    let managed = Config::load_or_default(&dirs.config_file())
        .map(|c| c.managed_products())
        .unwrap_or_default();
    let report = battlenet::provision(
        &runner, container.as_deref(), &layout, &user, &locale, &installer_path, &managed,
    )
    .context("setting Battle.net up")?;
    if report.install.skipped {
        println!("battle.net      already installed");
    } else {
        println!(
            "battle.net      installed in {:.0}s",
            report.install.waited.as_secs_f64()
        );
    }
    println!("registry        {} values applied", report.registry_values);
    println!("config          {}", report.config_path.display());
    match report.account_configs.len() {
        0 => println!("accounts        none yet — log in once, then re-run `battlenet-config`"),
        n => println!("accounts        {n} updated"),
    }
    Ok(())
}

pub(crate) fn battlenet_config(ctx: Ctx) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::battlenet;
    use lightforge_core::config::Config;

    let user = Config::default().prefix_user;

    // The same set setup and pre-flight write, so running this by
    // hand cannot leave a config the next launch disagrees with.
    let mut tweaks = battlenet::default_tweaks(&layout);
    tweaks.extend(battlenet::product_tweaks(&managed_products()));
    let path = battlenet::configure(&layout, &user, &tweaks)
        .context("writing Battle.net.config")?;
    println!("{}", path.display());
    print_tweaks(&tweaks);

    let account_tweaks = battlenet::account_tweaks();
    let accounts = battlenet::configure_accounts(&layout, &user, &account_tweaks)
        .context("writing the per-account config")?;
    if accounts.is_empty() {
        println!("\nno per-account config yet — it appears after the first login");
    }
    for account in &accounts {
        println!("\n{}", account.display());
        print_tweaks(&account_tweaks);
    }
    Ok(())
}

// One parameter per command-line flag.
#[allow(clippy::too_many_arguments)]
pub(crate) fn battlenet_run(ctx: Ctx, proton: Option<String>, product: Option<String>, exec: Option<String>, uri: Option<String>, install_game: Option<String>, skip_preflight: bool) -> Result<()> {
    let Ctx { layout, config } = ctx;
    use lightforge_core::battlenet::{self, Launch};
    use lightforge_core::config::Config;
    use lightforge_core::preflight::{DownloadGuard, NoDownloads, Preflight};

    let launch = match (&product, &uri) {
        _ if install_game.is_some() => Launch::InstallFlow {
            uid: install_game.clone().unwrap_or_default(),
        },
        (_, Some(family)) => Launch::Uri(battlenet::game_uri(family)),
        (Some(code), None) => Launch::Product {
            code: code.parse().expect("product codes are infallible"),
            exec: exec.clone(),
        },
        (None, None) => Launch::Client,
    };

    // The product this launch is for, so its own pin is honoured the
    // same way the launcher honours it.
    let for_product = match &launch {
        Launch::Product { code, .. } => Some(code.clone()),
        _ => None,
    };
    let (mut runner, build) =
        proton_runner(&layout, battle_net_pin(proton.as_deref(), for_product.as_ref()))?;
    let (exe, args) = launch.command(&layout.battle_net_dir());
    println!("using {} ({})", build.version, build.path.display());
    warn_on_downgrade(&layout, &build);

    // Never spawn the client into a prefix that still has one running.
    let code = match &launch {
        Launch::Product { code, .. } => Some(code.clone()),
        Launch::Client
        | Launch::Autostart
        | Launch::Uri(_)
        | Launch::InstallFlow { .. } => None,
    };
    let mut downloads = NoDownloads;
    let mut download_paused = false;
    if !skip_preflight {
        let report = Preflight::new(config.prefix_user.clone()).with_managed(managed_products())
            .run(&runner, &layout, code.as_ref(), &mut downloads)
            .context("pre-flight cleanup")?;
        print_preflight(&report);
        download_paused = report.download_paused;
    }

    println!("running {}{}", exe.display(), if args.is_empty() { String::new() } else { format!(" {}", args.join(" ")) });
    // The shared Battle.net profile, the same one the GUI applies,
    // a launcher that honoured it from one entry point and not the
    // other would make every measurement unreproducible. Unlike the
    // GUI this always *is* the client (the CLI has no running-client
    // fast path), so the wrappers always belong here.
    let shared = AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .map(|config| config.battle_net_tweaks)
        .unwrap_or_default();
    let tools = lightforge_core::tweaks::HostTools::detect();
    for (key, value) in shared.environment(&tools) {
        runner = runner.with_env(key, value);
    }
    let mut child =
        battlenet::launch_wrapped(&runner, &layout, &launch, &shared.wrappers(&tools))
            .context("launching Battle.net")?;
    let status = child.wait().context("waiting for Battle.net")?;

    if download_paused {
        if let Some(code) = &code {
            downloads.resume(code).context("resuming the download")?;
            println!("download        resumed");
        }
    }

    // Battle.net exits non-zero on perfectly ordinary paths (a window
    // closed mid-update, a hand-off to the game), so a code is worth
    // printing but not worth failing the command over.
    match status.code() {
        Some(0) | None => println!("exited"),
        Some(code) => println!("exited with code {code}"),
    }
    Ok(())
}

pub(crate) fn product_db(ctx: Ctx, path: Option<PathBuf>, products: Vec<String>, account_country: String, geo_country: String, agent_tags: bool, dry_run: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::tact::productdb::{self, ProductInstall};

    let install = path.unwrap_or_else(|| default_install(&layout));
    let info = BuildInfo::load(install.join(".build.info"))
        .with_context(|| format!("reading {}/.build.info", install.display()))?;

    // Every product the install actually has a row for, unless told
    // otherwise, a `.product.db` naming a product that is not there is
    // exactly the inconsistency Agent refuses.
    let codes: Vec<ProductCode> = match products.is_empty() {
        false => products
            .iter()
            .map(|p| p.parse().expect("product codes are infallible"))
            .collect(),
        true => info.products().collect(),
    };

    let mut records = Vec::new();
    let mut info = info;
    for code in &codes {
        let windows_path = windows_wow_path(&layout, code);
        let mut record = ProductInstall::from_build_info(&info, code, &windows_path)?
            .with_countries(&account_country, &geo_country);
        if agent_tags {
            record = record.with_agent_tags();
            // Keep `.build.info` saying the same thing; the two
            // disagreeing is exactly the kind of inconsistency Agent
            // refuses an install for.
            info.set(code, "Tags", record.tags.clone())?;
        }
        println!(
            "{:<18} {:<16} {:<22} {}",
            record.product, record.version, record.flavor_dir, record.build_key
        );
        println!("  tags          {}", record.tags);
        records.push(record);
    }

    match dry_run {
        true => println!("\n--dry-run: nothing written"),
        false => {
            if agent_tags {
                info.save(install.join(".build.info"))?;
                println!("updated         .build.info tags");
            }
            let written = productdb::write(&install, &records)?;
            println!("\nwrote           {} ({} bytes)", written.display(), std::fs::metadata(&written)?.len());
            if let Some(first) = codes.first() {
                println!("install path    {}", windows_wow_path(&layout, first));
            }
        }
    }
    Ok(())
}
