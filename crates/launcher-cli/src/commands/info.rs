//! What Lightforge sees: paths, config, catalogue, status.

use crate::*;

pub(crate) fn paths(ctx: Ctx) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    println!("blizzard root   {}", layout.root().display());
    println!("compat data     {}", layout.compat_data().display());
    println!("prefix          {}", layout.prefix().display());
    println!("battle.net      {}", layout.battle_net_dir().display());
    println!("wow symlink     {}", layout.wow_symlink().display());
    println!("config file     {}", dirs.config_file().display());
    println!("asset cache     {}", dirs.asset_cache_dir().display());
    println!("proton dir      {}", dirs.proton_dir().display());

    for library in layout.libraries().all() {
        let state = match library.state() {
            lightforge_core::library::State::Ready => "ready",
            lightforge_core::library::State::New => "not set up",
            lightforge_core::library::State::Missing => "not mounted",
            lightforge_core::library::State::Foreign => "foreign marker",
        };
        println!(
            "\nlibrary         {} ({state}){}",
            library.label,
            if library.is_default { ", default" } else { "" },
        );
        println!("  games         {}", library.games().display());
        println!("  wow install   {}", layout.wow_install_in(library).display());
        println!(
            "  drives        {} -> games, {} -> wow",
            library.games_letter, library.wow_letter,
        );
        println!("  windows path  {}", library.windows_wow_path());
    }
    Ok(())
}

pub(crate) fn config() -> Result<()> {
    // No loader here; show what a fresh install writes.
    print!("{}", Config::with_default_products().to_toml()?);
    Ok(())
}

pub(crate) async fn products(region: String, codes: Vec<String>, all: bool, concurrency: usize) -> Result<()> {
    use lightforge_core::net::Http;
    use lightforge_core::tact::version::{self, Region, KNOWN_PRODUCTS};

    let region: Region = region.parse().context("parsing --region")?;

    let mut candidates: Vec<&str> = KNOWN_PRODUCTS.iter().map(|(c, _)| *c).collect();
    for code in &codes {
        if !candidates.contains(&code.as_str()) {
            candidates.push(code);
        }
    }

    let http = Http::new()?;
    let found = version::probe_all(&http, region, &candidates, concurrency).await;

    println!("{:<22} {:<24} {:<18} BUILD", "CODE", "LABEL", "VERSION");
    for status in &found {
        match &status.version {
            Some(v) => println!(
                "{:<22} {:<24} {:<18} {}",
                status.code,
                status.label.as_deref().unwrap_or("-"),
                v.versions_name,
                v.build_id
            ),
            None if all => println!(
                "{:<22} {:<24} {:<18} not served",
                status.code,
                status.label.as_deref().unwrap_or("-"),
                "-"
            ),
            None => {}
        }
    }
    let live = found.iter().filter(|s| s.exists()).count();
    println!("\n{live} of {} probed codes are live in {}", found.len(), region.as_str());
    Ok(())
}

pub(crate) async fn assets(product: String, source: Option<String>) -> Result<()> {
    use lightforge_core::assets::{Origin, Pipeline, Store};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let config = Config::load_or_default(&dirs.config_file())
        .with_context(|| format!("reading {}", dirs.config_file().display()))?;
    let origin = match source.as_deref().map(str::trim) {
        Some(s) if s.contains("://") => Origin::Http(s.to_owned()),
        Some(s) => Origin::Dir(PathBuf::from(s)),
        None => config.asset_origin(),
    };

    let store = Store::new(dirs.asset_cache_dir());
    println!("origin          {origin:?}");
    println!("cache           {}", store.dir().display());

    let pipeline = Pipeline::new(origin, store);

    // The cache-only path the GUI paints its first frame from. Shown
    // before the network runs, because "the dashboard opens unthemed"
    // is almost always this returning None.
    let started = std::time::Instant::now();
    match pipeline.cached(&code) {
        Some(branding) => println!(
            "cache-first     hit in {:?} ({})",
            started.elapsed(),
            branding.label
        ),
        None => println!(
            "cache-first     MISS in {:?} — the GUI opens unthemed",
            started.elapsed()
        ),
    }

    let http = Http::new()?;
    let started = std::time::Instant::now();
    let manifest = pipeline.manifest(&http).await.context("fetching the manifest")?;
    println!("manifest        {:?}", started.elapsed());
    println!("schema          {}", manifest.schema);
    println!("themes          {}", manifest.products.len());
    println!("default         {}", manifest.default.as_deref().unwrap_or("-"));

    let started = std::time::Instant::now();
    let branding = pipeline
        .resolve_with(&http, &manifest, &code)
        .await
        .with_context(|| format!("resolving branding for {}", code.as_str()))?;
    println!("images          {:?}", started.elapsed());

    println!("\nproduct         {}", code.as_str());
    println!("label           {}", branding.label);
    let slots = [
        ("logo", Some(&branding.logo)),
        ("background", Some(&branding.background)),
        ("icon", branding.icon.as_ref()),
    ];
    for (slot, asset) in slots {
        let Some(asset) = asset else {
            println!("{slot:<15} —");
            continue;
        };
        let size = std::fs::metadata(&asset.path).map(|m| m.len()).unwrap_or(0);
        let height = match asset.height {
            Some(h) => format!(", {h}px"),
            None => String::new(),
        };
        println!(
            "{slot:<15} {} ({size} bytes, {}{height})",
            asset.path.display(),
            if asset.fetched { "fetched" } else { "cached" }
        );
    }
    Ok(())
}

