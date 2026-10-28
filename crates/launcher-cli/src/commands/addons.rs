//! Addons and settings backups.

use super::truncate;
use crate::*;

pub(crate) async fn addon_adopt(ctx: Ctx, product: String, path: Option<PathBuf>, ledger: Option<PathBuf>, catalog: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons::catalog::Catalog;
    use lightforge_core::addons::manage;
    use lightforge_core::addons::{self};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let dir = path.unwrap_or_else(|| layout.addons_dir(&code));
    let options = manage::Options {
        flavor: addons::flavor_of(&code).to_owned(),
        addons_dir: dir.clone(),
        ledger_path: ledger
            .unwrap_or_else(|| manage::ledger_path(dirs.data_dir(), &dir, &code)),
        pinned: false,
        reinstall: false,
        channel: addon_channel(&dirs),
        // A directory given by hand may not belong to this game, and
        // its settings are not the ones an update there could touch.
        backup: (dir == layout.addons_dir(&code))
            .then(|| settings_backup(&layout, &dirs, &code)),
    };
    let client = addon_origin(catalog.as_deref(), &dirs).map(Catalog::new);
    let http = Http::new().context("http client")?;

    println!("addons dir      {}", options.addons_dir.display());
    println!("ledger          {}\n", options.ledger_path.display());

    let adopted = manage::adopt(&http, client.as_ref(), &options)
        .await
        .context("adopting hand-installed addons")?;
    match adopted.len() {
        0 => println!("nothing recognised"),
        n => {
            for item in &adopted {
                println!("{:<34} {}", item.name, item.folders.join(", "));
            }
            println!("\n{n} adopted");
        }
    }
    Ok(())
}

pub(crate) async fn addon_fingerprints(ctx: Ctx, product: String, path: Option<PathBuf>, catalog: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons::catalog::Catalog;
    use lightforge_core::addons::install::Ledger;
    use lightforge_core::addons::manage;
    use lightforge_core::addons::{self, fingerprint};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let dir = path.unwrap_or_else(|| layout.addons_dir(&code));
    let ledger = Ledger::load(&manage::ledger_path(dirs.data_dir(), &dir, &code))
        .unwrap_or_default();

    // What the catalog says each folder of each installed build
    // should hash to, gathered per addon so the comparison below is
    // a lookup, not a guess.
    let mut expected: std::collections::BTreeMap<String, u32> = Default::default();
    if let Some(origin) = addon_origin(catalog.as_deref(), &dirs) {
        let client = Catalog::new(origin);
        let http = Http::new().context("http client")?;
        for record in ledger.addons.iter().filter(|r| r.provider == "curse") {
            let files = client
                .files(&http, &record.id, addons::flavor_of(&code), 50)
                .await
                .unwrap_or_default();
            let Some(file) = files
                .iter()
                .find(|f| Some(&f.file_id) == record.file_id.as_ref())
            else {
                continue;
            };
            expected.extend(file.fingerprints.clone());
        }
    }

    println!("{:<32} {:>12} {:>12}", "folder", "computed", "published");
    let mut checked = 0;
    let mut agreed = 0;
    for addon in addons::scan(&dir, &addons::Client::default()).unwrap_or_default() {
        let computed = fingerprint::of_folder(&dir.join(&addon.folder));
        let published = expected.get(&addon.folder).copied();
        let verdict = match (computed, published) {
            (Some(a), Some(b)) => {
                checked += 1;
                match a == b {
                    true => {
                        agreed += 1;
                        "match"
                    }
                    false => "MISMATCH",
                }
            }
            (None, _) => "no toc",
            (_, None) => "",
        };
        println!(
            "{:<32} {:>12} {:>12}  {verdict}",
            addon.folder,
            computed.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            published.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
        );
    }
    match checked {
        0 => println!("\nnothing to check against — no catalog, or no CurseForge addons"),
        n => println!("\n{agreed}/{n} folders hash to what the catalog publishes"),
    }
    Ok(())
}

