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
    /// Show the `.build.info` row for a product.
    BuildInfo {
        /// Product code, e.g. `wow` or `wow_classic`.
        #[arg(default_value = "wow")]
        product: String,
    },
}

fn main() -> Result<()> {
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
            let steam = lightforge_core::steam::SteamRoot::discover()
                .context("no Steam installation found")?;
            println!("steam root   {}", steam.path().display());
            for lib in steam.library_paths() {
                println!("library      {}", lib.display());
            }
            println!();

            let installed = lightforge_core::proton::discover_installed(&steam);
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
