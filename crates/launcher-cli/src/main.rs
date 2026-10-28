//! Headless consumer of `lightforge-core`.
//!
//! For inspecting core without the GUI, on a machine with no display.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lightforge_core::backup;
use lightforge_core::buildinfo::BuildInfo;
use lightforge_core::config::Config;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::prefix::{LinkState, PrefixStatus};
use lightforge_core::ProductCode;

mod cli;
mod commands;

use cli::{Cli, Command};

/// Die quietly when output is piped into something that stops reading
/// (`lightforge idx … | head`): Rust ignores `SIGPIPE`, so every later
/// `println!` would panic on `EPIPE`. Called first thing in `main`, before
/// any thread exists.
fn restore_sigpipe() {
    // SAFETY: `signal` with `SIG_DFL` is async-signal-safe and this is the
    // first thing main does, before any output or threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    use lightforge_core::advice::{advise, Fix};

    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // What returning the error from `main` printed, then what to do about
        // it, when that is known.
        Err(error) => {
            eprintln!("Error: {error:?}");
            tracing::error!("{}", lightforge_core::error::chain(error.as_ref()));
            if let Some(advice) = advise(error.as_ref()) {
                eprintln!("\nhint: {}", advice.todo);
                match advice.fix {
                    Some(Fix::CopyCommand(command)) => eprintln!("      {command}"),
                    Some(Fix::DeepScan) => eprintln!("      lightforge verify --deep --repair"),
                    _ => {}
                }
            }
            std::process::ExitCode::FAILURE
        }
    }
}

/// What every command starts from: where things are, and the settings.
struct Ctx {
    layout: Layout,
    config: Config,
}