pub(crate) fn addons(ctx: Ctx, product: String, path: Option<PathBuf>, token: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dir = path.unwrap_or_else(|| layout.addons_dir(&code));
    let version = BuildInfo::load(layout.build_info(&code))
        .ok()
        .and_then(|info| info.get(&code, lightforge_core::buildinfo::COL_VERSION).ok().map(str::to_owned));
    let client = addons::Client::of(&code, version.as_deref());
    let installed = addons::scan(&dir, &client)
        .with_context(|| format!("reading {}", dir.display()))?;

    println!("addons dir      {}", dir.display());
    println!("installed       {}", installed.len());
    if let Some(interface) = client.interface {
        let stale = installed.iter().filter(|a| a.out_of_date).count();
        println!("out of date     {stale}, for interface {interface}");
    }
    for (folder, dep) in addons::missing_dependencies(&installed) {
        println!("missing         {dep}, which {folder} needs");
    }
    println!();

    for addon in &installed {
        let ids: Vec<String> = addon
            .ids
            .iter()
            .map(|(provider, id)| format!("{}:{id}", provider.as_str()))
            .collect();
        println!(
            "{:<34} {:<18} {}{}",
            addon.folder,
            addon.version.as_deref().unwrap_or("-"),
            ids.join(" "),
            if addon.out_of_date { "  (out of date)" } else { "" },
        );
    }

    if let Some(token) = token {
        let collection = addons::Collection::parse(&read_token(&token)?)?;
        println!(
            "\ncollection      {} ({} addons, exported from {})",
            collection.collection_name,
            collection.addons.len(),
            collection.client_type
        );

        let report = collection.reconcile(&installed);
        let have = report.iter().filter(|r| r.is_installed()).count();
        println!("already present {have} of {}\n", report.len());
        for item in &report {
            println!(
                "{} {:<44} {:<14} {:<22} {}",
                if item.is_installed() { "ok " } else { "-- " },
                truncate(&item.entry.name, 44),
                format!("{}:{}", item.entry.provider().as_str(), item.entry.id),
                item.version.as_deref().unwrap_or("not installed"),
                // How it matched matters: an id match is certain, a
                // name match is a guess and should look like one.
                item.matched_by.map_or(String::new(), |how| match how.is_certain() {
                    true => format!("by {}", how.as_str()),
                    false => format!("by {} (guess)", how.as_str()),
                })
            );
        }
    }
    Ok(())
}

pub(crate) async fn addon_search(query: String, product: String, limit: usize, catalog: Option<String>) -> Result<()> {
    use lightforge_core::addons::{self, browse, catalog::Catalog};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let catalog = addon_origin(catalog.as_deref(), &dirs).map(Catalog::new);
    let http = Http::new()?;

    let found = browse::search(
        &http,
        catalog.as_ref(),
        dirs.cache_dir(),
        &query,
        addons::flavor_of(&code),
        limit,
    )
    .await;

    println!("{:<14} {:<22} {:<40} DOWNLOADS", "PROVIDER", "ID", "NAME");
    for listing in &found.listings {
        println!(
            "{:<14} {:<22} {:<40} {}",
            listing.provider.as_str(),
            truncate(&listing.id, 22),
            truncate(&listing.name, 40),
            match listing.downloads {
                0 => "-".to_owned(),
                n => n.to_string(),
            }
        );
    }
    println!("\n{} results", found.listings.len());
    for problem in &found.problems {
        eprintln!("  ! {problem}");
    }
    Ok(())
}

pub(crate) async fn addon_install(ctx: Ctx, id: String, provider: String, product: String, catalog: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons::{self, catalog::Catalog, catalog::Request, manage};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let catalog = addon_origin(catalog.as_deref(), &dirs).map(Catalog::new);
    let options = manage::Options {
        flavor: addons::flavor_of(&code).to_owned(),
        addons_dir: layout.addons_dir(&code),
        ledger_path: manage::ledger_path(
            dirs.data_dir(),
            &layout.addons_dir(&code),
            &code,
        ),
        pinned: false,
        reinstall: false,
        channel: addon_channel(&dirs),
        backup: Some(settings_backup(&layout, &dirs, &code)),
    };

    // install_all fills the channel in from the ledger or the default.
    let requests = vec![Request { provider, id, version_id: None, channel: None }];
    let http = Http::new()?;
    let report =
        manage::install_all(&http, catalog.as_ref(), &requests, &options, |event| {
            print_addon_event(&event)
        })
        .await?;
    print_addon_report(&report);
    Ok(())
}