pub(crate) async fn status(ctx: Ctx, product: String, region: String, offline: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::net::Http;
    use lightforge_core::state::{decide, Local};
    use lightforge_core::tact::version::{self, Region};

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let region: Region = region.parse().context("parsing --region")?;

    let local = Local::inspect(&layout, &code);
    println!("product         {}", code.as_str());
    println!("install dir     {}", layout.product_dir(&code).display());
    println!("build key       {}", local.build_key.as_deref().unwrap_or("-"));
    println!("version         {}", local.version.as_deref().unwrap_or("-"));
    println!(
        "executable      {}",
        local.executable.as_ref().map_or("-".into(), |p| p.display().to_string())
    );
    println!("store files     {}", local.store_files);
    println!("installed       {}", local.is_installed());

    // `None` here is the offline case the resolver is required to
    // survive: a complete install stays playable when the service
    // cannot be reached.
    let remote = if offline {
        None
    } else {
        match version::versions(&Http::new()?, region, code.as_str()).await {
            Ok(rows) => version::for_region(&rows, region, |v| &v.region).cloned(),
            Err(e) => {
                println!("version service unreachable: {e}");
                None
            }
        }
    };
    if let Some(remote) = &remote {
        println!("remote build    {}", remote.build_config);
        println!("remote version  {}", remote.versions_name);
    }

    // No transfer: the CLI is one-shot, so nothing of ours is running.
    let state = decide(&local, remote.as_ref(), None);
    println!("\nbutton          [ {} ]", state.label());
    println!("state           {state:?}");
    println!("playable        {}", state.is_playable());
    Ok(())
}

pub(crate) async fn update_check(ctx: Ctx, path: Option<PathBuf>, product: String, region: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::net::Http;
    use lightforge_core::tact::version::{self, Region, Status};

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let region: Region = region.parse().context("parsing --region")?;
    let install = path.unwrap_or_else(|| layout.wow_install(&code));

    // Local side: whatever .build.info records, if the product is
    // installed at all. A missing file is "not installed", not a fault.
    let local = BuildInfo::load(install.join(".build.info")).ok();
    let local_build = local.as_ref().and_then(|i| {
        i.get(&code, lightforge_core::buildinfo::COL_BUILD_KEY).ok()
    });
    let local_version = local.as_ref().and_then(|i| {
        i.get(&code, lightforge_core::buildinfo::COL_VERSION).ok()
    });

    let http = Http::new()?;
    let t = std::time::Instant::now();
    let versions = version::versions(&http, region, code.as_str())
        .await
        .context("fetching the version table")?;
    let remote = version::for_region(&versions, region, |v| &v.region)
        .context("the version table is empty")?;
    println!("service         {} ({:?})", region.host(), t.elapsed());
    println!("regions         {}", versions.len());
    println!("remote build    {} ({})", remote.build_config, remote.versions_name);
    match local_build {
        Some(b) => println!("local build     {b} ({})", local_version.unwrap_or("?")),
        None => println!("local build     (not installed)"),
    }

    let status = Status::compare(local_build, local_version, remote);
    println!("\nstatus          {:?}", status.action().label());
    match &status {
        Status::Current { version, .. } => println!("                up to date on {version}"),
        Status::Behind { local_version, remote_version, .. } => println!(
            "                {} -> {remote_version}",
            local_version.as_deref().unwrap_or("unknown")
        ),
        Status::NotInstalled => println!("                nothing installed for `{code}`"),
    }

    let cdns = version::cdns(&http, region, code.as_str())
        .await
        .context("fetching the CDN table")?;
    if let Some(cdn) = version::for_region(&cdns, region, |c| &c.name) {
        println!("\ncdn path        {}", cdn.path);
        println!("cdn config path {}", cdn.config_path);
        for server in cdn.https_servers() {
            println!("cdn (tls)       {server}");
        }
        for server in cdn.servers.iter().filter(|s| s.starts_with("http://")) {
            println!("cdn (plain)     {server}");
        }
    }
    Ok(())
}
