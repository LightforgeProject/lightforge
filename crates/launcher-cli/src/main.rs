//! Headless consumer of `lightforge-core`.
//!
//! Exists to exercise and inspect core without the GUI in the loop, useful
//! when debugging a prefix or a `.build.info` on a machine with no display.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lightforge_core::backup;
use lightforge_core::buildinfo::BuildInfo;
use lightforge_core::config::Config;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::prefix::{LinkState, PrefixStatus};
use lightforge_core::ProductCode;

mod commands;

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
        /// Proton build to use, e.g. `GE-Proton11-7`. Defaults to the tested
        /// build when it is installed, else the newest supported one.
        #[arg(long)]
        proton: Option<String>,
    },
    /// Create or repair only the game symlink, without running Proton.
    PrefixLink,
    /// Download Battle.net-Setup.exe into the cache.
    BattlenetFetch,
    /// Install Battle.net unattended, then apply registry and config defaults.
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
    #[command(hide = true)]
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
        /// Hand the client a `battlenet://` link instead, the way the system
        /// protocol handler does. Takes a game family, e.g. `WTCG`.
        #[arg(long, conflicts_with_all = ["product", "exec"])]
        uri: Option<String>,
        /// Open the client's install flow for an install uid, e.g. `w1r`.
        #[arg(long, conflicts_with_all = ["product", "exec", "uri"])]
        install_game: Option<String>,
        /// Skip pre-flight. For testing a hand-off against a client that is
        /// already running, without killing the session first.
        #[arg(long)]
        skip_preflight: bool,
    },
    /// Launch the game directly under Proton, the `[ PLAY ]` path.
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
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    #[command(hide = true)]
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
        /// Opt in to optional content by tag name, e.g. `HighRes`. Repeatable.
        #[arg(long)]
        optional: Vec<String>,
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
        /// Requests in flight. These transfers are request-latency bound, not
        /// bandwidth bound: 8 measured 9-10 MB/s where 32 measured 25.
        #[arg(long, default_value_t = 32)]
        concurrency: usize,
        /// Merge files separated by fewer than this many KiB into one request.
        #[arg(long, default_value_t = 64)]
        gap_kib: u32,
        /// Rewrite the indices every this many MiB. Bounds what an interrupted
        /// download has to fetch again.
        #[arg(long, default_value_t = 4096)]
        checkpoint_mib: u64,
        /// With `--dry-run`, list the install-manifest files this build would
        /// extract, and where each would land.
        #[arg(long)]
        show_install: bool,
        /// Trip the cancel token after this many seconds, the same path the
        /// GUI's pause button takes. For measuring how long a pause takes to
        /// come to rest.
        #[arg(long)]
        cancel_after: Option<u64>,
    },
    #[command(hide = true)]
    /// Explain a plan: what a build wants, what the store already has, and
    /// why the difference is the size it is. Fetches nothing.
    Plan {
        /// Install root to explain against.
        dest: PathBuf,
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long, default_value = "eu")]
        region: String,
        #[arg(long, default_value = "enUS")]
        locale: String,
        #[arg(long, default_value = "_retail_")]
        flavor: String,
        /// Break the difference down by priority, bucket and source, and
        /// cross-examine the store probe that produced it.
        #[arg(long)]
        explain: bool,
        /// Opt in to optional content by tag name, e.g. `HighRes`. Repeatable.
        #[arg(long)]
        optional: Vec<String>,
        /// Only cache this many archive indices. Bounds a test run.
        #[arg(long)]
        limit_archives: Option<usize>,
        #[arg(long, default_value_t = 32)]
        concurrency: usize,
    },
    #[command(hide = true)]
    /// Fingerprint each installed addon folder, and say what the catalog
    /// expects for the ones Lightforge installed.
    ///
    /// The check that the hashing is right: a folder we installed from
    /// CurseForge must hash to the number CurseForge publishes for it. A
    /// mismatch means we hash the wrong bytes, which is indistinguishable
    /// from "unknown addon" once matching starts.
    AddonFingerprints {
        #[arg(long, default_value = "wow")]
        product: String,
        /// Read this AddOns directory instead of the product's own.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Base URL of the addon catalog, or a fixture directory.
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Recognise hand-installed addons by their contents and manage them.
    AddonAdopt {
        #[arg(long, default_value = "wow")]
        product: String,
        /// Read this AddOns directory instead of the product's own.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Write the ledger here instead of beside the product's.
        #[arg(long)]
        ledger: Option<PathBuf>,
        /// Base URL of the addon catalog, or a fixture directory.
        #[arg(long)]
        catalog: Option<String>,
    },
    /// List the addons installed for a product.
    Addons {
        #[arg(long, default_value = "wow")]
        product: String,
        /// Read this AddOns directory instead of the product's own.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Reconcile against a WoWUp import token (or `@file`).
        #[arg(long)]
        token: Option<String>,
    },
    /// Search for addons across every provider that can answer.
    AddonSearch {
        /// What to search for, or `owner/repo` for a GitHub addon.
        query: String,
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long, default_value_t = 15)]
        limit: usize,
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Install an addon by provider and id.
    AddonInstall {
        /// Provider id: a number, a slug, or `owner/repo`.
        id: String,
        #[arg(long, default_value = "wowinterface")]
        provider: String,
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Install or update addons from a WoWUp import token.
    AddonImport {
        /// The token, or `@path` to read it from a file.
        token: String,
        #[arg(long, default_value = "wow")]
        product: String,
        /// Catalog base URL, or a fixture directory. Defaults to config.
        #[arg(long)]
        catalog: Option<String>,
        /// Install the exact files the export pinned, rather than the newest.
        #[arg(long)]
        pinned: bool,
        /// Resolve and report without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Update every addon Lightforge installed for a product.
    AddonUpdate {
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Remove an addon Lightforge installed.
    AddonRemove {
        /// Provider id, e.g. `2382`.
        id: String,
        #[arg(long, default_value = "curse")]
        provider: String,
        #[arg(long, default_value = "wow")]
        product: String,
    },
    /// List a game's `WTF/Account` backups, newest first.
    BackupList {
        #[arg(long, default_value = "wow")]
        product: String,
    },
    /// Back up a game's `WTF/Account` now. Skipped when nothing changed.
    BackupCreate {
        #[arg(long, default_value = "wow")]
        product: String,
    },
    /// Restore a backup over `WTF/Account`. The current state is backed up
    /// first, so a restore can be undone the same way.
    BackupRestore {
        /// The number `backup-list` shows (1 is the newest), or the file name.
        backup: String,
        #[arg(long, default_value = "wow")]
        product: String,
    },
    #[command(hide = true)]
    /// Decode a WoWUp import token and print what it contains.
    AddonToken {
        /// The token, or `@path` to read it from a file.
        token: String,
    },
    /// Verify every file in a store against its own entry headers, and
    /// optionally drop the damaged ones so a download refetches them.
    Verify {
        /// Install to check. Defaults to the configured one.
        path: Option<PathBuf>,
        /// Also hash every stored byte against its chunk table and key. Reads
        /// the whole store, minutes rather than seconds, and is safe while
        /// the game runs.
        #[arg(long)]
        deep: bool,
        /// Remove damaged entries from the indices. Requires a closed client.
        #[arg(long)]
        repair: bool,
        /// List every damaged entry rather than the first few.
        #[arg(long)]
        all: bool,
    },
    /// Rewrite a store's `.idx` files without touching its archives.
    ///
    /// Repairs an install whose indices were written with too little free
    /// space for the client's update section, the `NeedsRepair` case, in
    /// seconds, instead of re-downloading the whole product.
    Reindex {
        /// Install root. Defaults to the managed WoW install.
        path: Option<PathBuf>,
    },
    /// Remove an installed product. Prints the plan unless `--yes` is given.
    Uninstall {
        #[arg(long, default_value = "wow")]
        product: String,
        /// Install root. Defaults to the managed WoW install.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Actually delete. Without this, nothing is touched.
        #[arg(long)]
        yes: bool,
    },
    #[command(hide = true)]
    /// Give back the shared store's space held by one product's data: the
    /// files no other installed product uses. Prints the plan unless `--yes`.
    ///
    /// Uninstalling from the GUI does this itself. This is for a product
    /// removed before it could, pass the `--build-key` it had installed.
    Reclaim {
        #[arg(long)]
        product: String,
        /// Build config hash of the departing build. Defaults to the one
        /// `.build.info` lists for `--product`.
        #[arg(long)]
        build_key: Option<String>,
        #[arg(long, default_value = "eu")]
        region: String,
        /// Install root. Defaults to the managed WoW install.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Actually delete. Without this, nothing is touched.
        #[arg(long)]
        yes: bool,
    },
    #[command(hide = true)]
    /// Delete one optional download's files (`HighRes`, the texture pack) from
    /// a product's install and compact the archives. Prints the plan unless
    /// `--yes`.
    ///
    /// Only files that option alone brought in and no installed build needs.
    /// It does not change what is opted in to: the GUI does both together.
    DropOptional {
        #[arg(long)]
        product: String,
        /// The optional tag's name, as the manifest spells it.
        #[arg(long, default_value = "HighRes")]
        tag: String,
        /// Other options that stay on, so what they brought stays.
        #[arg(long)]
        keep: Vec<String>,
        /// Install root. Defaults to the managed WoW install.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Actually delete. Without this, nothing is touched.
        #[arg(long)]
        yes: bool,
    },
    /// Drop game data no installed or pre-installed version uses, then compact
    /// what that leaves. Prints the plan unless `--yes`.
    ///
    /// That data is mostly what earlier patches left behind. Never run it
    /// while a download is running or paused: what that has fetched so far
    /// belongs to no version yet, and would have to be fetched again.
    Prune {
        /// Install root. Defaults to the managed WoW install.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Actually drop and compact. Without this, nothing is touched.
        #[arg(long)]
        yes: bool,
    },
    /// Rewrite the store's archives that are mostly dead space, copying what
    /// still lives in them into fresh ones. Prints the plan unless `--yes`.
    ///
    /// Dead space is what dropped files leave behind: earlier patches, a
    /// repair, or a version uninstalled before uninstalling compacted.
    Compact {
        /// Install root. Defaults to the managed WoW install.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Actually rewrite. Without this, nothing is touched.
        #[arg(long)]
        yes: bool,
    },
    #[command(hide = true)]
    /// Write `.product.db` so Battle.net will adopt an existing install.
    ///
    /// Agent refuses to locate an install without this file (error 2800). The
    /// record is built from `.build.info`, so it cannot disagree with it.
    ProductDb {
        /// Install root. Defaults to the managed WoW install.
        path: Option<PathBuf>,
        /// Products to record. Defaults to every row in `.build.info`.
        #[arg(long = "product", value_name = "CODE")]
        products: Vec<String>,
        /// Account country as Battle.net reports it, e.g. `AUT`.
        #[arg(long, default_value = "")]
        account_country: String,
        /// GeoIP country, e.g. `AT`.
        #[arg(long, default_value = "")]
        geo_country: String,
        /// Write tags the way Agent writes them (`EU? acct-AUT? … speech?`),
        /// and update `.build.info` to match.
        #[arg(long)]
        agent_tags: bool,
        /// Print what would be written without writing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Report or run first-run provisioning: Proton, prefix, Battle.net.
    Setup {
        /// Only report what is missing; provision nothing.
        #[arg(long)]
        status: bool,
        /// Proton-GE release to install if one is needed.
        #[arg(long)]
        proton: Option<String>,
    },
    /// Probe Blizzard's version service for which product codes are live.
    ///
    /// There is no listing endpoint any more, so this asks about a built-in
    /// list of candidate codes and reports what answers.
    Products {
        #[arg(long, default_value = "eu")]
        region: String,
        /// Extra codes to probe beyond the built-in list.
        #[arg(long = "code", value_name = "CODE")]
        codes: Vec<String>,
        /// Show codes that answered 404 as well.
        #[arg(long)]
        all: bool,
        /// Simultaneous probes.
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
    },
    /// Resolve a product's themed branding through the asset pipeline.
    Assets {
        /// Product code to theme, e.g. `wow`.
        #[arg(long, default_value = "wow")]
        product: String,
        /// Base URL or fixture directory. Defaults to `assets_source` in
        /// config.toml, else the built-in source.
        #[arg(long)]
        source: Option<String>,
    },
    /// Resolve the primary action button's state for a product.
    Status {
        /// Product code, e.g. `wow` or `wow_classic_era`.
        #[arg(long, default_value = "wow")]
        product: String,
        #[arg(long, default_value = "eu")]
        region: String,
        /// Skip the version service and report what the disk alone implies.
        #[arg(long)]
        offline: bool,
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
    #[command(hide = true)]
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
    #[command(hide = true)]
    /// Open an install's CASC store and decode a sample of its files.
    Casc {
        /// A WoW install root, or a `Data`/`Data/data` directory.
        path: PathBuf,
        /// How many files to decode, spread across the whole index.
        #[arg(long, default_value_t = 2000)]
        sample: usize,
    },
    #[command(hide = true)]
    /// Inspect the CASC bucket indices of an install, verifying both hashes
    /// and re-serializing each file to prove we can write them byte-identically.
    Idx {
        /// A WoW install root, or a `Data/data` directory directly.
        path: PathBuf,
    },
    #[command(hide = true)]
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

/// A progress callback for a compaction copying `total` bytes: one line per
/// whole percent, overwritten in place.
fn copy_progress(total: u64) -> impl FnMut(u64) {
    let mut shown = None;
    move |copied| {
        let percent = (copied * 100).checked_div(total).unwrap_or(100).min(100);
        if shown != Some(percent) {
            shown = Some(percent);
            eprint!("\rcopying         {percent:3}%");
            if percent == 100 {
                eprintln!();
            }
        }
    }
}

/// The install a store-level command (`verify`, `reindex`, `product-db`)
/// works on without `--path`: the default library's.
fn default_install(layout: &Layout) -> std::path::PathBuf {
    layout
        .default_library()
        .map(|library| layout.wow_install_in(library))
        .unwrap_or_else(|| layout.games_root().join(lightforge_core::paths::WOW_DIR))
}

/// How Battle.net addresses a product's install, which depends on the library
/// it is in; see [`lightforge_core::library::Library::windows_wow_path`].
fn windows_wow_path(layout: &Layout, code: &ProductCode) -> String {
    layout
        .library(code)
        .map(|library| library.windows_wow_path())
        .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned())
}

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

    // A config that exists but cannot be read stops every command, the way
    // the GUI stops. Falling back to the defaults would point the command at
    // `~/Games/Blizzard` whatever the file said, and the helpers further down
    // that read the config again rely on this having parsed.
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

    // Last, because it is the answer to a different question: not what the
    // pass did, but where it spent the time. A launch that feels stuck is one
    // of these stages, and a signal grace is five seconds where a scan is
    // sixty milliseconds.
    for timing in &report.timings {
        println!("timing          {:<18} {:>6} ms", timing.stage, timing.took.as_millis());
    }
}

/// Render a config tweak list: key, value, and why it is set.
fn print_tweaks(tweaks: &[lightforge_core::battlenet::Tweak]) {
    for tweak in tweaks {
        println!("  {:<40} {}\n    {}", tweak.key(), tweak.value, tweak.reason);
    }
}

/// The build a Battle.net verb should use: `--proton` if given, else the pin
/// from `config.toml`, else the newest installed.
///
/// With a product named, that product's own pin comes first, the client is
/// being told to start that game, and a pin set for it means it. Without one,
/// the verb is about the client itself and takes Battle.net's pin.
///
/// Leaks a `String` so the borrowed `&str` outlives the call. That is fine for
/// a CLI process that is about to exec a game and exit, and beats threading a
/// lifetime through every call site for one value read once.
fn battle_net_pin<'a>(explicit: Option<&'a str>, code: Option<&ProductCode>) -> Option<&'a str> {
    if explicit.is_some() {
        return explicit;
    }
    let dirs = AppDirs::discover().ok()?;
    let config = Config::load_or_default(&dirs.config_file()).ok()?;
    let version = match code {
        Some(code) => config.handoff_proton(code)?,
        None => config.battle_net_proton_version()?,
    }
    .to_owned();
    Some(Box::leak(version.into_boxed_str()))
}

/// Say so when the build about to run is older than the one that built the
/// prefix every product shares.
///
/// A line on stderr rather than a refusal: the CLI is where an older build is
/// deliberately tried, and `--proton` is how you try it. The GUI asks first.
fn warn_on_downgrade(layout: &Layout, build: &lightforge_core::proton::ProtonRelease) {
    use lightforge_core::proton;

    let built_by = proton::prefix_build(&layout.compat_data());
    if let proton::PrefixFit::Older { built_by } =
        proton::prefix_fit(&build.version, built_by.as_deref())
    {
        eprintln!(
            "warning: {} is older than {built_by}, which last upgraded this prefix.\n\
             \x20        Proton migrates a prefix forward only; the shared prefix may not \
             survive it.",
            build.version,
        );
    }
}

/// Products Lightforge patches, read fresh from `config.toml`.
///
/// Read per call rather than threaded through: pre-flight runs once per
/// launch, and a stale list here would leave a product's auto-update on.
fn managed_products() -> Vec<lightforge_core::product::ProductCode> {
    AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .map(|config| config.managed_products())
        .unwrap_or_default()
}

/// Resolve the Proton build to run against the shared prefix, and a runner for
/// it. `None` selects the tested build, see `proton::select`.
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

/// The configured default addon channel, or stable when there is no config.
///
/// The CLI has no settings UI, so it reads the same `addon_channel` the GUI
/// writes rather than inventing a second default.
/// Where this game's settings backups go, and how many it keeps.
fn settings_backup(layout: &Layout, dirs: &AppDirs, code: &ProductCode) -> backup::Plan {
    let keep = Config::load_or_default(&dirs.config_file())
        .map_or(backup::DEFAULT_KEEP, |config| config.backup_keep);
    backup::Plan::for_product(layout, dirs, code, keep)
}

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
    // falls back to the built-in proxy rather than to nothing.
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

fn truncate(text: &str, width: usize) -> String {
    match text.chars().count() > width {
        true => format!("{}…", text.chars().take(width - 1).collect::<String>()),
        false => text.to_owned(),
    }
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
