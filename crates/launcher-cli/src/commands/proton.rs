//! Proton builds, the prefix and first-run setup.

use super::proton_runner;
use crate::*;

pub(crate) fn proton() -> Result<()> {
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let steam = lightforge_core::steam::SteamRoot::discover();
    match &steam {
        Some(s) => {
            println!("steam root   {}", s.path().display());
            for lib in s.library_paths() {
                println!("library      {}", lib.display());
            }
        }
        None => println!("steam root   (not found)"),
    }
    println!("managed      {}", dirs.proton_dir().display());
    println!();

    let installed =
        lightforge_core::proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    if installed.is_empty() {
        println!("no Proton builds found");
    }
    for release in &installed {
        let mark = if release.flavor.is_supported() {
            "*"
        } else {
            " "
        };
        println!(
            "{mark} {:<24} {:?} {:?}\n    {}",
            release.version,
            release.flavor,
            release.source,
            release.path.display(),
        );
    }
    println!("\n* = supported launch target");
    Ok(())
}

pub(crate) async fn proton_check() -> Result<()> {
    use lightforge_core::net::{ETagCache, Http};
    use lightforge_core::proton;

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let cache = ETagCache::new(dirs.cache_dir().join("proton-releases.json"));
    let cached = cache.etag().is_some();

    let http = Http::new()?;
    let releases = proton::fetch_releases(&http, &cache, proton::ProtonFamily::GloriousEggroll)
        .await
        .context("fetching Proton-GE releases")?;

    println!(
        "{} releases ({})",
        releases.len(),
        if cached { "revalidated" } else { "fresh" }
    );
    if let Some(latest) = proton::latest_stable(&releases) {
        let size = latest.tarball().map(|a| a.size).unwrap_or(0);
        println!(
            "latest stable  {}  ({:.0} MB)",
            latest.tag_name,
            size as f64 / 1_048_576.0
        );
        println!(
            "checksum       {}",
            latest
                .tarball()
                .and_then(|t| latest.checksum_for(t))
                .map(|a| a.name.as_str())
                .unwrap_or("MISSING")
        );
    }

    let steam = lightforge_core::steam::SteamRoot::discover();
    let installed: Vec<_> = proton::discover_all(steam.as_ref(), &dirs.proton_dir())
        .into_iter()
        .filter(|r| r.flavor == proton::ProtonFlavor::GloriousEggroll)
        .collect();

    for build in &installed {
        match proton::check_for_update(&build.version, &releases) {
            Some(n) => println!("  {} -> {} available", n.current, n.available),
            None => println!("  {} is current", build.version),
        }
    }
    Ok(())
}

pub(crate) async fn proton_install(version: Option<String>) -> Result<()> {
    use lightforge_core::net::{ETagCache, Http};
    use lightforge_core::proton;
    use std::io::Write;

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let family = version
        .as_deref()
        .and_then(proton::ProtonFamily::of_tag)
        .unwrap_or(proton::ProtonFamily::GloriousEggroll);
    let cache = ETagCache::new(dirs.cache_dir().join(family.cache_file()));
    let http = Http::new()?;
    let releases = proton::fetch_releases(&http, &cache, family).await?;

    let release = match &version {
        Some(tag) => releases
            .iter()
            .find(|r| &r.tag_name == tag)
            .with_context(|| format!("no release tagged `{tag}`"))?,
        None => proton::latest_stable(&releases).context("no stable release found")?,
    };

    let dest = dirs.proton_dir();
    println!("installing {} -> {}", release.tag_name, dest.display());

    let start = std::time::Instant::now();
    let mut last = 0u64;
    let installed =
        proton::install_release(&http, release, &dest, dirs.cache_dir(), |done, total| {
            // Redraw at most once per MB to keep the callback cheap.
            if done - last < 1_048_576 && Some(done) != total {
                return;
            }
            last = done;
            let mb = done as f64 / 1_048_576.0;
            let rate = mb / start.elapsed().as_secs_f64().max(0.001);
            match total {
                Some(t) => print!(
                    "\r  {:.0}/{:.0} MB  {:.1} MB/s  ",
                    mb,
                    t as f64 / 1_048_576.0,
                    rate
                ),
                None => print!("\r  {mb:.0} MB  {rate:.1} MB/s  "),
            }
            let _ = std::io::stdout().flush();
        })
        .await?;

    println!("\nverified and extracted");
    println!("  {} at {}", installed.version, installed.path.display());
    Ok(())
}