pub(crate) async fn addon_import(ctx: Ctx, token: String, product: String, catalog: Option<String>, pinned: bool, dry_run: bool) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons::{self, catalog::Catalog, manage};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let collection = addons::Collection::parse(&read_token(&token)?)?;

    println!("collection      {}", collection.collection_name);
    println!("exported from   {}", collection.client_type);
    if let Some(exported) = collection.product() {
        if exported != code {
            println!(
                "NOTE            exported from {} but installing into {}",
                exported.as_str(),
                code.as_str()
            );
        }
    }

    let requests = manage::requests_from(&collection);
    for entry in manage::unsupported(&collection) {
        println!("skipping        {} ({})", entry.name, entry.provider);
    }

    // Optional now: the keyless providers work without it.
    let catalog = addon_origin(catalog.as_deref(), &dirs).map(Catalog::new);
    let options = manage::Options {
        flavor: addons::flavor_of(&code).to_owned(),
        addons_dir: layout.addons_dir(&code),
        ledger_path: manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code),
        pinned,
        reinstall: false,
        channel: addon_channel(&dirs),
        backup: Some(settings_backup(&layout, &dirs, &code)),
    };
    println!("addons dir      {}", options.addons_dir.display());
    println!("flavor          {}\n", options.flavor);

    let http = Http::new()?;
    if dry_run {
        let resolved = manage::resolve_everywhere(
            &http,
            catalog.as_ref(),
            &requests,
            &options.flavor,
        )
        .await?;
        for (_, entry) in &resolved {
            let detail = entry.unavailable_reason().unwrap_or_else(|| {
                format!(
                    "{} {}",
                    entry.file_name.as_deref().unwrap_or("-"),
                    entry.file_id.as_deref().unwrap_or("")
                )
            });
            println!(
                "{:<26} {:<46} {}{}",
                entry.id,
                truncate(&detail, 46),
                entry.note.as_deref().unwrap_or(""),
                if entry.md5.is_some() { "  [md5]" } else { "" }
            );
        }
        println!("\n--dry-run: nothing written");
        return Ok(());
    }

    let report = manage::install_all(&http, catalog.as_ref(), &requests, &options, |event| {
        print_addon_event(&event)
    })
    .await?;
    print_addon_report(&report);
    Ok(())
}

pub(crate) async fn addon_update(ctx: Ctx, product: String, catalog: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons::{self, catalog::Catalog, manage};
    use lightforge_core::net::Http;

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let catalog = addon_origin(catalog.as_deref(), &dirs).map(Catalog::new);
    let options = manage::Options {
        flavor: addons::flavor_of(&code).to_owned(),
        addons_dir: layout.addons_dir(&code),
        ledger_path: manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code),
        pinned: false,
        reinstall: false,
        channel: addon_channel(&dirs),
        backup: Some(settings_backup(&layout, &dirs, &code)),
    };

    let http = Http::new()?;
    let report = manage::update_all(&http, catalog.as_ref(), &options, |event| {
        print_addon_event(&event)
    })
    .await?;
    print_addon_report(&report);
    Ok(())
}

pub(crate) fn backup_list(ctx: Ctx, product: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let plan = settings_backup(&layout, &dirs, &code);
    let backups = plan.list().context("reading backups")?;
    if backups.is_empty() {
        println!("no backups in {}", plan.dir.display());
    }
    for (n, b) in backups.iter().enumerate() {
        println!(
            "{:>2}  {}  {:<36} {:<16} {:>8.1} MiB  {}",
            n + 1,
            b.when(),
            b.reason.label(),
            b.build.as_deref().unwrap_or("-"),
            b.size as f64 / 1048576.0,
            b.name(),
        );
    }
    Ok(())
}

pub(crate) fn backup_create(ctx: Ctx, product: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let plan = settings_backup(&layout, &dirs, &code);
    match plan.create(backup::Reason::Manual).context("backing up")? {
        Some(b) => println!("backed up to {}", b.path.display()),
        None if plan.account_dir.is_dir() => println!("unchanged since the last backup"),
        None => println!("nothing to back up: {} does not exist", plan.account_dir.display()),
    }
    Ok(())
}

pub(crate) fn backup_restore(ctx: Ctx, which: String, product: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let running = lightforge_core::preflight::games_running(&layout);
    if !running.is_empty() {
        anyhow::bail!("close the game first: {} is running", running[0].name);
    }
    let plan = settings_backup(&layout, &dirs, &code);
    let backups = plan.list().context("reading backups")?;
    let chosen = match which.parse::<usize>() {
        Ok(n) => n.checked_sub(1).and_then(|i| backups.get(i)),
        Err(_) => backups.iter().find(|b| b.name() == which),
    }
    .with_context(|| format!("no backup {which}; see `lightforge backup-list`"))?;
    let safety = plan.restore(chosen).context("restoring")?;
    println!("restored {} ({})", chosen.name(), chosen.when());
    if let Some(safety) = safety {
        println!("the replaced settings are in {}", safety.name());
    }
    Ok(())
}

