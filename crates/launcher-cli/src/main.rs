//! Headless consumer of `lightforge-core`.
//!
//! Exists to exercise and inspect core without the GUI in the loop — useful
//! when debugging a prefix or a `.build.info` on a machine with no display.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lightforge_core::buildinfo::BuildInfo;
use lightforge_core::config::Config;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::prefix::{LinkState, PrefixStatus};
use lightforge_core::ProductCode;

#[derive(Parser)]
#[command(name = "lightforge", version, about = "Lightforge CLI")]
struct Cli {
    /// Override the Blizzard root (default: ~/Games/Blizzard).
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the resolved directory layout.
    Paths,
    /// Print the effective configuration as TOML.
    Config,
    /// List Proton builds installed on this system.
    Proton,
    /// Check upstream Proton-GE releases and report whether a newer one exists.
    ProtonCheck,
    /// Download, verify, and install a Proton-GE release.
    ProtonInstall {
        /// Release tag, e.g. `GE-Proton11-7`. Defaults to the latest stable.
        version: Option<String>,
    },
    /// Report on the shared prefix and the game symlink.
    Prefix,
    /// Create or update the prefix, then put the game symlink in place.
    PrefixInit {
        /// Proton build to use, e.g. `GE-Proton11-7`. Defaults to the newest
        /// supported build installed.
        #[arg(long)]
        proton: Option<String>,
    },
    /// Create or repair only the game symlink, without running Proton.
    PrefixLink,
    /// Download Battle.net-Setup.exe into the cache.
    BattlenetFetch,
    /// Install Battle.net headlessly, then apply registry and config defaults.
    BattlenetSetup {
        /// Proton build to run the installer under.
        #[arg(long)]
        proton: Option<String>,
        /// Battle.net UI language.
        #[arg(long, default_value = "enUS")]
        locale: String,
        /// Use an already-downloaded installer instead of the cached one.
        #[arg(long)]
        installer: Option<PathBuf>,
        /// Seconds to wait for the install to appear.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Re-apply the `Battle.net.config` defaults without running the installer.
    BattlenetConfig,
    /// Open Battle.net in the prefix, optionally handing straight off to a game.
    BattlenetRun {
        /// Proton build to launch under.
        #[arg(long)]
        proton: Option<String>,
        /// Product code to launch, e.g. `wow`. Omit to just open the client.
        #[arg(long)]
        product: Option<String>,
        /// Override the `--exec="launch <uid>"` verb Battle.net is given.
        #[arg(long, requires = "product")]
        exec: Option<String>,
    },
    /// Clean the prefix up the way a launch would, without launching anything.
    Preflight {
        /// Proton build whose `wineserver` shuts the prefix down.
        #[arg(long)]
        proton: Option<String>,
        /// Product the launch is for, so a download can be paused.
        #[arg(long)]
        product: Option<String>,
        /// Report what would be done without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// Seconds a process gets between SIGTERM and SIGKILL.
        #[arg(long, default_value_t = 5)]
        grace: u64,
    },
    /// Show the `.build.info` row for a product.
    BuildInfo {
        /// Product code, e.g. `wow` or `wow_classic`.
        #[arg(default_value = "wow")]
        product: String,
    },
    /// Inspect the CASC bucket indices of an install, verifying both hashes
    /// and re-serializing each file to prove we can write them byte-identically.
    Idx {
        /// A WoW install root, or a `Data/data` directory directly.
        path: PathBuf,
    },
    /// Atomically rewrite one product's build keys in `.build.info`.
    BuildInfoSet {
        /// Product code, e.g. `wow`.
        product: String,
        #[arg(long)]
        build_key: String,
        #[arg(long)]
        cdn_key: String,
        #[arg(long)]
        version: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let layout = match &cli.root {
        Some(root) => Layout::with_root(root),
        None => Layout::default_for_user().context("resolving the default Blizzard root")?,
    };

    match cli.command {
        Command::Paths => {
            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            println!("blizzard root   {}", layout.root().display());
            println!("compat data     {}", layout.compat_data().display());
            println!("prefix          {}", layout.prefix().display());
            println!("battle.net      {}", layout.battle_net_dir().display());
            println!("games root      {}", layout.games_root().display());
            println!("wow install     {}", layout.wow_install().display());
            println!("wow symlink     {}", layout.wow_symlink().display());
            println!(".build.info     {}", layout.build_info().display());
            println!("config file     {}", dirs.config_file().display());
            println!("asset cache     {}", dirs.asset_cache_dir().display());
            println!("proton dir      {}", dirs.proton_dir().display());
        }
        Command::Config => {
            // Milestone 1 has no loader yet; show what a fresh install writes.
            print!("{}", Config::with_default_products().to_toml()?);
        }
        Command::Proton => {
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
        }
        Command::ProtonCheck => {
            use lightforge_core::net::{ETagCache, Http};
            use lightforge_core::proton;

            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            let cache = ETagCache::new(dirs.cache_dir().join("proton-releases.json"));
            let cached = cache.etag().is_some();

            let http = Http::new()?;
            let releases = proton::fetch_releases(&http, &cache)
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
                .filter(|r| r.flavor.is_supported())
                .collect();

            for build in &installed {
                match proton::check_for_update(&build.version, &releases) {
                    Some(n) => println!("  {} -> {} available", n.current, n.available),
                    None => println!("  {} is current", build.version),
                }
            }
        }
        Command::ProtonInstall { version } => {
            use lightforge_core::net::{ETagCache, Http};
            use lightforge_core::proton;
            use std::io::Write;

            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            let cache = ETagCache::new(dirs.cache_dir().join("proton-releases.json"));
            let http = Http::new()?;
            let releases = proton::fetch_releases(&http, &cache).await?;

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
        }
        Command::Prefix => {
            let status = PrefixStatus::inspect(&layout);
            println!("compat data     {}", status.compat_data.display());
            println!("prefix          {}", status.prefix.display());
            println!(
                "initialized     {}",
                if status.initialized { "yes" } else { "no" }
            );
            println!("symlink         {}", describe_link(&status.link));
            println!(
                "ready           {}",
                if status.is_ready() { "yes" } else { "no" }
            );
        }
        Command::PrefixInit { proton } => {
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
        }
        Command::PrefixLink => {
            let (link, target) = (layout.wow_symlink(), layout.wow_install());
            let before = lightforge_core::prefix::ensure_link(&link, &target)
                .with_context(|| format!("linking {} -> {}", link.display(), target.display()))?;
            println!("was             {}", describe_link(&before));
            println!("now             {} -> {}", link.display(), target.display());
        }
        Command::BattlenetFetch => {
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
        }
        Command::BattlenetSetup {
            proton,
            locale,
            installer,
            timeout,
        } => {
            use lightforge_core::battlenet::{self, Installer};
            use lightforge_core::config::Config;

            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            let installer_path =
                installer.unwrap_or_else(|| dirs.cache_dir().join(battlenet::INSTALLER_FILE));
            let user = Config::default().prefix_user;

            let (runner, build) = proton_runner(&layout, proton.as_deref())?;
            println!("using {} ({})", build.version, build.path.display());
            println!("installer {}", installer_path.display());

            let setup = Installer::new(&layout, &installer_path)
                .with_locale(locale)
                .with_timeout(std::time::Duration::from_secs(timeout));
            if !setup.is_installed() {
                println!("running setup — this takes a few minutes");
            }

            let report = battlenet::provision(&runner, &layout, &user, &installer_path)
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
        }
        Command::BattlenetConfig => {
            use lightforge_core::battlenet;
            use lightforge_core::config::Config;

            let user = Config::default().prefix_user;

            let tweaks = battlenet::default_tweaks();
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
        }
        Command::BattlenetRun { proton, product, exec } => {
            use lightforge_core::battlenet::{self, Launch};
            use lightforge_core::config::Config;
            use lightforge_core::preflight::{DownloadGuard, NoDownloads, Preflight};

            let launch = match &product {
                Some(code) => Launch::Product {
                    code: code.parse().expect("product codes are infallible"),
                    exec: exec.clone(),
                },
                None => Launch::Client,
            };

            let (runner, build) = proton_runner(&layout, proton.as_deref())?;
            let (exe, args) = launch.command(&layout.battle_net_dir());
            println!("using {} ({})", build.version, build.path.display());

            // Never spawn the client into a prefix that still has one running.
            let code = match &launch {
                Launch::Product { code, .. } => Some(code.clone()),
                Launch::Client => None,
            };
            let mut downloads = NoDownloads;
            let report = Preflight::new(Config::default().prefix_user)
                .run(&runner, &layout, code.as_ref(), &mut downloads)
                .context("pre-flight cleanup")?;
            print_preflight(&report);

            println!("running {}{}", exe.display(), if args.is_empty() { String::new() } else { format!(" {}", args.join(" ")) });
            let mut child = battlenet::launch(&runner, &layout, &launch)
                .context("launching Battle.net")?;
            let status = child.wait().context("waiting for Battle.net")?;

            if report.download_paused {
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
        }
        Command::Preflight { proton, product, dry_run, grace } => {
            use lightforge_core::config::Config;
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

            let report = Preflight::new(Config::default().prefix_user)
                .with_grace(std::time::Duration::from_secs(grace))
                .run(&runner, &layout, code.as_ref(), &mut NoDownloads)
                .context("pre-flight cleanup")?;
            print_preflight(&report);
        }
        Command::Idx { path } => {
            use lightforge_core::tact::idx::{self, Index};

            // Accept either spelling so this is usable against an install root
            // or a bare data directory.
            let dir = if path.join("Data").join("data").is_dir() {
                path.join("Data").join("data")
            } else {
                path.clone()
            };

            let files = idx::discover(&dir).with_context(|| format!("scanning {}", dir.display()))?;
            if files.is_empty() {
                println!("no .idx files under {}", dir.display());
                return Ok(());
            }

            let (mut exact, mut differing) = (0usize, 0usize);
            let mut entries_total = 0usize;
            for file in &files {
                let raw = std::fs::read(file)?;
                let name = file.file_name().unwrap_or_default().to_string_lossy();

                match Index::parse(&raw) {
                    Ok(index) => {
                        // Both hashes already verified by parse; this proves we
                        // can reproduce the file Blizzard wrote, byte for byte.
                        let round = index.to_bytes()?;
                        let identical = round == raw;
                        if identical { exact += 1 } else { differing += 1 }
                        entries_total += index.entries().len();
                        println!(
                            "{name}  bucket {:02x}  v{}  {:>7} files  {:>4} markers  {:>5} free  {}",
                            index.bucket(),
                            index.version(),
                            index.files().count(),
                            index.entries().len() - index.files().count(),
                            index.remaining_capacity(),
                            if identical { "round-trips exactly" } else { "DIFFERS" },
                        );
                    }
                    Err(e) => {
                        differing += 1;
                        println!("{name}  FAILED: {e}");
                    }
                }
            }
            println!(
                "\n{} files, {entries_total} entries; {exact} round-trip byte-for-byte, {differing} do not",
                files.len()
            );
        }
        Command::BuildInfoSet { product, build_key, cdn_key, version } => {
            use lightforge_core::buildinfo::BuildInfoError;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let path = layout.build_info();

            // Read-modify-write, atomically. The caller is responsible for
            // having run pre-flight first — this is the file Agent watches.
            let info = BuildInfo::update(&path, |info| {
                info.set_build(&code, &build_key, &cdn_key, &version)
            })
            .map_err(|e: BuildInfoError| e)
            .with_context(|| format!("rewriting {}", path.display()))?;

            println!("{}", path.display());
            println!("{}", info.row(&code).expect("the row just written").cells().join(" | "));
        }
        Command::BuildInfo { product } => {
            let code: ProductCode = product.parse().expect("product codes are infallible");
            let path = layout.build_info();
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let info = BuildInfo::parse(&contents)
                .with_context(|| format!("parsing {}", path.display()))?;

            let row = info
                .row(&code)
                .with_context(|| format!("`{code}` has no row in {}", path.display()))?;
            println!("{}", row.cells().join(" | "));
        }
    }

    Ok(())
}

/// Render what a pre-flight pass did, one aligned line per fact.
fn print_preflight(report: &lightforge_core::preflight::Report) {
    use lightforge_core::preflight::Disposition;

    if report.processes.is_empty() {
        println!("processes       none running");
    }
    for outcome in &report.processes {
        let how = match outcome.disposition {
            Disposition::Wineserver => "wineserver -k",
            Disposition::Terminated => "SIGTERM",
            Disposition::Killed => "SIGKILL",
            Disposition::Survived => "SURVIVED",
        };
        println!(
            "processes       {:<24} pid {:<8} {how}",
            outcome.process.name, outcome.process.pid
        );
    }

    for (label, bytes) in [
        ("agent cache", report.agent_cache_removed),
        ("staging", report.staging_removed),
    ] {
        match bytes {
            Some(n) => println!("{label:<15} removed, {:.0} MB reclaimed", n as f64 / 1_048_576.0),
            None => println!("{label:<15} absent"),
        }
    }

    for config in &report.configs {
        println!("config          re-applied {}", config.display());
    }
    if report.download_paused {
        println!("download        paused for the session");
    }
}

/// Render a config tweak list: key, value, and why it is set.
fn print_tweaks(tweaks: &[lightforge_core::battlenet::Tweak]) {
    for tweak in tweaks {
        println!("  {:<40} {}\n    {}", tweak.key(), tweak.value, tweak.reason);
    }
}

/// Resolve the Proton build to run against the shared prefix, and a runner for
/// it. `None` selects the newest supported build installed.
fn proton_runner(
    layout: &Layout,
    version: Option<&str>,
) -> Result<(
    lightforge_core::prefix::ProtonRunner,
    lightforge_core::proton::ProtonRelease,
)> {
    use lightforge_core::prefix::ProtonRunner;
    use lightforge_core::proton;

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let build = proton::select(&builds, version).with_context(|| match version {
        Some(tag) => format!("no installed Proton build tagged `{tag}`"),
        None => {
            "no supported Proton-GE build installed — run `lightforge proton-install`".to_owned()
        }
    })?;

    let runner = ProtonRunner::new(
        build,
        layout,
        ProtonRunner::steam_client_path(steam.as_ref(), &dirs),
    );
    Ok((runner, build.clone()))
}

/// One-line rendering of the symlink's state for the CLI.
fn describe_link(state: &LinkState) -> String {
    match state {
        LinkState::Correct => "ok".to_owned(),
        LinkState::Missing => "missing".to_owned(),
        LinkState::Dangling(t) => format!("dangling -> {}", t.display()),
        LinkState::Mislinked(t) => format!("points at {}", t.display()),
        LinkState::OccupiedEmpty => "empty directory in the way".to_owned(),
        LinkState::Occupied => "a real directory with files in it".to_owned(),
    }
}
