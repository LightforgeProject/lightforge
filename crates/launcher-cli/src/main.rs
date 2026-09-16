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
    /// Show the `.build.info` row for a product.
    BuildInfo {
        /// Product code, e.g. `wow` or `wow_classic`.
        #[arg(default_value = "wow")]
        product: String,
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
                let mark = if release.flavor.is_supported() { "*" } else { " " };
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

            println!("{} releases ({})", releases.len(), if cached { "revalidated" } else { "fresh" });
            if let Some(latest) = proton::latest_stable(&releases) {
                let size = latest.tarball().map(|a| a.size).unwrap_or(0);
                println!("latest stable  {}  ({:.0} MB)", latest.tag_name, size as f64 / 1_048_576.0);
                println!("checksum       {}", latest.tarball().and_then(|t| latest.checksum_for(t)).map(|a| a.name.as_str()).unwrap_or("MISSING"));
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
            let installed = proton::install_release(
                &http,
                release,
                &dest,
                dirs.cache_dir(),
                |done, total| {
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
                },
            )
            .await?;

            println!("\nverified and extracted");
            println!("  {} at {}", installed.version, installed.path.display());
        }
        Command::BuildInfo { product } => {
            let code: ProductCode = product.parse().expect("product codes are infallible");
            let path = layout.build_info();
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let info = BuildInfo::parse(&contents).with_context(|| format!("parsing {}", path.display()))?;

            let row = info
                .row(&code)
                .with_context(|| format!("`{code}` has no row in {}", path.display()))?;
            println!("{}", row.cells().join(" | "));
        }
    }

    Ok(())
}