async fn run() -> Result<()> {
    restore_sigpipe();
    let cli = Cli::parse();

    // An unreadable config stops every command, as in the GUI.
    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let config = Config::load_or_default(&dirs.config_file())
        .with_context(|| format!("reading {}", dirs.config_file().display()))?;

    // `--root` overrides everything; otherwise the config decides, and only
    // then the default. The GUI resolves it through the same function.
    let layout = match &cli.root {
        Some(root) => Layout::with_root(root),
        None => config.layout().context("resolving the Blizzard root")?,
    };

    // Only to a file, never to the terminal the command is printing on. A
    // missing log is not worth failing a command for.
    if let Err(e) = lightforge_core::logging::init(&dirs, "lightforge-cli") {
        eprintln!("warning: logging disabled: {}", lightforge_core::error::chain(&e));
    }

    let ctx = Ctx { layout, config };
    match cli.command {
        Command::Paths => commands::info::paths(ctx),
        Command::Config => commands::info::config(),
        Command::Proton => commands::proton::proton(),
        Command::ProtonCheck => commands::proton::proton_check().await,
        Command::ProtonInstall { version } => commands::proton::proton_install(version).await,
        Command::Prefix => commands::proton::prefix(ctx),
        Command::PrefixInit { proton } => commands::proton::prefix_init(ctx, proton),
        Command::PrefixLink => commands::proton::prefix_link(ctx),
        Command::BattlenetFetch => commands::battlenet::battlenet_fetch().await,
        Command::BattlenetSetup {
            proton,
            locale,
            installer,
            timeout,
        } => commands::battlenet::battlenet_setup(ctx, proton, locale, installer, timeout),
        Command::BattlenetConfig => commands::battlenet::battlenet_config(ctx),
        Command::BattlenetRun { proton, product, exec, uri, install_game, skip_preflight } => commands::battlenet::battlenet_run(ctx, proton, product, exec, uri, install_game, skip_preflight),
        Command::Play { product, proton, dry_run } => commands::play::play(ctx, product, proton, dry_run),
        Command::Preflight { proton, product, dry_run, grace } => commands::play::preflight(ctx, proton, product, dry_run, grace),
        Command::CascVerify { src, dst, sample } => commands::inspect::casc_verify(src, dst, sample),
        Command::CascRebuild { src, dst, percent } => commands::inspect::casc_rebuild(src, dst, percent),
        Command::Download {
            dest,
            product,
            region,
            locale,
            flavor,
            dry_run,
            optional,
            limit_archives,
            limit_runs,
            skip_loose,
            install_files,
            concurrency,
            gap_kib,
            checkpoint_mib,
            cancel_after,
            show_install,
        } => commands::download::download(dest, product, region, locale, flavor, dry_run, optional, limit_archives, limit_runs, skip_loose, install_files, concurrency, gap_kib, checkpoint_mib, cancel_after, show_install).await,
        Command::CdnArchive { archive, product, region, fetch, verify_with } => commands::download::cdn_archive(archive, product, region, fetch, verify_with).await,
        Command::CdnFetch { product, region, hash, kind, compare } => commands::download::cdn_fetch(product, region, hash, kind, compare).await,
        Command::Plan {
            dest,
            product,
            region,
            locale,
            flavor,
            explain,
            optional,
            limit_archives,
            concurrency,
        } => commands::download::plan(dest, product, region, locale, flavor, explain, optional, limit_archives, concurrency).await,
        Command::AddonAdopt { product, path, ledger, catalog } => commands::addons::addon_adopt(ctx, product, path, ledger, catalog).await,
        Command::AddonFingerprints { product, path, catalog } => commands::addons::addon_fingerprints(ctx, product, path, catalog).await,
        Command::Addons { product, path, token } => commands::addons::addons(ctx, product, path, token),
        Command::AddonSearch { query, product, limit, catalog } => commands::addons::addon_search(query, product, limit, catalog).await,
        Command::AddonInstall { id, provider, product, catalog } => commands::addons::addon_install(ctx, id, provider, product, catalog).await,
        Command::AddonImport { token, product, catalog, pinned, dry_run } => commands::addons::addon_import(ctx, token, product, catalog, pinned, dry_run).await,
        Command::AddonUpdate { product, catalog } => commands::addons::addon_update(ctx, product, catalog).await,
        Command::BackupList { product } => commands::addons::backup_list(ctx, product),
        Command::BackupCreate { product } => commands::addons::backup_create(ctx, product),
        Command::BackupRestore { backup: which, product } => commands::addons::backup_restore(ctx, which, product),
        Command::AddonRemove { id, provider, product } => commands::addons::addon_remove(ctx, id, provider, product),
        Command::AddonToken { token } => commands::addons::addon_token(token),
        Command::Verify { path, deep, repair, all } => commands::storage::verify(ctx, path, deep, repair, all),
        Command::Reindex { path } => commands::storage::reindex(ctx, path),
        Command::Uninstall { product, path, yes } => commands::storage::uninstall(ctx, product, path, yes),
        Command::Reclaim { product, build_key, region, path, yes } => commands::storage::reclaim(ctx, product, build_key, region, path, yes).await,
        Command::Prune { path, yes } => commands::storage::prune(ctx, path, yes).await,
        Command::DropOptional { product, tag, keep, path, yes } => {
            commands::storage::drop_optional(ctx, product, tag, keep, path, yes).await
        }
        Command::Compact { path, yes } => commands::storage::compact(ctx, path, yes),
        Command::ProductDb { path, products, account_country, geo_country, agent_tags, dry_run } => commands::battlenet::product_db(ctx, path, products, account_country, geo_country, agent_tags, dry_run),
        Command::Setup { status, proton } => commands::proton::setup(ctx, status, proton).await,
        Command::Products { region, codes, all, concurrency } => commands::info::products(region, codes, all, concurrency).await,
        Command::Assets { product, source } => commands::info::assets(product, source).await,
        Command::Status { product, region, offline } => commands::info::status(ctx, product, region, offline).await,
        Command::UpdateCheck { path, product, region } => commands::info::update_check(ctx, path, product, region).await,
        Command::Content { path, product, locale, fdid, file } => commands::inspect::content(path, product, locale, fdid, file),
        Command::Casc { path, sample } => commands::inspect::casc(path, sample),
        Command::Idx { path } => commands::inspect::idx(path),
        Command::BuildInfoSet { product, build_key, cdn_key, version } => commands::inspect::build_info_set(ctx, product, build_key, cdn_key, version),
        Command::BuildInfo { product } => commands::inspect::build_info(ctx, product),
    }
}
