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
/// pipe goes away, `lightforge idx … | head` being the obvious case, every
/// subsequent `println!` hits `EPIPE` and panics. Restoring the default
/// disposition makes the process exit the way every other Unix tool does,
/// with no backtrace and no partial-looking failure.
///
/// Safe to call here and nowhere else: it runs before any thread is spawned,
/// and it only resets a signal to what the kernel would have done anyway.
/// The install a store-level command works on when `--path` was not given:
/// the default library's.
///
/// Store-level commands, `verify`, `reindex`, `product-db`: address a whole
/// `World of Warcraft/` folder rather than one product, and with libraries
/// there can be more than one. The default library is the useful answer, and
/// `--path` is how you name another.
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

    match cli.command {
        Command::Paths => {
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
        }
        Command::Config => {
            // No loader here; show what a fresh install writes.
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
            let main = layout
                .default_library()
                .context("no storage library is configured")?;
            let (link, target) = (layout.wow_symlink(), layout.wow_install_in(main));
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

            let (runner, build) = proton_runner(&layout, battle_net_pin(proton.as_deref(), None))?;
            println!("using {} ({})", build.version, build.path.display());
            warn_on_downgrade(&layout, &build);
            println!("installer {}", installer_path.display());

            let setup = Installer::new(&layout, &installer_path)
                .with_locale(&locale)
                .with_timeout(std::time::Duration::from_secs(timeout));
            if !setup.is_installed() {
                println!("running setup — this takes a few minutes");
            }

            let managed = Config::load_or_default(&dirs.config_file())
                .map(|c| c.managed_products())
                .unwrap_or_default();
            let report =
                battlenet::provision(&runner, &layout, &user, &locale, &installer_path, &managed)
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
        }
        Command::BattlenetRun { proton, product, exec, uri, install_game, skip_preflight } => {
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
                let report = Preflight::new(Config::default().prefix_user).with_managed(managed_products())
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
        }
        Command::Play { product, proton, dry_run } => {
            use lightforge_core::config::Config;
            use lightforge_core::preflight::{NoDownloads, Preflight};

            let code: ProductCode = product.parse().expect("product codes are infallible");

            // The game is launched through the symlink, so Wine sees the
            // conventional Windows path, the same thing that keeps CEF happy
            // for the in-game store.
            let flavor_dir = layout.wow_symlink().join(&*code.install_dir());
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
                .unwrap_or_else(|_| layout.wow_install(&code));

            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            let (runner, build) = proton_runner(&layout, proton.as_deref())?;
            let mut runner = runner
                .with_compat_mount(&target)
                // Per-product shader cache: the prefix is shared, so pooling
                // it would mean a Proton swap for one product discarding
                // another's accumulated shaders.
                .with_shader_cache(dirs.shader_cache_dir(&code));

            // The same per-product settings the GUI's Game Optimizations panel
            // writes. A launcher that honoured them from one entry point and
            // not the other would make every measurement unreproducible.
            let tweaks = Config::load_or_default(&dirs.config_file())
                .ok()
                .and_then(|config| config.product(&code).map(|p| p.profile.tweaks.clone()))
                .unwrap_or_default();
            let tools = lightforge_core::tweaks::HostTools::detect();
            for (key, value) in tweaks.environment(&tools) {
                runner = runner.with_env(key, value);
            }

            println!("using {} ({})", build.version, build.path.display());
            println!("install {}", target.display());
            println!("running {}", exe.display());
            let wrappers = tweaks.wrappers(&tools);
            if !wrappers.is_empty() {
                let names: Vec<String> =
                    wrappers.iter().map(|w| w.display().to_string()).collect();
                println!("through  {}", names.join(" "));
            }
            if dry_run {
                return Ok(());
            }

            let report = Preflight::new(Config::default().prefix_user).with_managed(managed_products())
                .run(&runner, &layout, Some(&code), &mut NoDownloads)
                .context("pre-flight cleanup")?;
            print_preflight(&report);

            // After pre-flight, with the client closed: it rewrites Config.wtf
            // from memory on exit, so anything set once is not still set.
            let (set, unset) = tweaks.cvars();
            match lightforge_core::cvars::apply(
                &layout.wtf_dir(&code).join("Config.wtf"),
                &set,
                &unset,
            ) {
                Ok(0) => {}
                Ok(n) => println!("applied {n} client setting(s)"),
                Err(e) => eprintln!("could not apply client settings: {e}"),
            }

            let status = lightforge_core::tweaks::wrapped(
                runner.command("run", &exe, Vec::<String>::new()),
                &wrappers,
            )
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

            let report = Preflight::new(Config::default().prefix_user).with_managed(managed_products())
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
        } => {
            use lightforge_core::net::Http;
            use lightforge_core::tact::download::Cancel;
            use lightforge_core::tact::install::{self, Event, Options as InstallOptions};
            use lightforge_core::tact::version::Region;
            use std::time::Instant;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let region: Region = region.parse().context("parsing --region")?;
            let gib = |b: u64| b as f64 / 1024f64.powi(3);

            let options = InstallOptions {
                region,
                locale,
                flavor,
                concurrency,
                optional_tags: optional,
                gap_bytes: gap_kib * 1024,
                checkpoint_bytes: checkpoint_mib * 1024 * 1024,
                install_files,
                limit_archives,
                limit_runs,
                skip_loose,
                ..InstallOptions::default()
            };

            // Event printing, shared by the dry run and the real one. The
            // index prelude and the transfer are both far too chatty to print
            // per event, so both are throttled here rather than in core.
            let start = Instant::now();
            let mut last_index = 0usize;
            let mut last_report = Instant::now();
            let mut on_event = move |event: Event| match event {
                Event::Build { versions_name, build_config, archives } => {
                    println!("build           {versions_name} ({build_config})");
                    println!("archives        {archives}");
                }
                Event::Repaired { dir } => {
                    println!("repaired        added owner write to {}", dir.display())
                }
                Event::Encoding { entries } => println!("encoding        {entries} entries"),
                Event::Manifests { download, install, unknown_tags } => {
                    println!("download        {download} entries");
                    println!("install         {install} entries");
                    println!("tags            {unknown_tags:?}");
                }
                Event::Wanted { files, bytes } => {
                    println!("wanted          {files} files, {:.1} GiB", gib(bytes))
                }
                Event::Indices(step) => {
                    if step.done - last_index >= 100 || step.done == step.total {
                        last_index = step.done;
                        println!("  indices {}/{}", step.done, step.total);
                    }
                }
                Event::IndicesDone { cached, downloaded, bytes } => println!(
                    "indices         {cached} cached, {downloaded} downloaded ({:.1} MiB)",
                    bytes as f64 / 1048576.0
                ),
                Event::Optional { names } => println!(
                    "optional        {}",
                    match names.is_empty() {
                        true => "none offered".to_owned(),
                        false => names.join(", "),
                    }
                ),
                Event::Loose { files } => println!("loose files     {files}"),
                Event::Located { located, wanted } => {
                    println!("located         {located} of {wanted}")
                }
                Event::Planned { requests, files, bytes, padding, already_present, missing } => {
                    println!(
                        "\nplan            {requests} requests, {files} files, {:.2} GiB ({:.2} GiB padding)",
                        gib(bytes),
                        gib(padding)
                    );
                    println!("already present {already_present}");
                    if missing > 0 {
                        println!("UNLOCATABLE     {missing} keys");
                    }
                }
                Event::Progress(p) => {
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
                            mb / start.elapsed().as_secs_f64().max(0.001)
                        );
                    }
                }
                Event::InstallFile { path, bytes } => {
                    println!("  {} ({:.1} MiB)", path.display(), bytes as f64 / 1048576.0)
                }
                Event::Extracted { files, bytes } => println!(
                    "extracted       {files} files, {:.1} MiB",
                    bytes as f64 / 1048576.0
                ),
                Event::BuildInfo { path, version } => {
                    println!("\n.build.info     {} ({version})", path.display())
                }
                Event::ProductDb { path } => {
                    println!(".product.db     {}", path.display())
                }
                Event::Registered { code } => {
                    println!("registered      {code} in Agent's product.db")
                }
                Event::Incomplete { reasons } => {
                    println!("\n.build.info     NOT written — this run is not a complete install");
                    for reason in reasons {
                        println!("                ({reason})");
                    }
                }
            };

            let http = Http::new()?;
            let cancel = Cancel::new();

            if let Some(secs) = cancel_after {
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    println!("\n--- cancelling at {secs}s ---");
                    cancel.cancel();
                });
            }

            if dry_run {
                let prepared = install::prepare(&http, &dest, &code, &options, &mut on_event).await?;
                if show_install {
                    let selected = prepared.install.select(&prepared.tags);
                    println!("\ninstall files   {} selected of {}", selected.len(), prepared.install.len());
                    for entry in &selected {
                        println!("  {}", entry.path);
                    }
                }
                println!("\n--dry-run: nothing fetched");
                return Ok(());
            }

            let t = Instant::now();
            let summary = install::run(&http, &dest, &code, &options, &cancel, on_event).await?;

            println!(
                "\nstored          {} files, {:.2} GiB in {:.1?}",
                summary.outcome.files_stored,
                gib(summary.outcome.bytes_stored),
                t.elapsed()
            );
            println!(
                "unverified      {} (encrypted or not in encoding)",
                summary.outcome.unverified
            );
            for failure in summary.outcome.failed.iter().take(10) {
                eprintln!(
                    "  FAILED {} — {}",
                    lightforge_core::tact::config::to_hex(&failure.ekey),
                    failure.reason
                );
            }
            if summary.outcome.failed.len() > 10 {
                eprintln!("  ... and {} more", summary.outcome.failed.len() - 10);
            }

            println!(
                "\nstore           {} archives, {} files, {:.2} GiB",
                summary.store.archives,
                summary.store.files,
                gib(summary.store.bytes)
            );
            println!("indices         {} written", summary.store.indices.len());
            match summary.complete {
                true => println!("\nverify with:  lightforge casc {}", dest.display()),
                false => println!("\nre-run to continue; what is stored is kept"),
            }
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
            // key, the encoding key names the bytes but cannot check them.
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
        } => {
            use lightforge_core::net::Http;
            use lightforge_core::tact::explain as tact_explain;
            use lightforge_core::tact::storage::Storage;
            use lightforge_core::tact::install::{self, Event, Options as InstallOptions};
            use lightforge_core::tact::version::Region;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let region: Region = region.parse().context("parsing --region")?;
            let gib = |b: u64| b as f64 / 1024f64.powi(3);

            let options = InstallOptions {
                region,
                locale,
                flavor,
                concurrency,
                limit_archives,
                optional_tags: optional,
                ..InstallOptions::default()
            };

            // Quiet by design: the report is the output, and the index
            // prelude would bury it.
            let mut last_index = 0usize;
            let mut on_event = move |event: Event| match event {
                Event::Build { versions_name, build_config, .. } => {
                    println!("build           {versions_name} ({build_config})");
                }
                Event::Indices(step)
                    if step.done - last_index >= 400 || step.done == step.total =>
                {
                    last_index = step.done;
                    eprintln!("  indices {}/{}", step.done, step.total);
                }
                _ => {}
            };

            let http = Http::new().context("building the HTTP client")?;
            let prepared =
                install::prepare(&http, &dest, &code, &options, &mut on_event).await?;

            println!(
                "optional        {}",
                match prepared.optional.is_empty() {
                    true => "none offered".to_owned(),
                    false => prepared.optional.join(", "),
                }
            );
            let wanted_bytes: u64 = prepared.wanted.iter().map(|e| e.size).sum();
            println!("wanted          {} files, {:.2} GiB", prepared.wanted.len(), gib(wanted_bytes));
            println!(
                "already present {} files",
                prepared.plan.already_present
            );
            println!(
                "plan            {} requests, {} files, {:.2} GiB",
                prepared.plan.requests(),
                prepared.plan.files(),
                gib(prepared.plan.bytes())
            );

            if !explain {
                println!("\nPass --explain for the breakdown.");
                return Ok(());
            }

            let store = Storage::open(&dest).ok();
            if !prepared.probe.opened() {
                println!(
                    "\nstore           WOULD NOT OPEN at {} — every key counts as absent",
                    dest.display()
                );
            }
            let report = tact_explain::explain(
                &prepared.wanted,
                store.as_ref(),
                &prepared.locations,
                &prepared.loose,
                |k| prepared.probe.has(k),
            );

            println!("\nby priority (lower is fetched sooner)");
            println!("{:>8}  {:>12} {:>9}  {:>12} {:>9}", "priority", "present", "GiB", "missing", "GiB");
            for (priority, split) in &report.by_priority {
                println!(
                    "{priority:>8}  {:>12} {:>9.2}  {:>12} {:>9.2}",
                    split.present.files,
                    gib(split.present.bytes),
                    split.missing.files,
                    gib(split.missing.bytes),
                );
            }

            println!("\nby bucket (missing keys against what the store lists)");
            println!("{:>6}  {:>12}  {:>12}", "bucket", "in store", "missing");
            for bucket in 0u8..16 {
                let held = report.store_by_bucket.get(&bucket).copied().unwrap_or(0);
                let missing = report.missing_by_bucket.get(&bucket).copied().unwrap_or(0);
                let flag = match held {
                    0 => "   <- no index loaded",
                    _ => "",
                };
                println!("{bucket:>6}  {held:>12}  {missing:>12}{flag}");
            }

            println!("\nmissing keys by source");
            println!("  in an archive   {}", report.sources.archived);
            println!("  loose           {}", report.sources.loose);
            println!("  nowhere         {}  (nothing can fetch these)", report.sources.nowhere);

            let by_tag = tact_explain::by_tag(
                &prepared.download,
                &prepared.tags,
                |k| prepared.probe.has(k),
            );
            println!("\nby tag (what the selection is made of)");
            println!(
                "{:<16} {:>4} {:>6}  {:>12} {:>9}  {:>12} {:>9}  {:>12}",
                "tag", "kind", "named", "selected", "GiB", "missing", "GiB", "covers"
            );
            // Normally only the tags doing work are worth printing. When
            // *nothing* is selected the interesting question is the opposite
            // one, what vocabulary does this manifest even have, and that
            // is exactly the case where the filtered table is empty and says
            // nothing at all.
            let selects_nothing = by_tag.rows.iter().all(|r| r.selected.files == 0);
            for row in by_tag.rows.iter().filter(|r| selects_nothing || r.selected.files > 0) {
                println!(
                    "{:<16} {:>4} {:>6}  {:>12} {:>9.2}  {:>12} {:>9.2}  {:>12}",
                    row.name,
                    row.kind,
                    match row.constrained {
                        true => "yes",
                        false => "NO",
                    },
                    row.selected.files,
                    gib(row.selected.bytes),
                    row.missing.files,
                    gib(row.missing.bytes),
                    row.covered.files,
                );
            }
            match by_tag.unconstrained_kinds.as_slice() {
                [] => println!("  every tag category the manifest defines is constrained"),
                kinds => println!(
                    "  UNCONSTRAINED categories: {kinds:?} — every value of these is selected"
                ),
            }

            println!("\nprobe cross-check");
            match report.false_missing {
                0 => println!("  the store agrees: every planned file really is absent"),
                n => {
                    println!("  {n} planned files ARE in the store — the probe is wrong");
                    for ekey in &report.examples {
                        let hex: String =
                            ekey.iter().map(|b| format!("{b:02x}")).collect();
                        println!("    {hex}");
                    }
                }
            }
        }
        Command::AddonAdopt { product, path, ledger, catalog } => {
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
        }

        Command::AddonFingerprints { product, path, catalog } => {
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
            // a lookup rather than a guess.
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
            for addon in addons::scan(&dir).unwrap_or_default() {
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
        }

        Command::Addons { product, path, token } => {
            use lightforge_core::addons;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let dir = path.unwrap_or_else(|| layout.addons_dir(&code));
            let installed = addons::scan(&dir)
                .with_context(|| format!("reading {}", dir.display()))?;

            println!("addons dir      {}", dir.display());
            println!("installed       {}\n", installed.len());

            for addon in &installed {
                let ids: Vec<String> = addon
                    .ids
                    .iter()
                    .map(|(provider, id)| format!("{}:{id}", provider.as_str()))
                    .collect();
                println!(
                    "{:<34} {:<18} {}",
                    addon.folder,
                    addon.version.as_deref().unwrap_or("-"),
                    ids.join(" ")
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
        }
        Command::AddonSearch { query, product, limit, catalog } => {
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
        }
        Command::AddonInstall { id, provider, product, catalog } => {
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
        }
        Command::AddonImport { token, product, catalog, pinned, dry_run } => {
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
                for entry in &resolved {
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
        }
        Command::AddonUpdate { product, catalog } => {
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
        }
        Command::BackupList { product } => {
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
        }
        Command::BackupCreate { product } => {
            let code: ProductCode = product.parse().expect("product codes are infallible");
            let dirs = AppDirs::discover().context("resolving XDG directories")?;
            let plan = settings_backup(&layout, &dirs, &code);
            match plan.create(backup::Reason::Manual).context("backing up")? {
                Some(b) => println!("backed up to {}", b.path.display()),
                None if plan.account_dir.is_dir() => println!("unchanged since the last backup"),
                None => println!("nothing to back up: {} does not exist", plan.account_dir.display()),
            }
        }
        Command::BackupRestore { backup: which, product } => {
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
        }
        Command::AddonRemove { id, provider, product } => {
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
        }
        Command::AddonToken { token } => {
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
        }
        Command::Verify { path, deep, repair, all } => {
            use lightforge_core::tact::storage::Storage;
            use lightforge_core::tact::verify;
            use std::time::{Duration, Instant};
            const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

            let install = path.unwrap_or_else(|| default_install(&layout));
            let data_dir = install.join("Data").join("data");

            // Repair rewrites the indices a running client memory-maps. The
            // scan itself only reads, so it is allowed either way.
            if repair {
                let alive = lightforge_core::preflight::scan(&layout);
                if !alive.is_empty() {
                    for process in &alive {
                        println!("running         {} (pid {})", process.name, process.pid);
                    }
                    anyhow::bail!("close the game and Battle.net first — the store is in use");
                }
            }

            let store = Storage::open(install.join("Data")).context("opening the store")?;
            let stats = store.stats();
            println!("store           {}", data_dir.display());
            println!("indexed         {} files across {} buckets", stats.files, stats.buckets);

            let started = Instant::now();
            let mut last = Instant::now();
            let report = match deep {
                false => verify::scan(&store, |p| {
                    if last.elapsed() >= Duration::from_millis(500) {
                        last = Instant::now();
                        eprintln!("  {}/{} checked, {} damaged", p.checked, p.total, p.damaged);
                    }
                }),
                true => {
                    // The deep scan opens the store itself, after snapshotting
                    // it, so a write between the two is caught.
                    drop(store);
                    let cancel = lightforge_core::tact::download::Cancel::new();
                    verify::deep_scan(&install, &cancel, |p| {
                        if last.elapsed() >= Duration::from_secs(2) {
                            last = Instant::now();
                            eprintln!(
                                "  {:.1}/{:.1} GiB hashed, {} damaged",
                                p.bytes as f64 / GIB,
                                p.bytes_total as f64 / GIB,
                                p.damaged
                            );
                        }
                    })?
                }
            };

            println!(
                "checked         {} entries in {:.1}s",
                report.checked,
                started.elapsed().as_secs_f64()
            );
            println!(
                "read            {} headers, {:.1} MiB",
                report.headers_read,
                report.bytes_read as f64 / (1024.0 * 1024.0)
            );
            if report.unverifiable > 0 {
                println!(
                    "unverifiable    {} raw patch manifests — kept, nothing to check them against",
                    report.unverifiable
                );
            }
            if !report.missing_archives.is_empty() {
                println!("missing         {} archive files", report.missing_archives.len());
            }

            if report.is_healthy() {
                println!("\nhealthy — every entry is where its index says, and is what it claims");
                if deep {
                    println!("          and every stored byte matches its checksum");
                }
                return Ok(());
            }

            println!("\ndamaged         {} entries", report.damaged.len());
            let mut by_fault: std::collections::BTreeMap<&str, usize> = Default::default();
            for damage in &report.damaged {
                *by_fault.entry(damage.fault.describe()).or_default() += 1;
            }
            for (fault, count) in &by_fault {
                println!("  {count:>8}  {fault}");
            }

            let show = match all {
                true => report.damaged.len(),
                false => 10.min(report.damaged.len()),
            };
            println!();
            for damage in &report.damaged[..show] {
                let key: String =
                    damage.entry.key.iter().map(|b| format!("{b:02x}")).collect();
                println!(
                    "  {key}  data.{:03} +{}  {}",
                    damage.entry.archive_id, damage.entry.offset, damage.fault.describe()
                );
            }
            if show < report.damaged.len() {
                println!("  … and {} more (--all to list them)", report.damaged.len() - show);
            }

            match repair {
                false => println!(
                    "\nRun again with --repair to drop these from the indices; the next \
                     download refetches them."
                ),
                true => {
                    // Again, not only at the start: a deep scan runs for
                    // minutes, and the game may have been started meanwhile.
                    let alive = lightforge_core::preflight::scan(&layout);
                    if !alive.is_empty() {
                        for process in &alive {
                            println!("running         {} (pid {})", process.name, process.pid);
                        }
                        anyhow::bail!("close the game and Battle.net first — the store is in use");
                    }
                    let removed = verify::repair_unchanged(&data_dir, &report)?;
                    println!("\nremoved         {removed} entries from the indices");
                    println!("Run a download for this product to fetch them again.");
                }
            }
        }
        Command::Reindex { path } => {
            use lightforge_core::tact::writer::StoreWriter;

            // Never rewrite indices under a live client: it memory-maps them,
            // and this is the exact overlap the whole pre-flight design exists
            // to prevent.
            let alive = lightforge_core::preflight::scan(&layout);
            if !alive.is_empty() {
                for process in &alive {
                    println!("running         {} (pid {})", process.name, process.pid);
                }
                anyhow::bail!("close the game and Battle.net first — the store is in use");
            }

            let install = path.unwrap_or_else(|| default_install(&layout));
            let data_dir = install.join("Data").join("data");

            let before: Vec<(String, u64)> = std::fs::read_dir(&data_dir)?
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "idx"))
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        e.metadata().map(|m| m.len()).unwrap_or(0),
                    )
                })
                .collect();
            println!("store           {}", data_dir.display());
            println!("indices         {} before", before.len());

            // Loading and finishing rewrites every bucket as a new generation,
            // with the current sizing rules, and prunes the old ones. No
            // archive is opened, so not a byte of game data is touched.
            let summary = StoreWriter::create(&data_dir)?.finish()?;
            println!("rewrote         {} indices", summary.indices.len());
            for path in &summary.indices {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                println!("  {}  {size} bytes", path.file_name().unwrap().to_string_lossy());
            }
        }
        Command::Uninstall { product, path, yes } => {
            use lightforge_core::tact::uninstall;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let install = path.unwrap_or_else(|| layout.wow_install(&code));
            let plan = uninstall::plan(&install, &code)?;

            let gib = |b: u64| b as f64 / 1024f64.powi(3);
            println!("install         {}", install.display());
            println!("product         {}", code.as_str());

            if plan.is_empty() {
                println!("\nnothing to remove — that product is not installed here");
                return Ok(());
            }

            println!("\nwould delete:");
            for dir in plan.flavor_dir.iter().chain(plan.store.iter()) {
                println!("  {}", dir.display());
            }
            if plan.has_row {
                println!(
                    "  .build.info {}",
                    match plan.remove_build_info {
                        true => "(the whole file — no products left)",
                        false => "(this product's row only)",
                    }
                );
            }
            println!("\nfrees           {:.2} GiB", gib(plan.bytes));
            match plan.remaining.is_empty() {
                true => println!("remaining       nothing — this removes the whole install"),
                false => println!(
                    "remaining       {} (the shared CASC store is kept for them)",
                    plan.remaining.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
                ),
            }

            if !yes {
                println!("\nnothing deleted. Re-run with --yes to go ahead.");
                return Ok(());
            }

            let removed = uninstall::execute(&install, &code, &plan, &windows_wow_path(&layout, &code))?;
            println!("\ndeleted         {} paths, {:.2} GiB", removed.paths.len(), gib(removed.bytes));
        }
        Command::Reclaim { product, build_key, region, path, yes } => {
            use lightforge_core::buildinfo::BuildInfo;
            use lightforge_core::net::Http;
            use lightforge_core::tact::version::{self, Region};
            use lightforge_core::tact::reclaim;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let install = path.unwrap_or_else(|| layout.wow_install(&code));
            let http = Http::new()?;
            let departing = match build_key {
                Some(build_key) => {
                    let region: Region = region.parse().context("parsing --region")?;
                    let cdns = version::cdns(&http, region, code.as_str()).await?;
                    let cdn = version::for_region(&cdns, region, |c| &c.name)
                        .context("the CDN table is empty")?
                        .clone();
                    reclaim::Build { product: code.clone(), build_key, cdn }
                }
                None => {
                    let info = BuildInfo::load(install.join(".build.info"))
                        .with_context(|| format!("reading {}/.build.info", install.display()))?;
                    reclaim::Build::installed(&info, &code)?
                }
            };

            let gib = |b: u64| b as f64 / 1024f64.powi(3);
            let plan = reclaim::plan(&http, &install, &departing).await?;
            println!("install         {}", install.display());
            println!("departing       {} ({})", code.as_str(), departing.build_key);
            println!("entries         {} used by nothing else installed", plan.entries.len());
            println!("archives        {} compacted or deleted", plan.archives.len());
            println!("copies          {:.2} GiB other versions still use", gib(plan.moved));
            println!("frees           {:.2} GiB", gib(plan.freed));
            println!(
                "stranded        {:.2} GiB left as dead space in shared archives",
                gib(plan.stranded)
            );

            if !yes || plan.entries.is_empty() {
                if !plan.entries.is_empty() {
                    println!("\nnothing changed. Re-run with --yes to go ahead.");
                }
                return Ok(());
            }
            if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
                anyhow::bail!("close {} first — the store is in use", process.name);
            }
            let freed = reclaim::execute(&install, &plan, copy_progress(plan.moved))?;
            println!("\nfreed           {:.2} GiB", gib(freed));
        }
        Command::Prune { path, yes } => {
            use lightforge_core::net::Http;
            use lightforge_core::tact::reclaim;

            let install = path.unwrap_or_else(|| default_install(&layout));
            let gib = |b: u64| b as f64 / 1024f64.powi(3);
            let http = Http::new()?;
            let builds = reclaim::builds(&install)?;
            let plan = reclaim::plan_unused(&http, &install).await?;
            let unused: u64 = plan.entries.iter().map(|e| u64::from(e.size)).sum();
            println!("install         {}", install.display());
            println!(
                "keeps           {}",
                builds.iter().map(|b| b.product.as_str()).collect::<Vec<_>>().join(", ")
            );
            println!("unused          {} files, {:.2} GiB", plan.entries.len(), gib(unused));
            println!("archives        {} compacted or deleted", plan.archives.len());
            println!("copies          {:.2} GiB still in use", gib(plan.moved));
            println!("frees           {:.2} GiB", gib(plan.freed));
            println!("stranded        {:.2} GiB left as dead space", gib(plan.stranded));

            if !yes || plan.entries.is_empty() {
                if !plan.entries.is_empty() {
                    println!("\nnothing changed. Re-run with --yes to go ahead.");
                }
                return Ok(());
            }
            if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
                anyhow::bail!("close {} first — the store is in use", process.name);
            }
            let freed = reclaim::execute(&install, &plan, copy_progress(plan.moved))?;
            println!("\nfreed           {:.2} GiB", gib(freed));
        }
        Command::Compact { path, yes } => {
            use lightforge_core::tact::compact;

            let install = path.unwrap_or_else(|| default_install(&layout));
            let gib = |b: u64| b as f64 / 1024f64.powi(3);
            let (plan, snapshot) = compact::plan(&install)?;
            println!("install         {}", install.display());
            println!("archives        {} to rewrite or delete", plan.archives.len());
            println!("copies          {:.2} GiB", gib(plan.moved));
            println!("frees           {:.2} GiB", gib(plan.freed));
            println!("left            {:.2} GiB of dead space not worth rewriting", gib(plan.left));

            if !yes || plan.archives.is_empty() {
                if !plan.archives.is_empty() {
                    println!("\nnothing changed. Re-run with --yes to go ahead.");
                }
                return Ok(());
            }
            if let Some(process) = lightforge_core::preflight::scan(&layout).first() {
                anyhow::bail!("close {} first — the store is in use", process.name);
            }
            let freed = compact::execute(&install, &plan, &snapshot, copy_progress(plan.moved))?;
            println!("\nfreed           {:.2} GiB", gib(freed));
        }
        Command::ProductDb { path, products, account_country, geo_country, agent_tags, dry_run } => {
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
        }
        Command::Setup { status, proton } => {
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
        }
        Command::Products { region, codes, all, concurrency } => {
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
        }
        Command::Assets { product, source } => {
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
        }
        Command::Status { product, region, offline } => {
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
        }
        Command::UpdateCheck { path, product, region } => {
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
        }
        Command::Content { path, product, locale, fdid, file } => {
            use lightforge_core::tact::config::BuildConfig;
            use lightforge_core::tact::content::{Content, Locale};
            use lightforge_core::tact::storage::Storage;

            let code: ProductCode = product.parse().expect("product codes are infallible");
            let locale: Locale = locale.parse().context("parsing --locale")?;

            // The build key is already recorded in .build.info, which is the
            // file M3's writer maintains. This is where the two halves meet.
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
            let path = layout.build_info(&code);

            // Read-modify-write, atomically. The caller is responsible for
            // having run pre-flight first. This is the file Agent watches.
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
            let path = layout.build_info(&code);
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