pub(crate) fn addon_remove(ctx: Ctx, id: String, provider: String, product: String) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::addons::{self, manage};

    let code: ProductCode = product.parse().expect("product codes are infallible");
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let options = manage::Options {
        flavor: addons::flavor_of(&code).to_owned(),
        addons_dir: layout.addons_dir(&code),
        ledger_path: manage::ledger_path(dirs.data_dir(), &layout.addons_dir(&code), &code),
        pinned: false,
        reinstall: false,
        channel: addon_channel(&dirs),
        backup: Some(settings_backup(&layout, &dirs, &code)),
    };

    let removed = manage::remove(&options, &provider, &id)?;
    match removed.is_empty() {
        true => println!("nothing to delete — the folders were already gone"),
        false => println!("removed         {}", removed.join(", ")),
    }
    Ok(())
}

pub(crate) fn addon_token(token: String) -> Result<()> {
    use lightforge_core::addons;

    let collection = addons::Collection::parse(&read_token(&token)?)?;
    println!("collection      {}", collection.collection_name);
    println!("client type     {}", collection.client_type);
    println!(
        "product         {}",
        collection.product().map_or("unrecognised".to_owned(), |p| p.as_str().to_owned())
    );
    println!("addons          {}\n", collection.addons.len());

    for (provider, entries) in collection.by_provider() {
        println!(
            "{} — {} addon(s){}",
            provider.as_str(),
            entries.len(),
            if provider.is_supported() { "" } else { "  [not resolvable]" }
        );
        for entry in entries {
            println!(
                "  {:<46} id {:<10} file {}",
                truncate(&entry.name, 46),
                entry.id,
                entry.version_id.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}

/// A token given inline, or `@path` to read it from a file, export strings
/// are long enough that pasting one into a shell is its own hazard.
fn read_token(token: &str) -> Result<String> {
    match token.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading {path}"))
            .map(|s| s.trim().to_owned()),
        None => Ok(token.trim().to_owned()),
    }
}

/// Where this game's settings backups go, and how many it keeps.
fn settings_backup(layout: &Layout, dirs: &AppDirs, code: &ProductCode) -> backup::Plan {
    let keep = Config::load_or_default(&dirs.config_file())
        .map_or(backup::DEFAULT_KEEP, |config| config.backup_keep);
    backup::Plan::for_product(layout, dirs, code, keep)
}

/// The configured default addon channel, or stable when there is no config.
///
/// The CLI has no settings UI, so it reads the same `addon_channel` the GUI
/// writes instead of inventing a second default.
fn addon_channel(dirs: &AppDirs) -> lightforge_core::addons::Channel {
    Config::load_or_default(&dirs.config_file())
        .map(|config| config.addon_channel)
        .unwrap_or_default()
}

/// Where the addon catalog lives: the flag, else config, else the public
/// endpoint. A path without a scheme is a fixture directory.
fn addon_origin(
    flag: Option<&str>,
    dirs: &AppDirs,
) -> Option<lightforge_core::addons::catalog::Origin> {
    use lightforge_core::addons::catalog::Origin;
    use lightforge_core::config::Config;

    // A `--catalog` flag wins; otherwise the config decides, and the config
    // falls back to the built-in proxy, not to nothing.
    if let Some(source) = flag {
        return Some(match source.contains("://") {
            true => Origin::Http(source.to_owned()),
            false => Origin::Dir(PathBuf::from(source)),
        });
    }
    Some(
        Config::load_or_default(&dirs.config_file())
            .map(|c| c.catalog_origin())
            .unwrap_or_else(|_| Origin::Http(
                lightforge_core::config::DEFAULT_ADDON_CATALOG.to_owned(),
            )),
    )
}

fn print_addon_event(event: &lightforge_core::addons::manage::Event) {
    use lightforge_core::addons::manage::Event;
    match event {
        Event::Resolving { count } => println!("resolving       {count} addons"),
        Event::Resolved { ok, unavailable } => {
            println!("resolved        {ok} available, {unavailable} not\n")
        }
        Event::Downloading { name } => println!("  fetching {name}"),
        Event::Installed { name, folders } => {
            println!("ok  {:<44} {}", truncate(name, 44), folders.join(", "))
        }
        Event::UpToDate { name } => println!("==  {:<44} up to date", truncate(name, 44)),
        Event::Unavailable { name, reason } => {
            println!("--  {:<44} {reason}", truncate(name, 44))
        }
    }
}

fn print_addon_report(report: &lightforge_core::addons::manage::Report) {
    println!(
        "\ninstalled       {}\nup to date      {}\nunavailable     {}",
        report.installed.len(),
        report.up_to_date.len(),
        report.unavailable.len()
    );
}
