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
    /// Launch the game directly under Proton — the `[ PLAY ]` path.
    ///
    /// Runs pre-flight first, then starts `Wow.exe` through the prefix's
    /// Windows-visible path. Battle.net is not involved.
    Play {
        /// Product to launch, e.g. `wow` or `wow_classic`.
        #[arg(long, default_value = "wow")]
        product: String,
        /// Proton build to launch under.
        #[arg(long)]
        proton: Option<String>,
        /// Print what would run and exit.
        #[arg(long)]
        dry_run: bool,
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
    /// Rebuild an install's CASC store into a scratch copy, then verify it.
    ///
    /// The source is only ever read. Used to prove that a store Lightforge
    /// wrote is byte-for-byte usable in place of one Agent wrote.
    CascRebuild {
        /// Existing install to read from.
        src: PathBuf,
        /// Directory to build the rebuilt install in. Must not be inside src.
        dst: PathBuf,
        /// Rebuild only this fraction of entries, for a quick smoke test.
        #[arg(long, default_value_t = 100)]
        percent: u8,
    },
    /// Compare a rebuilt CASC store against the install it was built from.
    CascVerify {
        /// The original install.
        src: PathBuf,
        /// The rebuilt copy.
        dst: PathBuf,
        /// How many entries to compare, spread across the whole store.
        #[arg(long, default_value_t = 20000)]
        sample: usize,
    },
    /// Read an archive's index and pull real files out of it by range request.
    CdnArchive {
        /// Archive hash. Defaults to the first archive in the CDN config.
        archive: Option<String>,
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long, default_value = "eu")]
        region: String,
        /// How many entries to fetch and verify.
        #[arg(long, default_value_t = 5)]
        fetch: usize,
        /// Local install to resolve content keys from, so downloads can be
        /// verified against the key that can actually check them.
        #[arg(long)]
        verify_with: Option<PathBuf>,
    },
    /// Fetch content from Blizzard's CDN and verify it by hash.
    CdnFetch {
        /// Product code, e.g. `wow`.
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long, default_value = "eu")]
        region: String,
        /// Hash to fetch. Defaults to the current build config.
        hash: Option<String>,
        /// Where the hash lives: `config`, `data` or `patch`.
        #[arg(long, default_value = "config")]
        kind: String,
        /// Compare the result against this local file, byte for byte.
        #[arg(long)]
        compare: Option<PathBuf>,
    },
    /// Download a product from Blizzard's CDN into a CASC store.
    Download {
        /// Install root to build. Created if absent.
        dest: PathBuf,
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long, default_value = "eu")]
        region: String,
        /// Locale to install, e.g. `enUS`.
        #[arg(long, default_value = "enUS")]
        locale: String,
        /// Flavor directory the install files go in.
        #[arg(long, default_value = "_retail_")]
        flavor: String,
        /// Plan and report, then stop without fetching any content.
        #[arg(long)]
        dry_run: bool,
        /// Only cache this many archive indices. Bounds a test run; a real
        /// install needs all of them, and the plan is incomplete without.
        #[arg(long)]
        limit_archives: Option<usize>,
        /// Only fetch this many runs. Bounds a test run.
        #[arg(long)]
        limit_runs: Option<usize>,
        /// Skip the loose files, which are the large ones.
        #[arg(long)]
        skip_loose: bool,
        /// Also extract the install manifest's files (Wow.exe and friends).
        #[arg(long)]
        install_files: bool,
        /// Requests in flight.
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
        /// Merge files separated by fewer than this many KiB into one request.
        #[arg(long, default_value_t = 64)]
        gap_kib: u32,
        /// Rewrite the indices every this many MiB. Bounds what an interrupted
        /// download has to fetch again.
        #[arg(long, default_value_t = 4096)]
        checkpoint_mib: u64,
    },
    /// Compare a local install against what Blizzard currently ships.
    UpdateCheck {
        /// Install root. Defaults to the configured games directory.
        path: Option<PathBuf>,
        /// Product code, e.g. `wow`.
        #[arg(long, default_value = "wow")]
        product: String,
        /// Blizzard region to ask.
        #[arg(long, default_value = "eu")]
        region: String,
    },
    /// Resolve files through an install's root and encoding manifests.
    Content {
        /// A WoW install root.
        path: PathBuf,
        /// Product whose build key to take from `.build.info`.
        #[arg(long, default_value = "wow")]
        product: String,
        /// Locale to filter the root manifest to (`any`, `enUS`, `deDE`, …).
        #[arg(long, default_value = "any")]
        locale: String,
        /// Read this file id and report what came back.
        #[arg(long)]
        fdid: Option<u32>,
        /// Read this game-relative path, e.g. `Interface/FrameXML/GlobalStrings.lua`.
        #[arg(long, conflicts_with = "fdid")]
        file: Option<String>,
    },
    /// Open an install's CASC store and decode a sample of its files.
    Casc {
        /// A WoW install root, or a `Data`/`Data/data` directory.
        path: PathBuf,
        /// How many files to decode, spread across the whole index.
        #[arg(long, default_value_t = 2000)]
        sample: usize,
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

/// Die quietly when output is piped into something that stops reading.
///
/// Rust sets `SIGPIPE` to `SIG_IGN` before `main`, so once the reader of a
/// pipe goes away — `lightforge idx … | head` being the obvious case — every
/// subsequent `println!` hits `EPIPE` and panics. Restoring the default
/// disposition makes the process exit the way every other Unix tool does,
/// with no backtrace and no partial-looking failure.
///
/// Safe to call here and nowhere else: it runs before any thread is spawned,
/// and it only resets a signal to what the kernel would have done anyway.
fn restore_sigpipe() {
    // SAFETY: `signal` with `SIG_DFL` is async-signal-safe and this is the
    // first thing main does, before any output or threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    restore_sigpipe();
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
        Command::Play { product, proton, dry_run } => {
            use lightforge_core::config::Config;
            use lightforge_core::preflight::{NoDownloads, Preflight};

            let code: ProductCode = product.parse().expect("product codes are infallible");

            // The game is launched through the symlink, so Wine sees the
            // conventional Windows path — the same thing that keeps CEF happy
            // for the in-game store.
            let flavor_dir = layout.wow_symlink().join(code.install_dir());
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
                .unwrap_or_else(|_| layout.wow_install());

            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            let (runner, build) = proton_runner(&layout, proton.as_deref())?;
            let runner = runner
                .with_compat_mount(&target)
                // Per-product shader cache: the prefix is shared, so pooling
                // it would mean a Proton swap for one product discarding
                // another's accumulated shaders.
                .with_shader_cache(dirs.shader_cache_dir(&code));

            println!("using {} ({})", build.version, build.path.display());
            println!("install {}", target.display());
            println!("running {}", exe.display());
            if dry_run {
                return Ok(());
            }

            let report = Preflight::new(Config::default().prefix_user)
                .run(&runner, &layout, Some(&code), &mut NoDownloads)
                .context("pre-flight cleanup")?;
            print_preflight(&report);

            let status = runner
                .command("run", &exe, Vec::<String>::new())
                .status()
                .context("launching the game")?;
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
        Command::CascVerify { src, dst, sample } => {
            use lightforge_core::tact::entry::Header;
            use lightforge_core::tact::storage::Storage;

            let mut source = Storage::open(&src).context("opening the source store")?;
            let mut rebuilt = Storage::open(&dst).context("opening the rebuilt store")?;
            let (a, b) = (source.stats(), rebuilt.stats());
            println!("source   {:>9} files, {:>9} entries, {:.1} GB", a.files, a.entries, a.bytes as f64 / 1e9);
            println!("rebuilt  {:>9} files, {:>9} entries, {:.1} GB", b.files, b.entries, b.bytes as f64 / 1e9);
            if a.files != b.files {
                println!("NOTE: file counts differ by {}", a.files.abs_diff(b.files));
            }

            let entries: Vec<_> = rebuilt.files().copied().collect();
            let stride = (entries.len() / sample.max(1)).max(1);
            let chosen: Vec<_> = entries.iter().step_by(stride).take(sample).copied().collect();
            println!("\ncomparing {} entries…", chosen.len());

            let t = std::time::Instant::now();
            let (mut same, mut differ, mut both_enc, mut hdr_ok, mut hdr_bad) =
                (0usize, 0usize, 0usize, 0usize, 0usize);
            for e in &chosen {
                // The header must verify at the position we chose for it.
                match rebuilt.read_raw(e) {
                    Ok(raw) if Header::verify(&raw, e.archive_id, e.offset) => hdr_ok += 1,
                    _ => hdr_bad += 1,
                }

                let mut ekey = [0u8; 16];
                ekey[..9].copy_from_slice(&e.key);
                match (source.read(&ekey), rebuilt.read(&ekey)) {
                    (Ok(x), Ok(y)) if x == y => same += 1,
                    // Encrypted content fails to decode in both, which is
                    // agreement, not damage.
                    (Err(_), Err(_)) => both_enc += 1,
                    _ => {
                        differ += 1;
                        if differ <= 3 {
                            println!("  MISMATCH at archive {} offset {}", e.archive_id, e.offset);
                        }
                    }
                }
            }

            println!(
                "\ndecoded identically: {same}\nundecodable in both:  {both_enc}\nmismatched:           {differ}"
            );
            println!("headers verifying:    {hdr_ok} ok, {hdr_bad} bad");
            println!("compared in {:.0}s", t.elapsed().as_secs_f64());
            if differ == 0 && hdr_bad == 0 {
                println!("\nthe rebuilt store is byte-equivalent to the source for every entry checked");
            } else {
                anyhow::bail!("the rebuilt store does not match the source");
            }
        }
        Command::CascRebuild { src, dst, percent } => {
            use lightforge_core::tact::storage::Storage;
            use lightforge_core::tact::writer::StoreWriter;

            if dst.starts_with(&src) {
                anyhow::bail!("dst must not be inside src — the source is read-only");
            }
            let dst_data = dst.join("Data").join("data");
            if dst_data.exists() {
                anyhow::bail!("{} already exists; remove it first", dst_data.display());
            }

            // Everything outside the CASC store is small and copied verbatim:
            // binaries, WTF, Data/config, and .build.info.
            println!("copying non-CASC files…");
            let t = std::time::Instant::now();
            let status = std::process::Command::new("rsync")
                .arg("-a")
                .arg("--exclude=Data/data/")
                .arg(format!("{}/", src.display()))
                .arg(format!("{}/", dst.display()))
                .status()
                .context("running rsync — is it installed?")?;
            anyhow::ensure!(status.success(), "rsync failed");
            std::fs::create_dir_all(&dst_data)?;
            println!("  done in {:?}", t.elapsed());

            let mut source = Storage::open(&src).context("opening the source store")?;
            let mut entries: Vec<_> = source.files().copied().collect();
            // Read in physical order: the index is sorted by key, which would
            // make this a random-access pass over 123 GB.
            entries.sort_by_key(|e| (e.archive_id, e.offset));
            if percent < 100 {
                let keep = entries.len() * usize::from(percent.max(1)) / 100;
                entries.truncate(keep);
            }
            println!("rebuilding {} entries", entries.len());

            // Carry the source's index generation numbers over. A real
            // install has exactly one generation per bucket in the 0x32-0x38
            // range; starting from zero produces a store the client does not
            // treat the same way.
            let mut writer = StoreWriter::create(&dst_data)?;
            let src_data = source.data_dir().to_path_buf();
            for bucket in 0..lightforge_core::tact::idx::BUCKETS {
                if let Some(seq) = lightforge_core::tact::idx::latest_sequence(&src_data, bucket)? {
                    writer = writer.with_index_generation(bucket, seq);
                }
            }

            let t = std::time::Instant::now();
            let (mut done, mut failed) = (0usize, 0usize);
            let mut last_report = std::time::Instant::now();

            for entry in &entries {
                let raw = match source.read_raw(entry) {
                    Ok(r) => r,
                    Err(e) => {
                        failed += 1;
                        if failed <= 3 { println!("  read failed: {e}"); }
                        continue;
                    }
                };
                // Pad the 9-byte index key back to a full key: the writer uses
                // only the first nine bytes, which is what headers store.
                let mut ekey = [0u8; 16];
                ekey[..9].copy_from_slice(&entry.key);
                writer.write(&ekey, &raw[lightforge_core::tact::idx::CASC_ENTRY_HEADER..])?;
                done += 1;

                if last_report.elapsed().as_secs() >= 5 {
                    let secs = t.elapsed().as_secs_f64();
                    println!(
                        "  {done}/{} ({:.0}%)  {:.1} GB  {:.0} MB/s",
                        entries.len(),
                        done as f64 / entries.len() as f64 * 100.0,
                        writer.bytes_written() as f64 / 1e9,
                        writer.bytes_written() as f64 / 1e6 / secs.max(0.001),
                    );
                    last_report = std::time::Instant::now();
                }
            }

            let summary = writer.finish()?;
            let secs = t.elapsed().as_secs_f64();
            println!(
                "\nrebuilt {} files / {:.1} GB into {} archives + {} indices in {:.0}s ({:.0} MB/s)",
                summary.files,
                summary.bytes as f64 / 1e9,
                summary.archives,
                summary.indices.len(),
                secs,
                summary.bytes as f64 / 1e6 / secs.max(0.001),
            );
            if failed > 0 {
                println!("{failed} source entries could not be read");
            }
            println!("\nverify with:  lightforge content {} --locale enUS", dst.display());
        }
        Command::Download {
            dest,
            product,
            region,
            locale,
            flavor,
            dry_run,
            limit_archives,
            limit_runs,
            skip_loose,
            install_files,
            concurrency,
            gap_kib,
            checkpoint_mib,
        } => {
            use lightforge_core::net::Http;
            use lightforge_core::tact::archive::FileIndex;
            use lightforge_core::tact::cdn::Client;
            use lightforge_core::tact::config::{BuildConfig, CdnConfig};
            use lightforge_core::tact::download::{
                extract_install, Cancel, Catalog, Downloader, Options as DlOptions, Source,
            };
            use lightforge_core::tact::encoding::Encoding;
            use lightforge_core::tact::indices::{Fetched, IndexCache, Locations};
            use lightforge_core::tact::manifest::{DownloadManifest, InstallManifest, TagSet};
            use lightforge_core::tact::plan::{reverse_encoding, Options as PlanOptions, Plan};
            use lightforge_core::tact::storage::{EKey, Storage};
            use lightforge_core::tact::version::{self, Region};
            use lightforge_core::tact::writer::StoreWriter;
            use std::collections::HashSet;
            use std::time::Instant;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let region: Region = region.parse().context("parsing --region")?;
            let gib = |b: u64| b as f64 / 1024f64.powi(3);

            // The tag expression a `.build.info` would carry for this machine.
            let tag_column = format!(
                "Windows x86_64 {r} {locale} speech:Windows x86_64 {r} {locale} text",
                r = region.as_str().to_uppercase()
            );
            let tags = TagSet::parse(&tag_column);

            let http = Http::new()?;
            let versions = version::versions(&http, region, code.as_str()).await?;
            let remote = version::for_region(&versions, region, |v| &v.region)
                .context("the version table is empty")?;
            let cdns = version::cdns(&http, region, code.as_str()).await?;
            let cdn = version::for_region(&cdns, region, |c| &c.name)
                .context("the CDN table is empty")?;
            let client = Client::from_cdn(cdn)?;
            println!("build           {} ({})", remote.versions_name, remote.build_config);

            let build = BuildConfig::parse(
                &client.fetch_config(&http, &remote.build_config).await
                    .context("fetching the build config")?,
            )?;
            let cdn_config = CdnConfig::parse(
                &client.fetch_config(&http, &remote.cdn_config).await
                    .context("fetching the CDN config")?,
            )?;
            println!("archives        {}", cdn_config.archives.len());

            // Manifests are the loose kind: one plain GET, decoded and checked
            // against the content key the build config names them by.
            let fetch_manifest = |pair: Option<lightforge_core::tact::config::KeyPair>, what: &'static str| {
                let client = &client;
                let http = &http;
                async move {
                    let pair = pair.with_context(|| format!("the build config has no {what}"))?;
                    let ekey = pair.ekey.with_context(|| format!("{what} has no encoding key"))?;
                    let decoded = client
                        .fetch_loose(http, &ekey, &pair.ckey, None)
                        .await
                        .with_context(|| format!("fetching the {what}"))?;
                    anyhow::Ok(decoded)
                }
            };

            let t = Instant::now();
            let encoding = Encoding::parse(&fetch_manifest(Some(build.encoding), "encoding manifest").await?)?;
            println!("encoding        {} entries in {:.1?}", encoding.len(), t.elapsed());

            let download = DownloadManifest::parse(&fetch_manifest(build.download, "download manifest").await?)?;
            let install = InstallManifest::parse(&fetch_manifest(build.install, "install manifest").await?)?;
            println!("download        {} entries, {} tags", download.len(), download.tags().len());
            println!("install         {} entries", install.len());
            println!("tags            {:?}", tags.unknown_names(download.tags()));

            let wanted = download.select(&tags);
            println!(
                "wanted          {} files, {:.1} GiB",
                wanted.len(),
                gib(wanted.iter().map(|e| e.size).sum())
            );

            // Cache the archive indices. Without these nothing can be located.
            let cache = IndexCache::new(dest.join("Data").join("indices"));
            let mut archives = cdn_config.archive_indices();
            if let Some(limit) = limit_archives {
                archives.truncate(limit);
                println!("\n(limited to {limit} archive indices — the plan will be incomplete)");
            }

            let t = Instant::now();
            let mut last = 0usize;
            let report = cache
                .ensure(&http, &client, &archives, concurrency, |step| {
                    if step.done - last >= 100 || step.done == step.total {
                        last = step.done;
                        println!("  indices {}/{}", step.done, step.total);
                    }
                    if let Fetched::Failed { detail } = &step.outcome {
                        eprintln!("  index {} failed: {detail}", step.hash);
                    }
                })
                .await?;
            println!(
                "indices         {} cached, {} downloaded ({:.1} MiB) in {:.1?}",
                report.cached,
                report.downloaded,
                report.bytes as f64 / 1048576.0,
                t.elapsed()
            );

            let file_index_hash = cdn_config.file_index.clone().context("no file index in the CDN config")?;
            if !cache.is_cached(&file_index_hash, cdn_config.file_index_size) {
                let bytes = client
                    .fetch_index(&http, &file_index_hash, cdn_config.file_index_size)
                    .await
                    .context("fetching the file index")?;
                std::fs::write(cache.path_of(&file_index_hash), &bytes)?;
            }
            let loose = FileIndex::parse(&std::fs::read(cache.path_of(&file_index_hash))?)?;
            println!("loose files     {}", loose.len());

            // Locate every wanted key.
            let wanted_keys: HashSet<EKey> = wanted.iter().map(|e| e.ekey).collect();
            let ckeys = reverse_encoding(&encoding, &wanted_keys);
            let t = Instant::now();
            let mut locations = Locations::default();
            for (archive, _) in &archives {
                if cache.is_cached(archive, None) {
                    locations.absorb(&cache.load_archive(archive)?, &wanted_keys);
                }
            }
            println!("located         {} of {} in {:.1?}", locations.len(), wanted.len(), t.elapsed());

            // Skip whatever the store already holds, which is what makes an
            // interrupted download resume instead of starting over.
            let present: HashSet<EKey> = match Storage::open(&dest) {
                Ok(store) => store.files().map(|e| {
                    let mut k = [0u8; 16];
                    k[..9].copy_from_slice(&e.key);
                    k
                }).collect(),
                Err(_) => HashSet::new(),
            };
            let have = |ekey: &EKey| {
                let mut probe = [0u8; 16];
                probe[..9].copy_from_slice(&ekey[..9]);
                present.contains(&probe)
            };

            let mut plan = Plan::build(
                &wanted,
                &locations,
                &loose,
                &ckeys,
                have,
                PlanOptions { max_gap: gap_kib * 1024, ..PlanOptions::default() },
            );
            if skip_loose {
                plan.loose.clear();
            }
            if let Some(limit) = limit_runs {
                plan.runs.truncate(limit);
            }

            println!(
                "\nplan            {} requests, {} files, {:.2} GiB ({:.2} GiB padding)",
                plan.requests(),
                plan.files(),
                gib(plan.bytes()),
                gib(plan.padding())
            );
            println!("already present {}", plan.already_present);
            if !plan.missing.is_empty() {
                println!("UNLOCATABLE     {} keys", plan.missing.len());
            }

            if dry_run {
                println!("\n--dry-run: nothing fetched");
                return Ok(());
            }

            // Fetch.
            let data_dir = dest.join("Data").join("data");
            let mut writer = StoreWriter::create(&data_dir)?;
            let cancel = Cancel::new();
            // The reader always carries a key service; the downloader must too,
            // or blte cannot even name a missing key properly. Built-in keys
            // decrypt some content outright, and anything still missing a key
            // is stored unverified rather than refused.
            let key_service = lightforge_core::tact::download::default_key_service();
            let source = Source { http: &http, client: &client, keys: Some(&key_service) };
            let downloader = Downloader::new(DlOptions {
                concurrency,
                checkpoint_bytes: checkpoint_mib * 1024 * 1024,
            });

            let t = Instant::now();
            let mut last_report = Instant::now();
            let outcome = downloader
                .run(source, &plan, &mut writer, &cancel, |p| {
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
                            mb / t.elapsed().as_secs_f64().max(0.001)
                        );
                    }
                })
                .await?;

            println!(
                "\nstored          {} files, {:.2} GiB in {:.1?}",
                outcome.files_stored,
                gib(outcome.bytes_stored),
                t.elapsed()
            );
            println!("unverified      {} (encrypted or not in encoding)", outcome.unverified);
            for failure in outcome.failed.iter().take(10) {
                eprintln!(
                    "  FAILED {} — {}",
                    lightforge_core::tact::config::to_hex(&failure.ekey),
                    failure.reason
                );
            }
            if outcome.failed.len() > 10 {
                eprintln!("  ... and {} more", outcome.failed.len() - 10);
            }

            if install_files {
                let selected = install.select(&tags);
                println!("\ninstall files   {} selected", selected.len());
                let catalog = Catalog { encoding: &encoding, locations: &locations, loose: &loose };
                let report = extract_install(
                    source,
                    &selected,
                    catalog,
                    &dest.join(&flavor),
                    &cancel,
                    |e| println!("  {} ({:.1} MiB)", e.path.display(), e.bytes as f64 / 1048576.0),
                )
                .await?;
                println!(
                    "extracted       {} files, {:.1} MiB",
                    report.written.len(),
                    report.bytes() as f64 / 1048576.0
                );

                // Agent writes this and the install manifest does not list it;
                // without it the client cannot tell which product it is.
                let flavor_file = lightforge_core::tact::download::write_flavor_info(
                    &dest.join(&flavor),
                    &code,
                )?;
                println!("flavor          {}", flavor_file.display());
                for (path, reason) in &report.failed {
                    eprintln!("  FAILED {path} — {reason}");
                }
            }

            // `.build.info` last: it is what Agent reads to decide the install
            // is present and current, so it must not claim a build whose
            // content is still arriving.
            let info_path = dest.join(".build.info");
            let mut info = match BuildInfo::load(&info_path) {
                Ok(existing) => existing,
                Err(_) => BuildInfo::with_default_schema(),
            };
            info.set_product(
                &code,
                &[
                    ("Branch", region.as_str()),
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
            info.save(&info_path)?;
            println!("\n.build.info     {} ({})", info_path.display(), remote.versions_name);

            let summary = writer.finish()?;
            println!(
                "\nstore           {} archives, {} files, {:.2} GiB",
                summary.archives,
                summary.files,
                gib(summary.bytes)
            );
            println!("indices         {} written", summary.indices.len());
            println!("\nverify with:  lightforge casc {}", dest.display());
        }
        Command::CdnArchive { archive, product, region, fetch, verify_with } => {
            use lightforge_core::net::Http;
            use lightforge_core::tact::archive::ArchiveIndex;
            use lightforge_core::tact::cdn::{Client, Kind};
            use lightforge_core::tact::config::{to_hex, BuildConfig, CdnConfig};
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
            let raw = http.get_bytes(&url).await.context("fetching the archive index")?;
            println!("index           {} bytes from {url}", raw.len());

            let index = ArchiveIndex::parse(&raw, &archive).context("parsing the archive index")?;
            println!("entries         {}", index.len());
            println!("archived        {:.1} MB", index.bytes() as f64 / 1e6);

            // Range-request real files and verify each against its CONTENT
            // key — the encoding key names the bytes but cannot check them.
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
                            to_hex(&located.ekey),
                            to_hex(&ckey),
                            decoded.len()
                        );
                    }
                    Err(e) => {
                        bad += 1;
                        println!("  {} FAILED: {e}", to_hex(&located.ekey));
                    }
                }
            }
            println!("\nverified {ok}, failed {bad}, not in local encoding {unknown}");
        }
        Command::CdnFetch { product, region, hash, kind, compare } => {
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
        }
        Command::UpdateCheck { path, product, region } => {
            use lightforge_core::net::Http;
            use lightforge_core::tact::version::{self, Region, Status};

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let region: Region = region.parse().context("parsing --region")?;
            let install = path.unwrap_or_else(|| layout.wow_install());

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
        }
        Command::Content { path, product, locale, fdid, file } => {
            use lightforge_core::tact::config::BuildConfig;
            use lightforge_core::tact::content::{Content, Locale};
            use lightforge_core::tact::storage::Storage;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let locale: Locale = locale.parse().context("parsing --locale")?;

            // The build key is already recorded in .build.info, which is the
            // file M3's writer maintains — this is where the two halves meet.
            let info = BuildInfo::load(path.join(".build.info"))
                .with_context(|| format!("reading {}", path.join(".build.info").display()))?;
            let build_key = info.get(&code, lightforge_core::buildinfo::COL_BUILD_KEY)?;
            println!("build key       {build_key}");

            let build = BuildConfig::load(path.join("Data").join("config"), build_key)
                .context("loading the build config")?;
            println!("build name      {}", build.build_name.as_deref().unwrap_or("(unnamed)"));

            let mut storage = Storage::open(&path).context("opening the CASC store")?;
            let t = std::time::Instant::now();
            let content = Content::open(&mut storage, build, locale)
                .context("loading the encoding and root manifests")?;
            println!("manifests       loaded in {:?}", t.elapsed());
            println!("content keys    {}", content.encoded_count());
            println!("file ids        {}", content.file_count());

            // Root lists every file in the build; local storage holds only what
            // was actually downloaded for this install's locale and options.
            // The gap is expected, and measuring it is the basis of a future
            // verify/repair pass.
            let sample = 20_000usize;
            let stride = (content.file_count() / sample).max(1);
            let (mut present, mut checked) = (0usize, 0usize);
            for id in content.file_ids().step_by(stride).take(sample) {
                checked += 1;
                if content.ekey_for_fdid(id).is_some_and(|k| storage.contains(&k)) {
                    present += 1;
                }
            }
            println!(
                "installed       {present}/{checked} sampled ({:.0}% of the build)",
                present as f64 / checked.max(1) as f64 * 100.0
            );

            let target = match (&fdid, &file) {
                (Some(id), _) => Some(*id),
                (None, Some(path)) => Some(
                    content
                        .fdid_for_path(path)
                        .with_context(|| format!("no file named `{path}` in this build"))?,
                ),
                // Nothing asked for: prove the chain works on the first file
                // that is actually present locally. The first file id overall
                // is usually *not* installed, since root covers the whole
                // build rather than this install's subset.
                (None, None) => content
                    .file_ids()
                    .find(|id| content.ekey_for_fdid(*id).is_some_and(|k| storage.contains(&k))),
            };

            if let Some(id) = target {
                let ckey = content.ckey_for_fdid(id).context("file id has no content key")?;
                let ekey = content.ekey_for_fdid(id).context("content key has no encoding key")?;
                let bytes = content.read_fdid(&mut storage, id).context("reading the file")?;
                println!("\nfile id         {id}");
                println!("content key     {}", lightforge_core::tact::config::to_hex(&ckey));
                println!("encoding key    {}", lightforge_core::tact::config::to_hex(&ekey));
                println!("expected size   {}", content.size_of(&ckey).unwrap_or(0));
                println!("decoded size    {}", bytes.len());
                let magic: String = bytes.iter().take(4).map(|b| {
                    if b.is_ascii_graphic() { *b as char } else { '.' }
                }).collect();
                println!("magic           {magic:?}");
            }
        }
        Command::Casc { path, sample } => {
            use lightforge_core::tact::storage::{Storage, StorageError};

            let t = std::time::Instant::now();
            let mut storage = Storage::open(&path)
                .with_context(|| format!("opening the CASC store at {}", path.display()))?;
            let stats = storage.stats();
            println!("store           {}", storage.data_dir().display());
            println!("buckets         {}", stats.buckets);
            println!("files           {}", stats.files);
            println!("archive markers {}", stats.entries - stats.files);
            println!("archived        {:.1} GB", stats.bytes as f64 / 1e9);
            println!("opened in       {:?}\n", t.elapsed());

            // Stride the sample so a single bad archive can't pass as healthy.
            let entries: Vec<_> = storage.files().copied().collect();
            let stride = (entries.len() / sample.max(1)).max(1);
            let chosen: Vec<_> = entries.iter().step_by(stride).take(sample).collect();

            let (mut ok, mut encrypted, mut failed) = (0usize, 0usize, 0usize);
            let mut bytes = 0u64;
            let t = std::time::Instant::now();
            for entry in &chosen {
                match storage.read_entry(entry) {
                    Ok(d) => {
                        ok += 1;
                        bytes += d.len() as u64;
                    }
                    // Unreleased content ships encrypted; without the key this
                    // is expected on a healthy install, not damage.
                    Err(StorageError::Decode { source, .. })
                        if source.to_string().to_lowercase().contains("key") =>
                    {
                        encrypted += 1
                    }
                    Err(e) => {
                        failed += 1;
                        if failed <= 3 {
                            println!("  {e}");
                        }
                    }
                }
            }
            let secs = t.elapsed().as_secs_f64();
            println!(
                "decoded {ok}/{} ({encrypted} encrypted, {failed} failed) — {:.1} MB in {:.2}s ({:.0} MB/s)",
                chosen.len(),
                bytes as f64 / 1e6,
                secs,
                bytes as f64 / 1e6 / secs.max(0.001),
            );
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
