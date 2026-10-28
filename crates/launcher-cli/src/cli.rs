//! The command line: every command and its arguments, as clap reads them.

use crate::*;

#[derive(Parser)]
#[command(name = "lightforge", version, about = "Lightforge CLI")]
pub(crate) struct Cli {
    /// Override the Blizzard root (default: ~/Games/Blizzard).
    #[arg(long, global = true)]
    pub(crate) root: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Print the resolved directory layout.
    Paths,
    /// Print the effective configuration as TOML.
    Config,
    /// Check this system for what Battle.net and the games need under Proton.
    Doctor {
        /// Print the checks as JSON.
        #[arg(long)]
        json: bool,
    },
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
        #[arg(long, value_parser = commands::product_code)]
        product: Option<ProductCode>,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, value_parser = commands::product_code)]
        product: Option<ProductCode>,
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
        #[arg(default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        #[arg(long, default_value = "eu")]
        region: String,
        /// Locale to install, e.g. `enUS`.
        #[arg(long, default_value = "enUS")]
        locale: String,
        /// Flavor directory the install files go in. Default: the one the
        /// product config names, e.g. `_forever_` for `wowf`.
        #[arg(long)]
        flavor: Option<String>,
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
        /// Pre-download the build Blizzard lists ahead of its release, into
        /// the store only. The installed build stays the one that launches.
        #[arg(long, conflicts_with = "install_files")]
        stage: bool,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        #[arg(long, default_value = "eu")]
        region: String,
        #[arg(long, default_value = "enUS")]
        locale: String,
        #[arg(long)]
        flavor: Option<String>,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        /// Read this AddOns directory instead of the product's own.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Base URL of the addon catalog, or a fixture directory.
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Recognise hand-installed addons by their contents and manage them.
    AddonAdopt {
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Install or update addons from a WoWUp import token.
    AddonImport {
        /// The token, or `@path` to read it from a file.
        token: String,
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Remove an addon Lightforge installed.
    AddonRemove {
        /// Provider id, e.g. `2382`.
        id: String,
        #[arg(long, default_value = "curse")]
        provider: String,
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
    },
    /// List a game's `WTF/Account` backups, newest first.
    BackupList {
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
    },
    /// Back up a game's `WTF/Account` now. Skipped when nothing changed.
    BackupCreate {
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
    },
    /// Restore a backup over `WTF/Account`. The current state is backed up
    /// first, so a restore can be undone the same way.
    BackupRestore {
        /// The number `backup-list` shows (1 is the newest), or the file name.
        backup: String,
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        /// Only this game account's folder, as `backup-list` names it; the
        /// other accounts and what they share stay as they are.
        #[arg(long)]
        account: Option<String>,
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
    /// Move a game to another library without downloading it again. Prints
    /// the plan unless `--yes` is given.
    Move {
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        /// The library to move it to, by name or id, as `lightforge paths` lists them.
        #[arg(long)]
        to: String,
        /// Actually move. Without this, nothing is touched.
        #[arg(long)]
        yes: bool,
    },
    /// Remove an installed product. Prints the plan unless `--yes` is given.
    Uninstall {
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long = "product", value_name = "CODE", value_parser = commands::product_code)]
        products: Vec<ProductCode>,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
        /// Base URL or fixture directory. Defaults to `assets_source` in
        /// config.toml, else the built-in source.
        #[arg(long)]
        source: Option<String>,
    },
    /// Resolve the primary action button's state for a product.
    Status {
        /// Product code, e.g. `wow` or `wow_classic_era`.
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(long, default_value = "wow", value_parser = commands::product_code)]
        product: ProductCode,
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
        #[arg(value_parser = commands::product_code)]
        product: ProductCode,
        #[arg(long)]
        build_key: String,
        #[arg(long)]
        cdn_key: String,
        #[arg(long)]
        version: String,
    },
}