pub(crate) fn prefix(ctx: Ctx) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let status = PrefixStatus::inspect(&layout);
    println!("compat data     {}", status.compat_data.display());
    println!("prefix          {}", status.prefix.display());
    println!(
        "initialized     {}",
        if status.initialized { "yes" } else { "no" }
    );
    // Which build the prefix *is*, as Proton recorded it. A pin older
    // than this is a downgrade of everything that shares the prefix.
    println!(
        "built by        {}",
        status.built_by.as_deref().unwrap_or("—")
    );
    println!("symlink         {}", describe_link(&status.link));
    println!(
        "ready           {}",
        if status.is_ready() { "yes" } else { "no" }
    );
    Ok(())
}

pub(crate) fn prefix_init(ctx: Ctx, proton: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let (runner, build) = proton_runner(&layout, proton.as_deref())?;
    println!("using {} ({})", build.version, build.path.display());
    println!("prefix {}", runner.prefix().display());

    let status = lightforge_core::prefix::provision(&runner, &layout)
        .context("provisioning the prefix")?;
    println!("symlink         {}", describe_link(&status.link));
    println!(
        "ready           {}",
        if status.is_ready() { "yes" } else { "no" }
    );
    Ok(())
}

pub(crate) fn prefix_link(ctx: Ctx) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    let main = layout
        .default_library()
        .context("no storage library is configured")?;
    let (link, target) = (layout.wow_symlink(), layout.wow_install_in(main));
    let before = lightforge_core::prefix::ensure_link(&link, &target)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))?;
    println!("was             {}", describe_link(&before));
    println!("now             {} -> {}", link.display(), target.display());
    Ok(())
}

pub(crate) async fn setup(ctx: Ctx, status: bool, proton: Option<String>) -> Result<()> {
    let Ctx { layout, .. } = ctx;
    use lightforge_core::config::Config;
    use lightforge_core::setup::{self, Event};

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let config = Config::load_or_default(&dirs.config_file())?;
    let current = setup::Status::inspect(&layout, &dirs, &config);

    let mark = |done: bool| if done { "ok " } else { "-- " };
    println!(
        "{}proton          {}",
        mark(current.proton.is_some()),
        match &current.proton {
            Some(b) => format!("{} ({})", b.version, setup::describe_source(b)),
            None => "not installed".to_owned(),
        }
    );
    println!(
        "{}prefix          {}",
        mark(current.prefix.is_ready()),
        current.prefix.prefix.display()
    );
    println!(
        "{}battle.net      {}",
        mark(current.battle_net),
        layout.battle_net_dir().display()
    );
    println!("   builds seen     {}", current.installed.len());
    println!(
        "\nready           {}  outstanding: {:?}",
        current.is_ready(),
        current.outstanding()
    );

    if status {
        return Ok(());
    }

    let mut last = std::time::Instant::now();
    let final_status = setup::run(&layout, &dirs, &config, proton.as_deref(), |event| {
        match event {
            Event::Started(step) => println!("\n> {} — {}", step.label(), step.detail()),
            Event::Progress { done, total } => {
                if last.elapsed().as_millis() >= 500 {
                    last = std::time::Instant::now();
                    match total {
                        Some(total) => println!(
                            "  {:.0}%  {:.0}/{:.0} MB",
                            done as f64 / total as f64 * 100.0,
                            done as f64 / 1e6,
                            total as f64 / 1e6
                        ),
                        None => println!("  {:.0} MB", done as f64 / 1e6),
                    }
                }
            }
            Event::Finished { step, skipped, note } => println!(
                "  {} {} — {note}",
                step.label(),
                if skipped { "already done" } else { "done" }
            ),
        }
    })
    .await?;

    println!("\nready           {}", final_status.is_ready());
    Ok(())
}

/// One-line rendering of the symlink's state for the CLI.
fn describe_link(state: &LinkState) -> String {
    match state {
        LinkState::Correct => "ok".to_owned(),
        LinkState::Missing => "missing".to_owned(),
        LinkState::Pending(t) => format!("ok -> {} (not downloaded yet)", t.display()),
        LinkState::Mislinked(t) => format!("points at {}", t.display()),
        LinkState::OccupiedEmpty => "empty directory in the way".to_owned(),
        LinkState::Occupied => "a real directory with files in it".to_owned(),
    }
}
