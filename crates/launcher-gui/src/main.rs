//! Lightforge GUI — the dashboard.
//!
//! One product at a time: its branding, one primary action whose label and
//! color *are* the state machine in [`lightforge_core::state`], and the
//! secondary paths around it.
//!
//! Two things here are deliberate and easy to get wrong:
//!
//! - **Progress arrives through a `watch` channel, sampled on a timer.** The
//!   downloader calls its callback once per stored file; forwarding each one as
//!   a message would drown the loop and make transfer speed a function of UI
//!   responsiveness. `watch` keeps only the newest value, so a slow window
//!   costs nothing.
//! - **Pausing is cancelling.** The downloader's cancel token stops the loop
//!   between chunks and the store keeps what it has, so resuming is a fresh
//!   run that plans against what is already on disk. That is also exactly what
//!   pre-flight does before launching Battle.net, which is why the same guard
//!   serves both.

mod addonwork;
mod art;
mod popover;
mod provision;
mod tasks;
mod theme;
mod trace;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use iced::widget::{
    button, column, container, image, mouse_area, pick_list, progress_bar, responsive, row,
    scrollable, stack, text, text_input, tooltip, Space,
};
use iced::{Alignment, ContentFit, Element, Length, Subscription, Task};

use lightforge_core::assets::{Manifest, Pipeline, Store};
use lightforge_core::config::Config;
use lightforge_core::debug::DebugInfo;
use lightforge_core::net::Http;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::{Branch, Catalog, Kind, Product, ProductCode, ProductLine};
use lightforge_core::state::{self, ActionState, Local};
use lightforge_core::proton::ProtonPin;
use lightforge_core::setup::Status as SetupStatus;
use lightforge_core::storage;
use lightforge_core::tact::install::Offerings;
use lightforge_core::transfers::{self, Saved, Standing};
use lightforge_core::tact::download::Cancel;
use lightforge_core::tact::version::Region;
use lightforge_core::tweaks::{GraphicsApi, HostTools, Tweaks};

use art::Art;
use tasks::{Refreshed, SharedDownloads, TransferState};

/// Where a player manages their subscription, per the spec.
const ACCOUNT_URL: &str = "https://account.battle.net/games";

/// The cog on the split Play button.
///
/// Bundled rather than themed: it is Lightforge's own chrome, not a product's
/// branding, so it must render before any asset pipeline has answered — and
/// the UI font has no gear glyph to fall back on.
const GEAR_ICON: &[u8] = include_bytes!("../assets/gear.png");

/// The cog's one handle. `Handle::from_bytes` mints a fresh id per call, so
/// building it in `view` made every rebuild a cache miss and a PNG decode.
fn gear_icon() -> image::Handle {
    static GEAR: OnceLock<image::Handle> = OnceLock::new();
    GEAR.get_or_init(|| image::Handle::from_bytes(GEAR_ICON)).clone()
}

/// The arrow on the band's overflow button. Bundled for the same reasons as
/// the cog: it is Lightforge's chrome, and the UI font has no dependable
/// chevron — U+2304 is missing from most of them and renders as a tofu box.
const CHEVRON_ICON: &[u8] = include_bytes!("../assets/chevron-down.png");

/// The chevron's one handle — see [`gear_icon`] for why it is cached.
fn chevron_icon() -> image::Handle {
    static CHEVRON: OnceLock<image::Handle> = OnceLock::new();
    CHEVRON.get_or_init(|| image::Handle::from_bytes(CHEVRON_ICON)).clone()
}

/// Add a game. A typed `+` does exist in every font, but it is a *character*:
/// it sits on the text baseline at the weight of the UI font, which reads as
/// a letter next to the drawn chevron beside it. Both marks are strokes of the
/// same width so the pair looks like one control.
const PLUS_ICON: &[u8] = include_bytes!("../assets/plus.png");

/// The plus's one handle — see [`gear_icon`] for why it is cached.
fn plus_icon() -> image::Handle {
    static PLUS: OnceLock<image::Handle> = OnceLock::new();
    PLUS.get_or_init(|| image::Handle::from_bytes(PLUS_ICON)).clone()
}

/// Opening size, and the floor for resizing — deliberately the same number.
///
/// The header sets the width: the title, the product picker (whose longest
/// entry is a user-supplied display name) and four navigation buttons, none of
/// which wrap. The dashboard sets the rest. Its action panel is a fixed 240pt
/// column of controls beside a column that fills, and a `Row` does not wrap
/// either — so once the window is narrower than both together, the fixed
/// column is simply not drawn. That is not a squeeze, it is PLAY disappearing
/// entirely, which is why the floor is the opening size rather than something
/// smaller that still technically renders.
///
/// The number itself is a display decision rather than a layout one: the
/// dashboard stops clipping somewhere around 1000pt, and the rest is giving
/// the key art enough room to be art. On a 2560x1440 screen this is half the
/// width and a little over half the height.
///
/// Because the floor is the same value, this is also the smallest screen
/// Lightforge is usable on — a 1366x768 laptop cannot fit 820pt of client
/// plus a title bar, and would get a window taller than its display.
///
/// A Wayland compositor is free to ignore all of it — a tiling one will hand
/// the window whatever its slot is, and below this width the dashboard will
/// lose its right-hand column. Nothing here can prevent that.
const WINDOW: iced::Size = iced::Size::new(1280.0, 820.0);
const WINDOW_MIN: iced::Size = WINDOW;

/// iced's renderer, chosen for us unless the user overrides it.
///
/// `wgpu` lags badly when resizing a window on native Wayland — the client
/// keeps up (a resize is serviced here in well under a millisecond) but the
/// buffer reaches the compositor late, so the window crawls behind the
/// pointer. It is an upstream bug, iced-rs/iced#2750, and the software
/// renderer does not have it.
///
/// Nothing is given up by paying for it. This is a launcher: the UI is static,
/// only redraws when something changes, and never animates. GPU rasterisation
/// buys frames we do not draw. One renderer everywhere also means what gets
/// tested is what people run, rather than a path that breaks only on hardware
/// no one here has.
///
/// `ICED_BACKEND` is read by iced itself, so setting it only when unset leaves
/// the override in the user's hands: `ICED_BACKEND=wgpu` restores the default.
fn choose_renderer() {
    if std::env::var_os("ICED_BACKEND").is_none() {
        std::env::set_var("ICED_BACKEND", "tiny-skia");
    }
}

fn main() -> iced::Result {
    trace::begin();
    choose_renderer();
    iced::application("Lightforge", Lightforge::update, Lightforge::view)
        .subscription(Lightforge::subscription)
        .theme(|_| theme::base())
        .window(iced::window::Settings {
            size: WINDOW,
            min_size: Some(WINDOW_MIN),
            ..iced::window::Settings::default()
        })
        // Closing is handled by the app, so a transfer can checkpoint first.
        // Set after `window`, which replaces the settings wholesale.
        .exit_on_close_request(false)
        .run_with(|| {
            let (app, task) = Lightforge::new();
            trace::mark("app constructed, startup tasks queued");
            (app, task)
        })
}

#[derive(Debug, Clone)]
enum Message {
    Refreshed(ProductCode, Box<Refreshed>),
    Themed(ProductCode, Box<tasks::Themed>),
    /// Open the Battle.net client on its own — the other-titles path.
    OpenBattleNet,
    /// Show or hide the download manager.
    ToggleManager,
    /// Show or hide the catalog of products that could be added.
    ToggleBrowse,
    ToggleStrip,
    ToggleHints,
    /// What Battle.net reports as installed, for the catalog's labels.
    InstalledVersions(HashMap<String, String>),
    /// The product codes Battle.net's own config uses.
    KnownCodes(Vec<String>),
    /// Track a product from the catalog, putting it in the header.
    AddProduct(ProductCode),
    /// Stop tracking one, taking it out of the header.
    RemoveProduct(ProductCode),
    /// Pause one product's transfer from the manager.
    PauseDownload(ProductCode),
    /// Resume or re-queue one product's transfer from the manager.
    ResumeDownload(ProductCode),
    /// Drop a transfer from the queue entirely.
    CancelDownload(ProductCode),
    /// A tile in the product bar was clicked.
    LineSelected(ProductLine),
    /// A branch was picked for the selected line. Adds the product first when
    /// the branch was on offer but not configured.
    BranchSelected(BranchChoice),
    /// Pre-flight finished; `Launch` says what it was clearing the way for.
    PreflightDone(Launch, Result<(), String>),
    /// Diagnostic only: subscribed to solely under `LIGHTFORGE_TRACE=frames`.
    ResizeTraced(iced::Size),
    /// The optimistic, cache-only paint that precedes [`Message::Themed`].
    CachedTheme(ProductCode, Box<tasks::Themed>),
    Primary,
    Tick,
    Downloaded(ProductCode, Result<Complete, String>),
    CloseRequested(iced::window::Id),
    Launched(Result<String, String>),
    BattleNet,
    ManageSubscription,
    OpenFiles,
    CopyDebug,
    /// A switch on the Game Optimizations panel was flipped.
    Tweak(Tweak, bool),
    /// The graphics API the client should ask for.
    TweakApi(ApiChoice),
    TweakFpsCap(String),
    TweakSpellQueue(String),
    /// Key and value fields of the environment escape hatch.
    TweakEnvKey(String),
    TweakEnvValue(String),
    TweakEnvAdd,
    TweakEnvRemove(String),
    /// Throw away this product's shader cache, e.g. after a Proton swap.
    ClearShaderCache,
    /// Opt in or out of one optional download, by manifest tag name.
    OptionalContent(String, bool),
    /// The cog beside the primary button.
    ToggleGear,
    /// A click that no widget consumed — close whatever is open, the way
    /// every other context menu on the desktop behaves.
    Dismiss,
    /// A click *inside* an open panel. Does nothing, and that is the point:
    /// it stops the click reaching [`Message::Dismiss`] underneath.
    MenuClick,
    /// Ask the version service what is current, and read what the build
    /// offers.
    CheckForUpdates,
    CheckedForUpdates(Box<Result<lightforge_core::tact::install::Offerings, String>>),
    /// Plan against the build and fetch whatever the store is short, after
    /// checking what is there is not damaged.
    ScanAndRepair,
    Scanned(Box<Result<tasks::Scanned, String>>),
    /// Hash every stored byte against its checksums — or, while that runs,
    /// stop it.
    DeepScan,
    DeepScanned(Box<Result<lightforge_core::tact::verify::Report, String>>),
    /// Drop what the last deep scan found damaged, once nothing is running.
    RepairDamaged,
    Repaired(Result<usize, tasks::Unrepaired>),

    Show(Screen),
    SetupStatus(Option<Box<SetupStatus>>),
    RunSetup,
    SetupFinished(Result<Box<SetupStatus>, String>),
    PinDefaultProton(ProtonChoice),
    /// The build Battle.net and Agent launch under.
    PinBattleNetProton(ProtonChoice),
    PinProductProton(ProtonChoice),
    /// The download cap field changed.
    DownloadLimitChanged(String),
    /// The pause-while-playing switch was flipped.
    PauseWhilePlaying(bool),
    /// Battle.net's own limit, as read from its config.
    BattleNetLimitRead(Option<u64>),
    /// The Battle.net limit field changed.
    BattleNetLimitChanged(String),
    /// Writing that limit finished.
    BattleNetLimitWritten(Result<(), String>),
    CheckProton,
    ProtonChecked(Result<provision::ProtonReleases, String>),
    /// A version was chosen in the "install another build" picker.
    ChooseProtonVersion(ProtonChoice),
    /// Install the chosen version.
    InstallProton,
    /// Delete one managed build, by version.
    RemoveProton(String),
    ProtonRemoved(Result<String, String>),
    /// Measured sizes for the installed builds.
    ProtonSizes(Vec<(PathBuf, u64)>),
    StorageMeasured(Box<storage::Report>),
    InstallOffered(ProductCode, Box<Result<Offerings, String>>),
    InstallHighRes(bool),
    GamesFolderChanged(String),
    InstallCancel,
    InstallStart,
    AskUninstall,
    UninstallPlanned(ProductCode, Box<Result<tasks::UninstallPreview, String>>),
    UninstallCancel,
    UninstallConfirm,
    Uninstalled(ProductCode, Result<u64, String>),
    ClearCache,
    CacheCleared(Result<u64, String>),
    OpenLibrary,
    OpenConfigFolder,
    LocationChanged(String),
    UseLocation,

    AddonsListed(Box<addonwork::Listing>),
    TokenChanged(String),
    ImportToken,
    UpdateAddons,
    AddonsFinished(Result<Box<lightforge_core::addons::Report>, String>),
    RemoveAddon(String, String),
    AddonRemoved(Result<Vec<String>, String>),
    OpenUrl(String),

    AddonTab(AddonTab),
    SettingsTab(SettingsTab),
    SearchChanged(String),
    RunSearch,
    SearchDone(Box<addonwork::SearchOutcome>),
    InstallAddon(String, String),
    CheckAddonUpdates,
    AddonUpdatesChecked(Result<Vec<lightforge_core::addons::manage::UpdateStatus>, String>),
}

/// What INSTALL opens before anything downloads: where it goes, what it
/// costs, and whether to include high-resolution textures.
#[derive(Debug, Clone)]
struct InstallDialog {
    code: ProductCode,
    label: String,
    stage: DialogStage,
    high_res: bool,
    /// Whether the game folder can still be chosen: no WoW version is in it.
    movable: bool,
    /// Space on the disk the typed game folder would land on.
    disk: Option<storage::Disk>,
}

#[derive(Debug, Clone)]
enum DialogStage {
    /// Reading the build's download manifest.
    Calculating,
    Ready(Box<Offerings>),
    Failed(String),
}

/// The confirmation before a game is deleted.
#[derive(Debug, Clone)]
struct UninstallDialog {
    code: ProductCode,
    label: String,
    /// `None` while the plan is being worked out.
    plan: Option<Result<tasks::UninstallPreview, String>>,
    /// Deleting: the buttons are held until it answers.
    working: bool,
}

/// Which section of Settings is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Setup,
    Proton,
    Downloads,
    Storage,
    About,
}

impl SettingsTab {
    const ALL: [Self; 5] =
        [Self::Setup, Self::Proton, Self::Downloads, Self::Storage, Self::About];

    fn label(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Proton => "Proton",
            Self::Downloads => "Downloads",
            Self::Storage => "Storage",
            Self::About => "About",
        }
    }
}

/// Which half of the addon screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddonTab {
    /// What is installed, and what has updates.
    Mine,
    /// Search every provider and install.
    Browse,
}

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Dashboard,
    /// Setup lives here too, as the first card.
    Settings,
    Addons,
    /// Per-product performance settings.
    Tweaks,
}

/// One switch on the Game Optimizations panel.
///
/// A single message with a discriminant rather than a variant each: the panel
/// is a list of independent booleans, and fifteen near-identical messages would
/// only obscure that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tweak {
    GamePerformance,
    GameMode,
    MangoHud,
    SkipAddonProfiling,
    ReduceRaidEffects,
    FasterZoneLoads,
    RawMouse,
    NoScreenEffects,
    MaxDrawDistance,
}

/// A graphics API in a picker, or the "leave the client's own choice alone"
/// entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiChoice(Option<GraphicsApi>);

impl std::fmt::Display for ApiChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(api) => f.write_str(api.label()),
            None => f.write_str("Whatever the client picks"),
        }
    }
}

/// A Proton build in a picker, or the "follow the default" entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtonChoice(Option<String>);

impl std::fmt::Display for ProtonChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(version) => f.write_str(version),
            None => f.write_str("Newest installed"),
        }
    }
}

/// The parts of a finished run the UI actually shows. `install::Summary` is not
/// `Clone`, and a message has to be.
#[derive(Debug, Clone)]
struct Complete {
    complete: bool,
    cancelled: bool,
    files: usize,
    version: Option<String>,
}

/// A product in the picker. `ProductCode` has no `Display`, and the label the
/// user reads is not the code.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    code: ProductCode,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// One entry in the branch dropdown: a build of the selected line.
///
/// Carries whether the product is configured so the list can offer branches
/// the user has never set up — picking one is how you add it, which beats
/// hand-editing `config.toml` to try a PTR.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BranchChoice {
    code: ProductCode,
    branch: Branch,
    /// The catalog's name for this product, used when adding it to config.
    label: String,
    configured: bool,
}

impl std::fmt::Display for BranchChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.configured {
            true => f.write_str(self.branch.label()),
            // Said plainly in the list rather than greyed out: iced's
            // pick_list cannot style one entry differently, and a silently
            // different-looking row explains nothing anyway.
            false => write!(f, "{} — add", self.branch.label()),
        }
    }
}

/// Where one product's transfer has got to.
///
/// Keyed to a product rather than held app-wide, which is the whole point:
/// the old single `Option<Transfer>` meant every product you switched to
/// inherited whichever one happened to be running.
enum Stage {
    /// Accepted, waiting for the running one to finish.
    Queued,
    Running {
        rx: tokio::sync::watch::Receiver<TransferState>,
        cancel: Cancel,
    },
    /// Stopped with work outstanding. Keeps the progress it reached so the bar
    /// does not snap back to zero on resume.
    Paused(state::Progress),
}

/// One product's place in the download queue.
struct Download {
    code: ProductCode,
    label: String,
    stage: Stage,
    /// Newest sample off the channel, kept after a pause so the manager can
    /// still say how far it got.
    sample: TransferState,
    rate: Rate,
    job: Job,
}

/// What one transfer set out to fetch, carried across its pauses.
///
/// The bar measures the job, not the product: an update that changes 10 GB of
/// a 90 GB install reads "0 of 10 GB", not "80 of 90 GB". A resumed run plans
/// only what is still missing, so the job's size is fixed by the *first* plan
/// and each later run is placed within it.
#[derive(Debug, Clone, Copy, Default)]
struct Job {
    /// Bytes the first run planned. `None` until one has planned.
    total: Option<u64>,
    /// Bytes done when the current run started — shown while a resumed run
    /// is still planning, so the bar holds rather than dropping to zero.
    reached: u64,
}

impl Download {
    fn progress(&self) -> state::Progress {
        job_progress(self.job, &self.sample, self.rate.per_second)
    }

    /// This entry as a resumed job: the size its first run planned, and how
    /// far it got before the pause.
    fn resumed(self) -> Job {
        let reached = match self.stage {
            Stage::Paused(p) => p.downloaded_bytes,
            _ => 0,
        };
        Job { reached, ..self.job }
    }

    fn transfer(&self) -> state::Transfer {
        match &self.stage {
            Stage::Queued => state::Transfer::Queued,
            Stage::Running { .. } => state::Transfer::Running(self.progress()),
            Stage::Paused(progress) => state::Transfer::Paused(*progress),
        }
    }
}

/// What a pre-flight pass is clearing the way for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    /// Straight to the game executable.
    Game,
    /// Hand off to Battle.net, so the session is SSO'd and the shop works.
    BattleNet,
    /// Open the Battle.net client itself, for everything outside the WoW
    /// family. A pass-through, not a managed install — see `SPEC.md`.
    Client,
}

struct Lightforge {
    layout: Layout,
    dirs: Option<AppDirs>,
    config: Config,
    region: Region,
    /// One client, one connection pool, for every request the update loop
    /// starts. `None` only if reqwest failed to build one at startup.
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    /// The branding manifest, read once and themed from thereafter.
    manifest: Option<Arc<Manifest>>,
    /// Every configured product's bar icon, for those that have one.
    icons: HashMap<ProductCode, PathBuf>,
    /// The branch last used in each line, so returning to a tile returns to
    /// the build you were on rather than snapping back to live.
    last_branch: HashMap<ProductLine, ProductCode>,
    /// Which products exist and how they group — built in, then whatever the
    /// manifest states over it.
    catalog: Catalog,
    choices: Vec<Choice>,
    selected: Choice,

    local: Local,
    remote: Option<lightforge_core::tact::version::Version>,
    branding: Option<Art>,

    /// Every transfer this session knows about, in queue order. At most one
    /// is [`Stage::Running`].
    queue: Vec<Download>,
    /// Whether the download manager panel is open.
    manager_open: bool,
    /// Whether the catalog panel is open.
    browse_open: bool,
    /// The band's overflow menu — only ever reachable when something is in it.
    strip_open: bool,
    /// Whether the explanatory prose under the controls is drawn.
    ///
    /// Off for the people who use Lightforge: the interface is meant to be
    /// readable without a paragraph under every switch, and a screen of grey
    /// sentences is what it looks like when it is not. On for whoever is
    /// working on it — see [`HINTS_CHORD`].
    hints: bool,
    /// Battle.net's own mark, if the prefix has one — see
    /// [`Layout::battle_net_icon`]. Held as a path, not a handle: iced keys an
    /// image by its path, so rebuilding the header does not decode it again.
    bnet_icon: Option<PathBuf>,
    /// Whether the cog menu beside the primary button is open.
    gear_open: bool,
    /// Live progress of a store scan, and whether one is running.
    scan_rx: Option<tokio::sync::watch::Receiver<lightforge_core::tact::verify::Progress>>,
    scan_sample: lightforge_core::tact::verify::Progress,
    scanning: bool,
    /// Set while a deep scan runs; tripping it stops the scan. The download
    /// queue is held for as long as it is set.
    deep_cancel: Option<Cancel>,
    /// Damage a deep scan found and could not drop yet, because something
    /// was running in the prefix. Repaired from the cog menu.
    owed_repair: Option<Box<lightforge_core::tact::verify::Report>>,
    /// Versions Battle.net reports installed, read when the catalog opens.
    installed_versions: HashMap<String, String>,
    /// Product codes Battle.net itself uses, folded into the catalog.
    known_codes: Vec<String>,
    /// Transfers paused for a play session, to be put back afterwards.
    resume_after_play: Vec<ProductCode>,
    /// Why `config.toml` could not be loaded. While set, the app shows only
    /// that and writes nothing — see [`Lightforge::new`].
    config_error: Option<String>,
    /// The queue as last written to disk, so an unchanged one is not
    /// rewritten on every message.
    saved_queue: transfers::Queue,
    /// Set once the window is closing: the pause that closing performs must
    /// not be what the next start restores.
    queue_frozen: bool,
    /// A cancelled transfer whose task has not reported back yet.
    ///
    /// Pausing flips the UI at once, but the run keeps going for a moment —
    /// it finishes the requests already in flight and checkpoints the store.
    /// Starting anything before it lands would put two writers on the same
    /// CASC store, which is the corruption pre-flight exists to prevent.
    winding_down: Option<ProductCode>,

    downloads: SharedDownloads,
    busy: bool,
    status: String,
    error: Option<String>,
    /// Set when the window has asked to close and a transfer is still coming
    /// to rest; the window closes once it has.
    closing: Option<iced::window::Id>,

    screen: Screen,
    setup: Option<SetupStatus>,
    /// Live setup progress, sampled off the same tick as a download.
    setup_rx: Option<tokio::sync::watch::Receiver<provision::Progress>>,
    setup_sample: provision::Progress,
    setup_running: bool,
    /// Editable copy of the download cap, in MB/s. Empty means unlimited.
    limit_input: String,
    /// Editable copy of Battle.net's *own* cap, in MB/s. Empty means none.
    bnet_limit_input: String,
    proton_notice: Option<String>,
    /// Stable releases that could be installed, newest first.
    proton_available: Vec<String>,
    /// The one picked in the install picker, if any.
    proton_choice: Option<String>,
    /// Measured size of each installed build, keyed by its directory.
    proton_sizes: HashMap<PathBuf, u64>,
    /// The last Storage measurement; `None` until the section first asks.
    storage: Option<storage::Report>,
    storage_measuring: bool,
    /// What clearing the cache last freed, for the line under its row.
    cache_notice: Option<String>,
    /// Editable library root, applied by "Use this location".
    location_input: String,
    install_dialog: Option<InstallDialog>,
    uninstall_dialog: Option<UninstallDialog>,
    /// Editable game folder, applied when the install dialog starts.
    games_input: String,

    /// Optional performance tools this machine has, detected once at startup.
    /// A toggle for a binary that is not installed is a launch failure waiting
    /// to happen, so the panel hides what is missing.
    tools: HostTools,
    /// Editable copies of the selected product's numeric tweaks, and the two
    /// fields of the environment escape hatch.
    fps_input: String,
    spell_input: String,
    env_key_input: String,
    env_value_input: String,

    addons: addonwork::Listing,
    token_input: String,
    addon_rx: Option<tokio::sync::watch::Receiver<addonwork::Progress>>,
    addon_sample: addonwork::Progress,
    addons_busy: bool,
    addon_report: Option<lightforge_core::addons::Report>,
    addon_tab: AddonTab,
    settings_tab: SettingsTab,
    search_input: String,
    search: addonwork::SearchOutcome,
    searching: bool,
    updates: Vec<lightforge_core::addons::manage::UpdateStatus>,
}

impl Lightforge {
    fn new() -> (Self, Task<Message>) {
        let dirs = AppDirs::discover().ok();
        // A missing file is a first run and gets defaults. A file that exists
        // but will not load is someone's settings with a problem in them: the
        // app stops on it rather than quietly running on defaults — which
        // point at the default library — and then saving those over it.
        let loaded = dirs.as_ref().map(|d| Config::load_or_default(&d.config_file()));
        let config_error = match (&loaded, dirs.as_ref()) {
            (Some(Err(e)), Some(dirs)) => Some(broken_config(e, &dirs.config_file())),
            _ => None,
        };
        let config = match loaded {
            Some(Ok(config)) => config,
            _ => Config::with_default_products(),
        };
        // Honours `blizzard_root`, the same way the CLI does.
        let layout = config.layout().unwrap_or_else(|_| Layout::with_root("."));
        let bnet_icon = layout.battle_net_icon();

        let choices: Vec<Choice> = config
            .products
            .iter()
            .map(|p| Choice { code: p.code.clone(), label: p.display_name.clone() })
            .collect();
        let selected = choices.first().cloned().unwrap_or(Choice {
            code: ProductCode::Retail,
            label: "World of Warcraft".to_owned(),
        });

        // Without XDG directories there is no cache to theme into, and the
        // dashboard renders its own plain look rather than failing to start.
        let pipeline = dirs
            .as_ref()
            .map(|dirs| Pipeline::new(config.asset_origin(), Store::new(dirs.asset_cache_dir())));


        let limit_input = config.download_limit_mbps.map(|v| v.to_string()).unwrap_or_default();
        let app = Self {
            layout,
            dirs,
            config,
            region: Region::default(),
            http: Http::new().ok(),
            pipeline,
            manifest: None,
            icons: HashMap::new(),
            last_branch: HashMap::new(),
            catalog: Catalog::builtin(),
            choices,
            selected: selected.clone(),
            local: Local::default(),
            remote: None,
            branding: None,
            queue: Vec::new(),
            manager_open: false,
            browse_open: false,
            strip_open: false,
            hints: false,
            bnet_icon,
            gear_open: false,
            scan_rx: None,
            scan_sample: lightforge_core::tact::verify::Progress::default(),
            scanning: false,
            deep_cancel: None,
            owed_repair: None,
            installed_versions: HashMap::new(),
            known_codes: Vec::new(),
            resume_after_play: Vec::new(),
            saved_queue: transfers::Queue::default(),
            config_error,
            queue_frozen: false,
            winding_down: None,
            downloads: SharedDownloads::default(),
            busy: false,
            status: "checking…".to_owned(),
            error: None,
            closing: None,
            screen: Screen::Dashboard,
            setup: None,
            setup_rx: None,
            setup_sample: provision::Progress::default(),
            setup_running: false,
            limit_input,
            bnet_limit_input: String::new(),
            proton_notice: None,
            proton_available: Vec::new(),
            proton_choice: None,
            proton_sizes: HashMap::new(),
            storage: None,
            storage_measuring: false,
            cache_notice: None,
            location_input: String::new(),
            install_dialog: None,
            uninstall_dialog: None,
            games_input: String::new(),
            tools: HostTools::detect(),
            fps_input: String::new(),
            spell_input: String::new(),
            env_key_input: String::new(),
            env_value_input: String::new(),
            addons: addonwork::Listing::default(),
            token_input: String::new(),
            addon_rx: None,
            addon_sample: addonwork::Progress::default(),
            addons_busy: false,
            addon_report: None,
            addon_tab: AddonTab::Mine,
            settings_tab: SettingsTab::Setup,
            search_input: String::new(),
            search: addonwork::SearchOutcome::default(),
            searching: false,
            updates: Vec::new(),
        };
        let mut app = app;
        // Nothing starts on a config that did not load: no probes against a
        // library it may not describe, and above all no restored download.
        if app.config_error.is_some() {
            return (app, Task::none());
        }
        app.load_tweak_inputs();
        app.restore_queue();

        // What is provisioned decides which screen opens: a machine that has
        // never run the CLI cannot install or launch anything, and a dashboard
        // whose only working button is an error is worse than saying so.
        let probe = Task::perform(
            provision::status(app.layout.clone(), app.config.clone()),
            |status| Message::SetupStatus(status.map(Box::new)),
        );
        let installed = Task::perform(
            tasks::installed_versions(app.layout.clone()),
            Message::InstalledVersions,
        );
        let codes = Task::perform(
            tasks::known_codes(app.layout.clone(), app.config.prefix_user.clone()),
            Message::KnownCodes,
        );
        let bnet_limit = Task::perform(
            tasks::battle_net_limit(app.layout.clone(), app.config.prefix_user.clone()),
            Message::BattleNetLimitRead,
        );
        // What was downloading when the app closed carries on, as it does in
        // Battle.net.
        let resume = app.pump();
        let task = Task::batch([
            app.reload(selected.code),
            probe,
            installed,
            codes,
            bnet_limit,
            resume,
        ]);
        (app, task)
    }

    /// Re-read the disk, the version service, and the theme for one product.
    fn reload(&self, code: ProductCode) -> Task<Message> {
        Task::batch([
            Task::perform(
                tasks::refresh(
                    self.http.clone(),
                    self.layout.clone(),
                    code.clone(),
                    self.region,
                    self.kind_of(&code) == Kind::Managed,
                ),
                {
                    let code = code.clone();
                    move |r| Message::Refreshed(code.clone(), Box::new(r))
                },
            ),
            // Fires first and usually lands first: it reads the cache and
            // nothing else, so the window opens themed rather than snapping
            // to art once the revalidation below comes back.
            Task::perform(
                tasks::cached_theme(self.pipeline.clone(), self.product_codes(), code.clone()),
                {
                    let code = code.clone();
                    move |t| Message::CachedTheme(code.clone(), Box::new(t))
                },
            ),
            Task::perform(
                tasks::theme(
                    self.http.clone(),
                    self.pipeline.clone(),
                    self.manifest.clone(),
                    self.product_codes(),
                    code.clone(),
                ),
                {
                    let code = code.clone();
                    move |t| Message::Themed(code.clone(), Box::new(t))
                },
            ),
            Task::perform(addonwork::list(self.layout.clone(), code), |listing| {
                Message::AddonsListed(Box::new(listing))
            }),
        ])
    }

    /// A running scan's bar and caption, for under the status line.
    ///
    /// A scan otherwise says nothing for a minute or more, which reads as a
    /// hung launcher. A deep scan is measured in bytes: its files range from
    /// a few bytes to hundreds of megabytes, so a file count stalls and leaps.
    fn scan_progress<'a>(
        &self,
        left: iced::widget::Column<'a, Message>,
    ) -> iced::widget::Column<'a, Message> {
        let p = &self.scan_sample;
        if !self.scanning || p.total == 0 {
            return left;
        }
        let (done, what) = match p.bytes_total {
            0 => (
                p.checked as f32 / p.total as f32,
                format!("{} of {} files checked", p.checked, p.total),
            ),
            total => (
                p.bytes as f32 / total as f32,
                format!("{} checked", bytes_of(p.bytes, total)),
            ),
        };
        let caption = match p.damaged {
            0 => what,
            n => format!("{what} — {n} damaged so far"),
        };
        left.push(progress_bar(0.0..=1.0, done).height(6)).push(muted(caption, theme::CAPTION))
    }

    /// Every configured product, in the order the bar draws them.
    fn product_codes(&self) -> Vec<ProductCode> {
        self.choices.iter().map(|c| c.code.clone()).collect()
    }

    /// Clear the way for a launch, then let [`Message::PreflightDone`] start it.
    fn preflight(&mut self, what: Launch) -> Task<Message> {
        self.busy = true;
        self.error = None;
        self.status = "pre-flight…".to_owned();

        // Pre-flight pauses the launched product's own download by itself —
        // that one is a correctness guard, since the client and the
        // downloader would share one CASC store. The rest of the queue shares
        // that store too, so it is paused here when asked, and resumed when
        // the game exits.
        if self.config.pause_downloads_while_playing {
            let others: Vec<ProductCode> = self
                .queue
                .iter()
                .filter(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued))
                .map(|d| d.code.clone())
                .collect();
            for code in others {
                self.pause(&code);
                self.resume_after_play.push(code);
            }
        }
        Task::perform(
            tasks::preflight(
                self.layout.clone(),
                self.selected.code.clone(),
                self.downloads.clone(),
                // Cleared under whichever build is about to run in the prefix.
                match what {
                    Launch::Game => tasks::Runs::Game,
                    Launch::BattleNet | Launch::Client => tasks::Runs::BattleNet,
                },
            ),
            move |result| Message::PreflightDone(what, result),
        )
    }

    /// Whether Lightforge installs this product or hands it to Battle.net.
    ///
    /// The catalog's answer where it has one, the code's shape otherwise, so a
    /// product added before the manifest was read is still classified.
    fn kind_of(&self, code: &ProductCode) -> Kind {
        self.catalog.get(code).map(|e| e.kind).unwrap_or_else(|| Kind::of(code))
    }

    /// The action panel for a game Battle.net installs and starts.
    ///
    /// Deliberately bare: no Install, no Update, no Play, not even greyed out.
    /// Offering those disabled would say Lightforge could manage this game and
    /// merely will not.
    fn handoff_panel(&self) -> Element<'_, Message> {
        let mut left = column![].spacing(theme::SM).width(Length::Fill);
        left = left.push(match &self.error {
            Some(e) => text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR),
            None => text(self.status.clone()).size(theme::LABEL).color(theme::MUTED),
        });

        left = self.scan_progress(left);
        if let Some(version) = self.installed_versions.get(self.selected.code.as_str()) {
            left = left.push(
                row![muted("installed".to_owned(), theme::LABEL), mono(version.clone())]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
            );
        }
        left = left.push_maybe(self.hint("Installed and updated through Battle.net. Lightforge opens it, nothing more."));

        // Labelled for what it will actually do. Without a known game family
        // the client cannot be told which tile to open, so the button says so
        // rather than promising a hand-off that lands on someone else's game.
        let launches = lightforge_core::battlenet::game_uid(&self.selected.code).is_some();
        let open = button(
            container(
                text(match launches {
                    true => "Play via Battle.net",
                    false => "Open Battle.net",
                })
                .size(theme::BODY),
            )
            .center_x(Length::Fill),
        )
        .style(theme::outlined)
        .padding([theme::MD, theme::LG])
        .width(Length::Fixed(240.0));

        let press = match launches {
            true => Message::BattleNet,
            false => Message::OpenBattleNet,
        };

        container(
            row![
                left,
                match self.busy {
                    true => open,
                    false => open.on_press(press),
                },
            ]
            .spacing(theme::XL)
            .align_y(Alignment::Center),
        )
        .padding(theme::LG)
        .style(theme::floating)
        .into()
    }

    /// The button state, which is core's decision and not the GUI's.
    fn action(&self) -> ActionState {
        // Only this product's own transfer. Anything else is another game's
        // business, however loudly it is downloading.
        let transfer = self.download_for(&self.selected.code).map(Download::transfer);
        state::decide(&self.local, self.remote.as_ref(), transfer)
    }

    /// Progress against the whole product, not against this run.
    ///
    /// A resumed run plans only what is missing, so run-relative numbers read
    /// as "starting over" every time a download is paused and picked up again.
    /// Refresh only when the finished product is the one on screen.
    fn reload_if_showing(&mut self, code: &ProductCode) -> Task<Message> {
        match code == &self.selected.code {
            true => self.reload(code.clone()),
            false => Task::none(),
        }
    }

    fn download_for(&self, code: &ProductCode) -> Option<&Download> {
        self.queue.iter().find(|d| &d.code == code)
    }

    /// The one transfer actually moving bytes, if any.
    fn running(&self) -> Option<&Download> {
        self.queue.iter().find(|d| matches!(d.stage, Stage::Running { .. }))
    }

    /// Put the selected product in the queue, starting it if nothing is running.
    fn enqueue(&mut self) -> Task<Message> {
        let code = self.selected.code.clone();
        // The panel above should never offer this, but the download path is
        // the one that writes to disk, so it checks for itself.
        if self.kind_of(&code) == Kind::HandOff {
            self.error = Some(format!("{} is installed through Battle.net", self.selected.label));
            return Task::none();
        }
        let label = self.selected.label.clone();
        self.error = None;

        // Already in the queue: a queued entry stays put, a paused one is
        // being resumed — as the same job, so its bar continues.
        let mut job = Job::default();
        if let Some(existing) = self.queue.iter().position(|d| d.code == code) {
            if matches!(self.queue[existing].stage, Stage::Queued) {
                return Task::none();
            }
            job = self.queue.remove(existing).resumed();
        }

        self.queue.push(Download {
            code,
            label,
            stage: Stage::Queued,
            sample: TransferState { stage: "queued".to_owned(), ..TransferState::default() },
            rate: Rate::default(),
            job,
        });
        let started = self.pump();
        // Not started: waiting on another download, or on this one's own
        // paused run to finish winding down.
        let waiting = self.queue.iter().any(|d| {
            d.code == self.selected.code && matches!(d.stage, Stage::Queued)
        });
        if waiting {
            self.status = "queued…".to_owned();
        }
        started
    }

    /// Start the first queued transfer, if the line is free.
    ///
    /// One at a time on purpose: two downloads would split one connection and
    /// write into the same CASC store, making both slower and both ETAs
    /// fiction.
    fn pump(&mut self) -> Task<Message> {
        // A deep scan reads the store a transfer would write. It does not
        // refuse the download, it holds it: the queue picks up once the scan
        // is done.
        if self.running().is_some() || self.winding_down.is_some() || self.deep_cancel.is_some() {
            return Task::none();
        }
        let Some(next) = self.queue.iter().position(|d| matches!(d.stage, Stage::Queued)) else {
            return Task::none();
        };

        let cancel = Cancel::new();
        let (tx, rx) = tokio::sync::watch::channel(TransferState::default());
        let code = self.queue[next].code.clone();

        self.downloads.register(code.clone(), cancel.clone());
        self.queue[next].stage = Stage::Running { rx, cancel: cancel.clone() };
        // Whatever the line said before — "paused", most often — is over.
        if code == self.selected.code {
            self.status = "downloading…".to_owned();
        }
        self.queue[next].rate = Rate::default();
        self.queue[next].sample =
            TransferState { stage: "preparing".to_owned(), ..TransferState::default() };

        Task::perform(
            tasks::download(
                self.layout.clone(),
                code.clone(),
                self.region,
                self.config.download_limit_bytes(),
                self.opted_optional(&code),
                cancel,
                tx,
            ),
            move |result| {
                let mapped = result.map(|summary| Complete {
                    complete: summary.complete,
                    cancelled: summary.outcome.cancelled,
                    files: summary.outcome.files_stored,
                    version: summary.version,
                });
                Message::Downloaded(code.clone(), mapped)
            },
        )
    }

    /// Stop one product's transfer, keeping what it reached.
    fn pause(&mut self, code: &ProductCode) {
        let Some(entry) = self.queue.iter_mut().find(|d| &d.code == code) else {
            return;
        };
        match &entry.stage {
            Stage::Running { cancel, .. } => {
                cancel.cancel();
                // Shown as paused immediately — the click is answered now,
                // not when the run finishes winding down.
                entry.stage = Stage::Paused(job_progress(entry.job, &entry.sample, 0));
                self.winding_down = Some(code.clone());
                self.status = "pausing…".to_owned();
            }
            // Never started, so there is nothing to wind down.
            Stage::Queued => entry.stage = Stage::Paused(state::Progress::default()),
            Stage::Paused(_) => {}
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.persist_queue();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LineSelected(line) => {
                // Picking from the overflow menu closes it: the tile is in the
                // band now, and the menu is answering a question already asked.
                self.strip_open = false;
                self.select_line(&line)
            }

            Message::BranchSelected(pick) => {
                // An unconfigured branch is offered, not hidden — picking it
                // is how it gets added, which beats hand-editing config.toml
                // to try a PTR for an afternoon.
                if !pick.configured {
                    let label = pick.label.clone();
                    self.config
                        .products
                        .push(Product::new(pick.code.clone(), label.clone()));
                    self.save_config();
                    self.choices.push(Choice { code: pick.code.clone(), label });
                }
                let Some(choice) = self.choices.iter().find(|c| c.code == pick.code).cloned() else {
                    return Task::none();
                };
                self.select(choice)
            }

            Message::Refreshed(code, refreshed) => {
                if code != self.selected.code {
                    return Task::none();
                }
                self.local = refreshed.local.clone();
                self.remote = refreshed.remote.clone();
                self.status = match &refreshed.offline {
                    // Rule 2 of the resolver: offline never blocks a launch, so
                    // it is a note, not an error.
                    Some(_) => "version service unreachable — offline".to_owned(),
                    None => refreshed
                        .remote
                        .as_ref()
                        .map(|r| format!("latest {}", r.versions_name))
                        .unwrap_or_default(),
                };
                Task::none()
            }

            Message::ResizeTraced(size) => {
                trace::mark(format_args!("resize {}x{}", size.width, size.height));
                Task::none()
            }

            Message::CachedTheme(code, themed) => {
                trace::mark(format_args!(
                    "cache-first theme for {}: {}, {} icon(s)",
                    code.as_str(),
                    match &themed.branding {
                        Some(_) => "hit",
                        None => "MISS — nothing usable on disk",
                    },
                    themed.icons.len(),
                ));
                // Strictly a gap-filler: the revalidated answer wins whenever
                // it happens to arrive first, so none of this overwrites it.
                if self.manifest.is_none() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                if self.icons.is_empty() {
                    self.icons = themed.icons;
                }
                if code == self.selected.code && self.branding.is_none() {
                    self.branding = themed.branding;
                }
                Task::none()
            }

            Message::Themed(code, themed) => {
                trace::mark(format_args!(
                    "revalidated theme for {}: manifest {}, branding {}",
                    code.as_str(),
                    if themed.manifest.is_some() { "ok" } else { "failed" },
                    if themed.branding.is_some() { "ok" } else { "failed" },
                ));
                // The manifest describes every product, so it is kept even
                // when a switch has already moved on from the one it themed.
                if themed.manifest.is_some() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                if !themed.icons.is_empty() {
                    self.icons = themed.icons;
                }
                if code == self.selected.code {
                    self.branding = themed.branding;
                }
                Task::none()
            }

            Message::Primary => match self.action() {
                // A first install asks first; an update or a retry already
                // has its answers.
                ActionState::Install => self.open_install(),
                ActionState::Update { .. } | ActionState::Error { .. } => self.enqueue(),
                ActionState::Downloading(_) => {
                    // Pause: trip the token. The loop stops between chunks and
                    // the store keeps everything already written.
                    self.pause(&self.selected.code.clone());
                    Task::none()
                }
                ActionState::Paused(_) => self.enqueue(),
                // Leaving the queue is pausing: nothing has started, so there
                // is nothing to stop, but the intent is the same.
                ActionState::Queued => {
                    self.pause(&self.selected.code.clone());
                    Task::none()
                }
                ActionState::Play => self.preflight(Launch::Game),
                ActionState::Unknown => Task::none(),
            },

            Message::Tick => {
                if let Some(rx) = &self.setup_rx {
                    self.setup_sample = rx.borrow().clone();
                }
                if let Some(rx) = &self.addon_rx {
                    self.addon_sample = rx.borrow().clone();
                }
                if let Some(rx) = &self.scan_rx {
                    self.scan_sample = *rx.borrow();
                }
                if let Some(entry) = self
                    .queue
                    .iter_mut()
                    .find(|d| matches!(d.stage, Stage::Running { .. }))
                {
                    if let Stage::Running { rx, .. } = &entry.stage {
                        entry.sample = rx.borrow().clone();
                    }
                    match (entry.job.total, entry.sample.remaining) {
                        (None, remaining) => entry.job.total = remaining,
                        // Restored from before a restart, and a new build has
                        // come out since: the saved size no longer describes
                        // this job, so it starts over from what is left.
                        (Some(total), Some(remaining)) if remaining > total => {
                            entry.job = Job { total: Some(remaining), reached: 0 };
                        }
                        _ => {}
                    }
                    // Rate is measured on what this run has stored; the
                    // baseline is a constant offset and would only mute the
                    // first sample.
                    entry.rate.sample(entry.sample.progress.bytes_stored);

                    // What this build offers is only knowable once its
                    // manifest has been read, so a run is the one chance to
                    // learn it. Cached per product so the settings panel has
                    // something real to list.
                    let (code, offered) =
                        (entry.code.clone(), entry.sample.optional.clone());
                    if !offered.is_empty() {
                        self.remember_optional(&code, offered);
                    }
                }
                Task::none()
            }

            Message::CloseRequested(id) => {
                // A deep scan only reads; there is nothing to wind down.
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                }
                // Closing mid-transfer must not orphan what was fetched: trip
                // the token, let the run checkpoint its store, then close.
                match self.running().map(|d| d.code.clone()) {
                    Some(code) => {
                        // Saved as running a message ago; keep it that way.
                        self.queue_frozen = true;
                        self.pause(&code);
                        self.closing = Some(id);
                        self.status = "finishing the download safely…".to_owned();
                        Task::none()
                    }
                    None => iced::window::close(id),
                }
            }

            Message::Downloaded(code, result) => {
                self.downloads.finished(&code);
                if self.winding_down.as_ref() == Some(&code) {
                    self.winding_down = None;
                }
                if let Some(id) = self.closing.take() {
                    return iced::window::close(id);
                }

                // Status and errors describe the selected product. A transfer
                // that finished while the user was looking elsewhere updates
                // the queue and says nothing.
                let showing = code == self.selected.code;
                let entry = self.queue.iter().position(|d| d.code == code);
                let note = |app: &mut Self, text: String| {
                    if showing {
                        app.status = text;
                    }
                };

                // Resumed while this run was still winding down: the entry is
                // already a fresh, queued one. Marking it paused here — as
                // this handler once did — swallowed the resume, and whether it
                // happened came down to how fast the old run stopped.
                let resumed = entry.is_some_and(|at| matches!(self.queue[at].stage, Stage::Queued));
                let complete = matches!(&result, Ok(done) if done.complete);
                if resumed && !complete {
                    if let (Err(e), true) = (&result, showing) {
                        self.error = Some(e.clone());
                    }
                    return self.pump();
                }

                match result {
                    Ok(done) if done.complete => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
                        }
                        note(
                            self,
                            match done.version {
                                Some(v) => format!("installed {v}"),
                                None => format!("stored {} files", done.files),
                            },
                        );
                        Task::batch([self.pump(), self.reload_if_showing(&code)])
                    }
                    Ok(done) if done.cancelled => {
                        // Paused, so it keeps its place with what it reached.
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        note(self, "paused".to_owned());
                        self.pump()
                    }
                    Ok(_) => {
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        note(self, "stopped — run again to continue".to_owned());
                        Task::batch([self.pump(), self.reload_if_showing(&code)])
                    }
                    Err(e) => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
                        }
                        if showing {
                            self.error = Some(e);
                        }
                        self.pump()
                    }
                }
            }

            Message::Launched(result) => {
                self.busy = false;
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                // A launch may have paused transfers — its own product through
                // pre-flight's guard, and the rest of the queue through the
                // setting. Both are owed a restart, and resuming means
                // planning afresh.
                for code in std::mem::take(&mut self.resume_after_play) {
                    if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                        self.queue[at].stage = Stage::Queued;
                        self.queue[at].sample.stage = "queued".to_owned();
                    }
                }
                let owed = self.downloads.pending().contains(&self.selected.code);
                if owed {
                    if let Some(at) =
                        self.queue.iter().position(|d| d.code == self.selected.code)
                    {
                        self.queue[at].stage = Stage::Queued;
                    }
                }
                Task::batch([self.pump(), self.reload(self.selected.code.clone())])
            }

            Message::BattleNet => self.preflight(Launch::BattleNet),

            Message::OpenBattleNet => self.preflight(Launch::Client),

            Message::ToggleManager => {
                self.manager_open = !self.manager_open;
                if self.manager_open {
                    self.browse_open = false;
                    self.strip_open = false;
                }
                Task::none()
            }

            Message::ToggleHints => {
                self.hints = !self.hints;
                Task::none()
            }

            Message::ToggleStrip => {
                self.strip_open = !self.strip_open;
                if self.strip_open {
                    self.browse_open = false;
                    self.manager_open = false;
                }
                Task::none()
            }

            Message::ToggleBrowse => {
                self.browse_open = !self.browse_open;
                self.strip_open = false;
                if !self.browse_open {
                    return Task::none();
                }
                self.manager_open = false;
                // Read on open, so the list says what is already on the
                // machine without anything running in the background.
                Task::perform(
                    tasks::installed_versions(self.layout.clone()),
                    Message::InstalledVersions,
                )
            }

            Message::InstalledVersions(found) => {
                self.installed_versions = found;
                Task::none()
            }

            Message::KnownCodes(codes) => {
                self.known_codes = codes;
                self.refresh_catalog();
                Task::none()
            }

            Message::RemoveProduct(code) => {
                // The header cannot be empty: with nothing tracked there is no
                // product to show and no way back to this panel.
                if self.choices.len() <= 1 {
                    self.error =
                        Some("that is the only game left — add another first".to_owned());
                    return Task::none();
                }

                // A transfer for a product nobody is tracking has nowhere to
                // report to, so it stops here rather than running on unseen.
                self.pause(&code);
                self.queue.retain(|d| d.code != code);
                self.downloads.finished(&code);

                self.config.products.retain(|p| p.code != code);
                self.save_config();
                self.choices.retain(|c| c.code != code);
                self.last_branch.retain(|_, branch| branch != &code);

                match self.selected.code == code {
                    // Standing on the one just removed: move to whatever the
                    // header now starts with.
                    true => {
                        let next = self.choices[0].clone();
                        self.select(next)
                    }
                    false => Task::none(),
                }
            }

            Message::AddProduct(code) => {
                if self.choices.iter().any(|c| c.code == code) {
                    return Task::none();
                }
                let label = self
                    .catalog
                    .get(&code)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());

                self.config.products.push(Product::new(code.clone(), label.clone()));
                self.save_config();
                self.choices.push(Choice { code, label });
                Task::none()
            }

            Message::PauseDownload(code) => {
                self.pause(&code);
                Task::none()
            }

            Message::ResumeDownload(code) => {
                // Back to the end of the queue rather than jumping the line:
                // whatever is running was started first.
                if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                    self.queue[at].stage = Stage::Queued;
                    self.queue[at].sample.stage = "queued".to_owned();
                }
                self.pump()
            }

            Message::CancelDownload(code) => {
                self.pause(&code);
                self.queue.retain(|d| d.code != code);
                self.downloads.finished(&code);
                if self.queue.is_empty() {
                    self.manager_open = false;
                }
                self.pump()
            }

            Message::PreflightDone(what, result) => {
                if let Err(e) = result {
                    self.busy = false;
                    self.error = Some(e);
                    return Task::none();
                }

                // The long wait starts here, so say what it actually is. The
                // old code left "pre-flight…" on screen for the whole session
                // and the wind-down after it, which reads as a stuck launcher
                // rather than a running game.
                match what {
                    Launch::Game => {
                        // Names the tail too: the wine process tree takes a
                        // few seconds to wind down after the window closes,
                        // and an unexplained dead button in that window is
                        // exactly what reads as a hung launcher.
                        self.status =
                            "running — Play unlocks once the game has fully exited".to_owned();
                        Task::perform(
                            tasks::run_game(self.layout.clone(), self.selected.code.clone()),
                            Message::Launched,
                        )
                    }
                    Launch::BattleNet => {
                        self.status = "handing off to Battle.net…".to_owned();
                        Task::perform(
                            tasks::launch_via_battle_net(
                                self.layout.clone(),
                                self.selected.code.clone(),
                            ),
                            Message::Launched,
                        )
                    }
                    Launch::Client => {
                        self.status = "opening Battle.net…".to_owned();
                        Task::perform(
                            tasks::open_battle_net(
                                self.layout.clone(),
                                self.selected.code.clone(),
                            ),
                            Message::Launched,
                        )
                    }
                }
            }

            Message::ManageSubscription => {
                if let Err(e) = tasks::open_url(ACCOUNT_URL) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::OpenFiles => {
                let dir = self.layout.product_dir(&self.selected.code);
                let dir = if dir.is_dir() { dir } else { self.layout.games_root() };
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::Show(screen) => {
                self.screen = screen;
                self.error = None;
                Task::none()
            }

            Message::SetupStatus(status) => {
                let status = status.map(|boxed| *boxed);
                let measure = match &status {
                    Some(status) => Task::perform(
                        provision::measure(
                            status.installed.iter().map(|b| b.path.clone()).collect(),
                        ),
                        Message::ProtonSizes,
                    ),
                    None => Task::none(),
                };
                // Open on settings, whose first card is setup, when the machine
                // cannot launch anything yet — but never yank the user out of
                // a screen they chose.
                if let Some(status) = &status {
                    if !status.is_ready() && self.screen == Screen::Dashboard && !self.setup_running
                    {
                        self.screen = Screen::Settings;
                        self.settings_tab = SettingsTab::Setup;
                    }
                }
                // Installing Battle.net is what puts its mark in the prefix,
                // and setup is the only thing that installs it, so this is the
                // one moment the answer can have changed.
                self.bnet_icon = self.layout.battle_net_icon();
                self.setup = status;
                measure
            }

            Message::RunSetup => {
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;

                Task::perform(
                    provision::run(
                        self.layout.clone(),
                        self.config.clone(),
                        self.config.default_proton.version().map(str::to_owned),
                        tx,
                    ),
                    |result| Message::SetupFinished(result.map(Box::new)),
                )
            }

            Message::SetupFinished(result) => {
                self.setup_running = false;
                self.setup_rx = None;
                match result {
                    Ok(status) => {
                        let ready = status.is_ready();
                        self.setup = Some(*status);
                        if ready {
                            self.screen = Screen::Dashboard;
                        }
                        // Provisioning changes what the disk holds, so the
                        // action button has to be re-resolved.
                        self.reload(self.selected.code.clone())
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::PinBattleNetProton(choice) => {
                self.config.battle_net_proton = ProtonPin::from(choice.0);
                self.save_config();
                Task::none()
            }

            Message::PinDefaultProton(choice) => {
                self.config.default_proton = ProtonPin::from(choice.0);
                self.save_config();
                Task::none()
            }

            Message::PinProductProton(choice) => {
                if let Some(product) = self.config.product_mut(&self.selected.code) {
                    product.profile.proton_ge_version = choice.0;
                }
                self.save_config();
                Task::none()
            }

            Message::DownloadLimitChanged(value) => {
                // Digits only, so the field cannot hold something that is not
                // a speed. Clearing it is how you say "unlimited".
                self.limit_input = value.chars().filter(char::is_ascii_digit).collect();
                let trimmed = self.limit_input.trim();
                self.config.download_limit_mbps = match trimmed.parse::<u64>() {
                    Ok(0) | Err(_) => None,
                    Ok(mbps) => Some(mbps),
                };
                self.save_config();
                Task::none()
            }

            Message::BattleNetLimitRead(mbps) => {
                self.bnet_limit_input = mbps.map(|v| v.to_string()).unwrap_or_default();
                Task::none()
            }

            Message::BattleNetLimitChanged(value) => {
                self.bnet_limit_input = value.chars().filter(char::is_ascii_digit).collect();
                let mbps = self.bnet_limit_input.trim().parse::<u64>().ok().filter(|v| *v > 0);
                Task::perform(
                    tasks::set_battle_net_limit(
                        self.layout.clone(),
                        self.config.prefix_user.clone(),
                        mbps,
                    ),
                    Message::BattleNetLimitWritten,
                )
            }

            Message::BattleNetLimitWritten(result) => {
                if let Err(e) = result {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::PauseWhilePlaying(on) => {
                self.config.pause_downloads_while_playing = on;
                self.save_config();
                Task::none()
            }

            Message::CheckProton => {
                // Runs with no build installed too: the release list is what
                // fills the install picker, and needing a build before you can
                // find one to install is the wrong way round.
                let installed = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.proton.as_ref())
                    .map(|p| p.version.clone());
                self.proton_notice = Some("checking…".to_owned());
                Task::perform(
                    provision::check_proton(self.http.clone(), installed),
                    Message::ProtonChecked,
                )
            }

            Message::ProtonChecked(result) => {
                match result {
                    Ok(releases) => {
                        self.proton_notice = Some(match &releases.update {
                            // Notify-only by policy: nothing updates itself.
                            Some(version) => format!("{version} is available"),
                            None => match self.setup.as_ref().and_then(|s| s.proton.as_ref()) {
                                Some(_) => "up to date".to_owned(),
                                None => "nothing installed yet".to_owned(),
                            },
                        });
                        self.proton_available = releases.available;
                    }
                    Err(e) => self.proton_notice = Some(format!("check failed: {e}")),
                }
                Task::none()
            }

            Message::ChooseProtonVersion(choice) => {
                self.proton_choice = choice.0;
                Task::none()
            }

            Message::InstallProton => {
                let Some(version) = self.proton_choice.clone() else {
                    return Task::none();
                };
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;
                self.screen = Screen::Settings;
                self.settings_tab = SettingsTab::Setup;

                Task::perform(
                    provision::run(
                        self.layout.clone(),
                        self.config.clone(),
                        Some(version),
                        tx,
                    ),
                    |result| Message::SetupFinished(result.map(Box::new)),
                )
            }

            Message::RemoveProton(version) => {
                let Some(build) = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.installed.iter().find(|b| b.version == version))
                    .cloned()
                else {
                    return Task::none();
                };
                self.proton_notice = Some(format!("removing {version}…"));
                Task::perform(
                    provision::remove_proton(self.config.clone(), build),
                    Message::ProtonRemoved,
                )
            }

            Message::ProtonRemoved(result) => {
                match result {
                    Ok(note) => {
                        self.proton_notice = Some(note);
                        // What is installed just changed, so re-probe rather
                        // than editing the list we happen to be holding.
                        return Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        );
                    }
                    Err(e) => self.proton_notice = Some(e),
                }
                Task::none()
            }

            Message::ProtonSizes(sizes) => {
                self.proton_sizes = sizes.into_iter().collect();
                Task::none()
            }

            Message::AddonsListed(listing) => {
                self.addons = *listing;
                Task::none()
            }

            Message::TokenChanged(value) => {
                self.token_input = value;
                Task::none()
            }

            Message::ImportToken => {
                let token = self.token_input.trim().to_owned();
                if token.is_empty() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.error = None;

                Task::perform(
                    addonwork::import(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        token,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::UpdateAddons => {
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.error = None;

                Task::perform(
                    addonwork::update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::AddonsFinished(result) => {
                self.addons_busy = false;
                self.addon_rx = None;
                match result {
                    Ok(report) => {
                        self.addon_report = Some(*report);
                        // The list on screen is now stale in both directions.
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::RemoveAddon(provider, id) => Task::perform(
                addonwork::remove(
                    self.layout.clone(),
                    self.selected.code.clone(),
                    provider,
                    id,
                ),
                Message::AddonRemoved,
            ),

            Message::AddonRemoved(result) => {
                match result {
                    Ok(folders) => self.status = format!("removed {}", folders.join(", ")),
                    Err(e) => self.error = Some(e),
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::OpenUrl(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::AddonTab(tab) => {
                self.addon_tab = tab;
                self.error = None;
                Task::none()
            }

            Message::SettingsTab(tab) => {
                self.settings_tab = tab;
                self.error = None;
                match tab {
                    SettingsTab::Storage => self.measure_storage(),
                    _ => Task::none(),
                }
            }

            Message::StorageMeasured(report) => {
                self.storage_measuring = false;
                if self.location_input.is_empty() {
                    self.location_input = report.root.display().to_string();
                }
                self.storage = Some(*report);
                Task::none()
            }

            Message::InstallOffered(code, result) => {
                if let Ok(offerings) = result.as_ref() {
                    self.remember_optional(&code, offerings.optional.clone());
                }
                if let Some(dialog) = self.install_dialog.as_mut().filter(|d| d.code == code) {
                    dialog.stage = match *result {
                        Ok(offerings) => DialogStage::Ready(Box::new(offerings)),
                        Err(e) => DialogStage::Failed(e),
                    };
                }
                Task::none()
            }

            Message::InstallHighRes(on) => {
                if let Some(dialog) = self.install_dialog.as_mut() {
                    dialog.high_res = on;
                }
                Task::none()
            }

            Message::GamesFolderChanged(value) => {
                let disk = parse_path(&value).ok().and_then(|p| storage::disk(&p).ok());
                if let Some(dialog) = self.install_dialog.as_mut() {
                    dialog.disk = disk;
                }
                self.games_input = value;
                Task::none()
            }

            Message::InstallCancel => {
                self.install_dialog = None;
                Task::none()
            }

            Message::InstallStart => self.start_install(),

            Message::AskUninstall => {
                self.gear_open = false;
                let code = self.selected.code.clone();
                self.uninstall_dialog = Some(UninstallDialog {
                    code: code.clone(),
                    label: self.selected.label.clone(),
                    plan: None,
                    working: false,
                });
                Task::perform(
                    tasks::uninstall_plan(self.http.clone(), self.layout.clone(), code.clone()),
                    move |plan| Message::UninstallPlanned(code.clone(), Box::new(plan)),
                )
            }

            Message::UninstallPlanned(code, plan) => {
                if let Some(dialog) = self.uninstall_dialog.as_mut().filter(|d| d.code == code) {
                    dialog.plan = Some(*plan);
                }
                Task::none()
            }

            Message::UninstallCancel => {
                self.uninstall_dialog = None;
                Task::none()
            }

            Message::UninstallConfirm => {
                let Some(dialog) = self.uninstall_dialog.as_mut() else {
                    return Task::none();
                };
                dialog.working = true;
                let code = dialog.code.clone();
                let http = self.http.clone();
                Task::perform(
                    tasks::uninstall(http, self.layout.clone(), code.clone()),
                    move |result| Message::Uninstalled(code.clone(), result),
                )
            }

            Message::Uninstalled(code, result) => {
                let label = self.uninstall_dialog.take().map(|d| d.label).unwrap_or_default();
                match result {
                    Ok(freed) => {
                        // A paused or queued download of it would only put
                        // the files straight back.
                        self.queue.retain(|d| d.code != code);
                        if code == self.selected.code {
                            self.status = format!("uninstalled {label} — freed {}", size_of(freed));
                        }
                        Task::batch([
                            Task::perform(
                                tasks::installed_versions(self.layout.clone()),
                                Message::InstalledVersions,
                            ),
                            self.reload_if_showing(&code),
                        ])
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::ClearCache => {
                let Some(dirs) = self.dirs.clone() else {
                    return Task::none();
                };
                self.cache_notice = Some("clearing…".to_owned());
                Task::perform(tasks::clear_cache(dirs), Message::CacheCleared)
            }

            Message::CacheCleared(result) => {
                self.cache_notice = Some(match result {
                    Ok(freed) => format!("freed {}", size_of(freed)),
                    Err(e) => e,
                });
                self.measure_storage()
            }

            Message::OpenConfigFolder => {
                if let Some(dirs) = &self.dirs {
                    if let Err(e) = tasks::open_path(dirs.config_dir().to_path_buf()) {
                        self.status = e;
                    }
                }
                Task::none()
            }

            Message::OpenLibrary => {
                let root = self.layout.root().to_path_buf();
                let dir = match root.is_dir() {
                    true => root,
                    // Not created yet: show where it would go.
                    false => root
                        .ancestors()
                        .find(|p| p.is_dir())
                        .map(Path::to_path_buf)
                        .unwrap_or(root),
                };
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::LocationChanged(value) => {
                self.location_input = value;
                Task::none()
            }

            Message::UseLocation => {
                match self.relocate() {
                    Ok(()) => Task::batch([
                        Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        ),
                        self.measure_storage(),
                        self.reload(self.selected.code.clone()),
                    ]),
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::SearchChanged(value) => {
                self.search_input = value;
                Task::none()
            }

            Message::RunSearch => {
                let query = self.search_input.trim().to_owned();
                if query.is_empty() {
                    return Task::none();
                }
                self.searching = true;
                Task::perform(
                    addonwork::search(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        query,
                    ),
                    |outcome| Message::SearchDone(Box::new(outcome)),
                )
            }

            Message::SearchDone(outcome) => {
                self.searching = false;
                // Ignore a reply for a query the user has already moved past.
                if outcome.query == self.search_input.trim() {
                    self.search = *outcome;
                }
                Task::none()
            }

            Message::InstallAddon(provider, id) => {
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.error = None;

                Task::perform(
                    addonwork::install_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::CheckAddonUpdates => Task::perform(
                addonwork::check_updates(
                    self.http.clone(),
                    self.layout.clone(),
                    self.selected.code.clone(),
                ),
                Message::AddonUpdatesChecked,
            ),

            Message::AddonUpdatesChecked(result) => {
                match result {
                    Ok(statuses) => self.updates = statuses,
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }

            Message::CopyDebug => {
                let action = self.action();
                let info = DebugInfo::gather(
                    &self.layout,
                    &self.selected.code,
                    &format!("{action:?}"),
                    tasks::proton_for(&self.selected.code).as_ref(),
                );
                self.status = "debug info copied".to_owned();
                iced::clipboard::write(info.render())
            }

            Message::Tweak(which, on) => {
                self.edit_tweaks(|tweaks| match which {
                    // Stored as an explicit refusal rather than a bare `false`:
                    // the default is on wherever the tool exists, so "off" has
                    // to be recorded to survive a restart.
                    Tweak::GamePerformance => tweaks.game_performance = Some(on),
                    Tweak::GameMode => tweaks.gamemode = on,
                    Tweak::MangoHud => tweaks.mangohud = on,
                    Tweak::SkipAddonProfiling => tweaks.skip_addon_profiling = on,
                    Tweak::ReduceRaidEffects => tweaks.reduce_raid_effects = on,
                    Tweak::FasterZoneLoads => tweaks.faster_zone_loads = on,
                    Tweak::RawMouse => tweaks.raw_mouse = on,
                    Tweak::NoScreenEffects => tweaks.no_screen_effects = on,
                    Tweak::MaxDrawDistance => tweaks.max_draw_distance = on,
                });
                Task::none()
            }

            Message::TweakApi(ApiChoice(api)) => {
                self.edit_tweaks(|tweaks| tweaks.graphics_api = api);
                Task::none()
            }

            Message::TweakFpsCap(value) => {
                // Digits only: a frame cap that is not a number is not a cap.
                // Clearing it is how you say uncapped.
                self.fps_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(|tweaks| tweaks.fps_cap = cap);
                Task::none()
            }

            Message::TweakSpellQueue(value) => {
                self.spell_input = value.chars().filter(char::is_ascii_digit).collect();
                let ms = self.spell_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(|tweaks| tweaks.spell_queue_ms = ms);
                Task::none()
            }

            Message::TweakEnvKey(value) => {
                // No spaces or `=`: the two halves are separate fields here,
                // and a key holding either would not survive the environment.
                self.env_key_input =
                    value.chars().filter(|c| !c.is_whitespace() && *c != '=').collect();
                Task::none()
            }

            Message::TweakEnvValue(value) => {
                self.env_value_input = value;
                Task::none()
            }

            Message::TweakEnvAdd => {
                let key = std::mem::take(&mut self.env_key_input);
                let value = std::mem::take(&mut self.env_value_input);
                if !key.is_empty() {
                    self.edit_tweaks(|tweaks| {
                        tweaks.env.insert(key, value);
                    });
                }
                Task::none()
            }

            Message::TweakEnvRemove(key) => {
                self.edit_tweaks(|tweaks| {
                    tweaks.env.remove(&key);
                });
                Task::none()
            }

            Message::OptionalContent(name, on) => {
                self.set_optional(&self.selected.code.clone(), &name, on);
                Task::none()
            }

            Message::ToggleGear => {
                self.gear_open = !self.gear_open;
                Task::none()
            }

            Message::Dismiss => {
                self.gear_open = false;
                self.manager_open = false;
                self.browse_open = false;
                self.strip_open = false;
                Task::none()
            }

            Message::MenuClick => Task::none(),

            Message::CheckForUpdates => {
                self.gear_open = false;
                self.busy = true;
                self.error = None;
                self.status = "checking for updates…".to_owned();
                Task::perform(
                    tasks::offerings(
                        self.http.clone(),
                        self.selected.code.clone(),
                        self.region,
                        self.opted_optional(&self.selected.code),
                        None,
                    ),
                    |result| Message::CheckedForUpdates(Box::new(result)),
                )
            }

            Message::CheckedForUpdates(result) => {
                self.busy = false;
                match *result {
                    Ok(offerings) => {
                        let code = self.selected.code.clone();
                        // The point of the round trip: the optional list is
                        // only knowable from the manifest, and this is the
                        // cheap way to learn it.
                        self.remember_optional(&code, offerings.optional.clone());
                        let current = self.local.version.as_deref() == Some(
                            offerings.remote.versions_name.as_str(),
                        );
                        self.remote = Some(offerings.remote.clone());
                        self.status = match current {
                            true => format!("up to date on {}", offerings.remote.versions_name),
                            false => {
                                format!("{} is available", offerings.remote.versions_name)
                            }
                        };
                    }
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }

            Message::ScanAndRepair => {
                self.gear_open = false;
                self.error = None;
                self.scanning = true;
                self.busy = true;
                self.scan_sample = Default::default();
                self.status = "checking stored files against their headers…".to_owned();

                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(
                    tasks::scan_and_repair(self.layout.clone(), tx),
                    |result| Message::Scanned(Box::new(result)),
                )
            }

            Message::DeepScan => {
                self.gear_open = false;
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                    self.status = "stopping the deep scan…".to_owned();
                    return Task::none();
                }
                // Every WoW flavour shares one store, so any transfer at all
                // is writing to what this would read.
                let transferring = self.winding_down.is_some()
                    || self
                        .queue
                        .iter()
                        .any(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued));
                if transferring {
                    self.error = Some(
                        "pause downloads first — they write to the files being checked".to_owned(),
                    );
                    return Task::none();
                }

                self.error = None;
                self.scanning = true;
                self.owed_repair = None;
                self.scan_sample = Default::default();
                self.status = "checking every stored byte — the game can stay open".to_owned();

                let cancel = Cancel::new();
                self.deep_cancel = Some(cancel.clone());
                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(tasks::deep_scan(self.layout.clone(), cancel, tx), |result| {
                    Message::DeepScanned(Box::new(result))
                })
            }

            Message::DeepScanned(result) => {
                self.scanning = false;
                self.scan_rx = None;
                let stopped = self.deep_cancel.take().is_some_and(|c| c.is_cancelled());
                match *result {
                    Err(_) if stopped => {
                        self.status = "deep scan stopped".to_owned();
                        self.pump()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.pump()
                    }
                    Ok(report) if report.is_healthy() => {
                        self.status = format!(
                            "{} files checked down to the byte — none damaged",
                            report.checked
                        );
                        self.pump()
                    }
                    // Repaired before the queue is released: a transfer
                    // landing first would move the store and void the
                    // findings.
                    Ok(report) => {
                        self.status = format!(
                            "{} damaged of {} — repairing…",
                            report.damaged.len(),
                            report.checked
                        );
                        self.owed_repair = Some(Box::new(report));
                        self.update(Message::RepairDamaged)
                    }
                }
            }

            Message::RepairDamaged => {
                self.gear_open = false;
                let Some(report) = self.owed_repair.clone() else {
                    return Task::none();
                };
                // Held only for the index rewrite — milliseconds — since that
                // is the one step a launching client must not overlap.
                self.busy = true;
                Task::perform(tasks::repair(self.layout.clone(), *report), Message::Repaired)
            }

            Message::Repaired(result) => {
                self.busy = false;
                match result {
                    Ok(removed) => {
                        self.owed_repair = None;
                        self.status =
                            format!("{removed} damaged files dropped — fetching them again");
                        self.enqueue()
                    }
                    Err(tasks::Unrepaired::InUse) => {
                        let damaged = self.owed_repair.as_ref().map_or(0, |r| r.damaged.len());
                        self.status = format!(
                            "{damaged} damaged files found — close the game and Battle.net, \
                             then choose Repair Damaged Files"
                        );
                        self.pump()
                    }
                    Err(tasks::Unrepaired::Failed(e)) => {
                        self.owed_repair = None;
                        self.error = Some(e);
                        self.pump()
                    }
                }
            }

            Message::Scanned(result) => {
                self.scanning = false;
                self.busy = false;
                self.scan_rx = None;
                match *result {
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                    Ok(scan) => {
                        // Either way the download path runs: repair works by
                        // removing damaged entries so the planner counts them
                        // missing, and the same plan also covers whatever was
                        // never fetched in the first place.
                        self.status = match scan.damaged {
                            0 => format!(
                                "{} files verified — none damaged; checking for anything missing",
                                scan.checked
                            ),
                            n => format!(
                                "{n} damaged of {} — {} dropped, fetching them again",
                                scan.checked, scan.removed
                            ),
                        };
                        self.enqueue()
                    }
                }
            }

            Message::ClearShaderCache => {
                let Some(dirs) = self.dirs.as_ref() else {
                    self.error = Some("no cache directory available".to_owned());
                    return Task::none();
                };
                // Per product, so this cannot take another product's shaders
                // with it — the reason the cache is keyed that way at all.
                let dir = dirs.shader_cache_dir(&self.selected.code);
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => self.status = "shader cache cleared".to_owned(),
                    // Nothing there is the desired end state, not a failure.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.status = "no shader cache to clear".to_owned()
                    }
                    Err(e) => self.error = Some(format!("{}: {e}", dir.display())),
                }
                Task::none()
            }
        }
    }

    /// Record what a build offers, if it has changed.
    ///
    /// Saved rather than held in memory: the list is the only thing standing
    /// between the settings panel and a hardcoded guess, and a guess is what
    /// put a 37 GiB texture pack in a plan nobody asked for.
    /// The optional downloads a product is opted into, limited to the ones
    /// Lightforge offers — see [`OFFERED_OPTIONAL`]. An opt-in to anything
    /// else, from before that list existed, is ignored rather than fetched
    /// behind a switch the user can no longer see.
    fn opted_optional(&self, code: &ProductCode) -> Vec<String> {
        offered_only(
            self.config.product(code).map(|p| p.profile.optional_content.as_slice()).unwrap_or(&[]),
        )
    }

    fn remember_optional(&mut self, code: &ProductCode, offered: Vec<String>) {
        let changed = self
            .config
            .product(code)
            .is_some_and(|p| p.profile.optional_available != offered);
        if !changed {
            return;
        }
        if let Some(product) = self.config.product_mut(code) {
            product.profile.optional_available = offered;
        }
        self.save_config();
    }

    /// This product's performance settings.
    fn tweaks(&self) -> Tweaks {
        self.config
            .product(&self.selected.code)
            .map(|p| p.profile.tweaks.clone())
            .unwrap_or_default()
    }

    /// Change them and persist, which is what every switch on the panel does.
    fn edit_tweaks(&mut self, edit: impl FnOnce(&mut Tweaks)) {
        if let Some(product) = self.config.product_mut(&self.selected.code) {
            edit(&mut product.profile.tweaks);
        }
        self.save_config();
    }

    /// Refill the numeric fields from the selected product's settings.
    fn load_tweak_inputs(&mut self) {
        let tweaks = self.tweaks();
        self.fps_input = tweaks.fps_cap.map(|v| v.to_string()).unwrap_or_default();
        self.spell_input = tweaks.spell_queue_ms.map(|v| v.to_string()).unwrap_or_default();
        self.env_key_input.clear();
        self.env_value_input.clear();
    }

    /// Persist the config, surfacing a write failure rather than swallowing it.
    /// The queue as the next start should find it.
    fn queue_snapshot(&self) -> transfers::Queue {
        // Paused only because a game is running: owed a restart, and one the
        // next start should give it rather than leave it waiting.
        let owed = {
            let mut owed = self.downloads.pending();
            owed.extend(self.resume_after_play.iter().cloned());
            owed
        };
        let entries = self
            .queue
            .iter()
            .map(|d| {
                let (standing, reached) = match &d.stage {
                    _ if owed.contains(&d.code) => (Standing::Running, 0),
                    Stage::Running { .. } => (Standing::Running, 0),
                    Stage::Queued => (Standing::Queued, d.job.reached),
                    Stage::Paused(p) => (Standing::Paused, p.downloaded_bytes),
                };
                Saved { code: d.code.clone(), standing, job_bytes: d.job.total, reached }
            })
            .collect();
        transfers::Queue { entries }
    }

    /// Write the queue if it changed. Called after every message, which is
    /// what makes a crash or a kill keep it too, not only a clean close.
    fn persist_queue(&mut self) {
        if self.queue_frozen || self.config_error.is_some() {
            return;
        }
        let snapshot = self.queue_snapshot();
        if snapshot == self.saved_queue {
            return;
        }
        if let Some(dirs) = &self.dirs {
            if let Err(e) = snapshot.save(&dirs.queue_file()) {
                trace::mark(format_args!("saving the queue failed: {e}"));
                return;
            }
        }
        self.saved_queue = snapshot;
    }

    /// Put back the queue the last run left, for products still configured.
    fn restore_queue(&mut self) {
        let Some(dirs) = &self.dirs else {
            return;
        };
        let saved = match transfers::Queue::load(&dirs.queue_file()) {
            Ok(saved) => saved,
            Err(e) => {
                trace::mark(format_args!("ignoring the saved queue: {e}"));
                return;
            }
        };
        for entry in &saved.entries {
            let Some(choice) = self.choices.iter().find(|c| c.code == entry.code) else {
                continue;
            };
            let job = Job { total: entry.job_bytes, reached: entry.reached };
            let stage = match entry.standing {
                Standing::Running | Standing::Queued => Stage::Queued,
                Standing::Paused => Stage::Paused(state::Progress {
                    downloaded_bytes: entry.reached,
                    total_bytes: entry.job_bytes.unwrap_or(0),
                    bytes_per_second: 0,
                }),
            };
            self.queue.push(Download {
                code: entry.code.clone(),
                label: choice.label.clone(),
                stage,
                sample: TransferState { stage: "queued".to_owned(), ..TransferState::default() },
                rate: Rate::default(),
                job,
            });
        }
        self.saved_queue = self.queue_snapshot();
    }

    /// Open the install dialog and start pricing the build.
    fn open_install(&mut self) -> Task<Message> {
        let code = self.selected.code.clone();
        self.games_input = self.layout.games_root().display().to_string();
        self.install_dialog = Some(InstallDialog {
            code: code.clone(),
            label: self.selected.label.clone(),
            stage: DialogStage::Calculating,
            high_res: self.opted_optional(&code).iter().any(|t| t == HIGH_RES),
            movable: storage::game_folder_is_empty(&self.layout),
            disk: storage::disk(&self.layout.games_root()).ok(),
        });
        // Priced with no opt-ins, since the dialog adds each extra's own size,
        // and against the store already there, so files another WoW version
        // shares are not counted.
        Task::perform(
            tasks::offerings(
                self.http.clone(),
                code.clone(),
                self.region,
                Vec::new(),
                Some(self.layout.wow_install()),
            ),
            move |result| Message::InstallOffered(code.clone(), Box::new(result)),
        )
    }

    /// Apply the dialog's answers and queue the install.
    fn start_install(&mut self) -> Task<Message> {
        let Some(dialog) = self.install_dialog.clone() else {
            return Task::none();
        };
        if dialog.code != self.selected.code {
            self.install_dialog = None;
            return Task::none();
        }
        // Re-checked rather than trusted from when the dialog opened.
        if dialog.movable && storage::game_folder_is_empty(&self.layout) {
            if let Err(e) = self.move_game_folder() {
                self.error = Some(e);
                return Task::none();
            }
        }
        if let DialogStage::Ready(offerings) = &dialog.stage {
            let offered = offerings.optional_bytes.iter().any(|(t, b)| t == HIGH_RES && *b > 0);
            if offered {
                self.set_optional(&dialog.code, HIGH_RES, dialog.high_res);
            }
        }
        self.install_dialog = None;
        self.enqueue()
    }

    /// Point the game folder at what the dialog holds.
    ///
    /// Only while no WoW version is installed: then moving it is a config
    /// change plus re-pointing the prefix's link, which is how Wine — and
    /// Battle.net and Agent through it — find the game at its usual Windows
    /// path wherever it really is.
    fn move_game_folder(&mut self) -> Result<(), String> {
        let path = parse_path(&self.games_input)?;
        if path == self.layout.games_root() {
            return Ok(());
        }
        let default = self.layout.clone().with_games(None).games_root();
        self.config.games_dir = (path != default).then(|| path.clone());
        self.save_config();
        self.layout = self.layout.clone().with_games(self.config.games_dir.clone());

        // Before setup there is no prefix to link from, and setup makes the
        // link itself; creating `drive_c` early would get in wineboot's way.
        if lightforge_core::prefix::PrefixStatus::inspect(&self.layout).initialized {
            lightforge_core::prefix::ensure_link(
                &self.layout.wow_symlink(),
                &self.layout.wow_install(),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Opt one product in or out of one optional download.
    fn set_optional(&mut self, code: &ProductCode, name: &str, on: bool) {
        if let Some(product) = self.config.product_mut(code) {
            let list = &mut product.profile.optional_content;
            list.retain(|n| n != name);
            if on {
                list.push(name.to_owned());
                list.sort();
            }
        }
        self.save_config();
    }

    /// Re-measure what Settings > Storage shows, off the UI thread.
    fn measure_storage(&mut self) -> Task<Message> {
        let Some(dirs) = self.dirs.clone() else {
            return Task::none();
        };
        self.storage_measuring = true;
        Task::perform(tasks::storage(self.layout.clone(), dirs), |report| {
            Message::StorageMeasured(Box::new(report))
        })
    }

    /// Point the library at the typed location.
    ///
    /// Only while nothing is installed in the current one: then it is a
    /// config change. With games or a prefix in it, it would be a data
    /// migration — re-pointing the prefix's game symlink, moving 100+ GB —
    /// and leaving the old library behind unannounced is not an answer.
    fn relocate(&mut self) -> Result<(), String> {
        let path = parse_path(&self.location_input)?;
        if path == self.layout.root() {
            return Ok(());
        }
        let downloading = self.queue.iter().any(|d| matches!(d.stage, Stage::Running { .. }));
        if self.setup_running || downloading {
            return Err("wait for setup and downloads to finish first".to_owned());
        }
        if !storage::library_is_empty(&self.layout) {
            return Err("the current library has games or a prefix in it; moving an \
                        installed library is not supported yet"
                .to_owned());
        }

        let default = Layout::default_for_user().ok();
        self.config.blizzard_root = match default.as_ref().map(Layout::root) {
            Some(root) if root == path => None,
            _ => Some(path.clone()),
        };
        self.save_config();
        self.layout = Layout::with_root(&path).with_games(self.config.games_dir.clone());
        self.location_input = path.display().to_string();
        self.error = None;
        Ok(())
    }

    fn save_config(&mut self) {
        // Never over a file that did not load: it is the user's to fix.
        if self.config_error.is_some() {
            return;
        }
        let Some(dirs) = self.dirs.as_ref() else {
            self.error = Some("no config directory available".to_owned());
            return;
        };
        if let Err(e) = self.config.save(&dirs.config_file()) {
            self.error = Some(e.to_string());
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        // Only while something is moving: an idle dashboard should wake up for
        // nothing.
        let closing = iced::window::close_requests().map(Message::CloseRequested);

        // The hint chord. `on_key_press` only produces a message when the
        // closure returns one, so an idle window is not woken by typing.
        let hints = iced::keyboard::on_key_press(|key, modifiers| {
            hints_chord(&key, modifiers).then_some(Message::ToggleHints)
        });

        // Resize events are only interesting to a human reading a trace, and
        // subscribing to them makes every resize step rebuild the view, so
        // this stays out of a normal run entirely.
        let resizes = match trace::frames() {
            true => iced::window::resize_events().map(|(_, size)| Message::ResizeTraced(size)),
            false => Subscription::none(),
        };

        match self.running().is_some() || self.setup_running || self.addons_busy || self.scanning {
            true => Subscription::batch([
                closing,
                hints,
                resizes,
                iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            ]),
            false => Subscription::batch([closing, hints, resizes]),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        use std::sync::atomic::AtomicBool;
        static FIRST: AtomicBool = AtomicBool::new(false);
        static THEMED: AtomicBool = AtomicBool::new(false);
        trace::once(&FIRST, "first frame");
        if self.branding.is_some() {
            trace::once(&THEMED, "first frame with branding");
        }

        let started = Instant::now();
        if let Some(error) = &self.config_error {
            return self.broken_config_screen(error);
        }
        let screen = match self.screen {
            Screen::Dashboard => self.dashboard(),
            Screen::Settings => self.settings(),
            Screen::Addons => self.addons_screen(),
            Screen::Tweaks => self.tweaks_screen(),
        };

        // The manager hangs under its button on every screen, not just the
        // dashboard: a download keeps running while you are in Settings, and
        // that is exactly when you want to look at it. iced 0.13 has no
        // popover, so it is a stacked layer pinned to the top right, padded
        // clear of the ribbon it drops out of.
        let overlay: Option<Element<Message>> =
            match (self.manager_open && !self.queue.is_empty(), self.browse_open) {
                (true, _) => Some(self.manager()),
                (_, true) => Some(self.browse()),
                _ => None,
            };
        // Clicks inside a panel are absorbed so they never reach the dismissal
        // beneath it; without this, clicking the gap between two rows of the
        // download manager would close it.
        let overlay = overlay.map(|panel| {
            Element::from(mouse_area(panel).on_press(Message::MenuClick))
        });

        let screen = match overlay {
            Some(panel) => stack![
                screen,
                container(panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding([theme::XL + theme::LG, theme::LG]),
            ]
            .into(),
            None => screen,
        };


        // Modal: over everything, including the panels above. The scrim takes
        // every press that the card itself does not, and a `Stack` stops
        // delivering an event once a layer captures it, so nothing beneath
        // can be clicked while the dialog is up.
        let card = match (&self.install_dialog, &self.uninstall_dialog) {
            (Some(dialog), _) => Some(self.install_card(dialog)),
            (_, Some(dialog)) => Some(self.uninstall_card(dialog)),
            _ => None,
        };
        let screen = match card {
            Some(card) => stack![
                screen,
                mouse_area(container(card).center(Length::Fill).style(theme::scrim))
                    .on_press(Message::MenuClick),
            ]
            .into(),
            None => screen,
        };

        // Anything open closes on a click that no widget wanted. `mouse_area`
        // hands the event to its child first and only fires when the child
        // ignored it, so a button still does its own job rather than being
        // swallowed by the dismissal.
        let screen = match self.gear_open || self.manager_open || self.browse_open || self.strip_open {
            true => Element::from(mouse_area(screen).on_press(Message::Dismiss)),
            false => screen,
        };

        trace::view_built(started.elapsed());
        screen
    }

    /// Title, product picker, and navigation.
    fn header(&self) -> Element<'_, Message> {
        let nav = |label: &'static str, screen: Screen| {
            let entry = button(text(label).size(theme::LABEL)).padding([theme::SM, theme::MD]);
            // Addons and Game Optimizations are a game's own pages, opened
            // from its panel, so Play stays lit on them — and stays clickable,
            // since it is the way back.
            let within = match screen {
                Screen::Dashboard => matches!(self.screen, Screen::Addons | Screen::Tweaks),
                _ => false,
            };
            match (self.screen == screen, within) {
                (true, _) => entry.style(theme::selected),
                (false, true) => entry.style(theme::selected).on_press(Message::Show(screen)),
                (false, false) => entry.style(theme::ghost).on_press(Message::Show(screen)),
            }
        };

        let mut bar = row![text("LIGHTFORGE").size(theme::TITLE)]
            .align_y(Alignment::Center)
            .spacing(theme::SM);

        if self.screen == Screen::Dashboard {
            bar = bar.push(Space::with_width(Length::Fixed(theme::MD as f32)));

            // The band takes the row's slack, which is also what pushes the
            // navigation right — the spacer below is for the screens that
            // have no band. Its height is pinned because a widget that fills
            // its cross axis is given the height of its *siblings*, and the
            // tiles are taller than the buttons beside them.
            bar = bar.push(
                container(responsive(|space| self.strip(space.width)))
                    .width(Length::Fill)
                    .height(Length::Fixed(STRIP_HEIGHT)),
            );
        } else {
            bar = bar.push(Space::with_width(Length::Fill));
        }

        // Only while something is in flight. An idle launcher has no
        // downloads to manage, and a permanent button that opens an empty
        // panel is furniture.
        if !self.queue.is_empty() {
            let count = self.queue.len();
            bar = bar.push(
                button(
                    text(match count {
                        1 => "↓ 1".to_owned(),
                        n => format!("↓ {n}"),
                    })
                    .size(theme::LABEL),
                )
                .padding([theme::SM, theme::MD])
                .style(match self.manager_open {
                    true => theme::selected,
                    false => theme::ghost,
                })
                .on_press(Message::ToggleManager),
            );
            bar = bar.push(Space::with_width(Length::Fixed(theme::SM as f32)));
        }

        // App-level, not per-product: everything outside the WoW family is a
        // hand-off into Battle.net, so it sits with the navigation rather
        // than on a panel that describes one product.
        // The mark comes out of the prefix rather than the binary: see
        // [`Layout::battle_net_icon`]. Before Battle.net is installed there is
        // none, and the button is the word alone.
        let mut face = row![].spacing(theme::SM).align_y(Alignment::Center);
        if let Some(path) = &self.bnet_icon {
            face = face.push(
                image(image::Handle::from_path(path))
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fixed(BAND_GLYPH))
                    .height(Length::Fixed(BAND_GLYPH)),
            );
        }
        let face = face.push(text("Battle.net").size(theme::LABEL));

        bar = bar.push(
            tooltip(
                button(face)
                    .padding([theme::SM, theme::MD])
                    .style(theme::ghost)
                    .on_press_maybe((!self.busy).then_some(Message::OpenBattleNet)),
                container(
                    text("Open Battle.net for other Blizzard titles".to_owned())
                        .size(theme::LABEL),
                )
                .padding([theme::XS, theme::SM])
                .style(theme::floating),
                tooltip::Position::Bottom,
            ),
        );
        bar = bar.push(Space::with_width(Length::Fixed(theme::SM as f32)));

        bar = bar.push(nav("Play", Screen::Dashboard));
        bar = bar.push(nav("Settings", Screen::Settings));

        column![
            container(bar).width(Length::Fill).padding([theme::SM, theme::LG]).style(theme::ribbon),
            container(Space::with_height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::ribbon_edge),
        ]
        .into()
    }

    /// The configured lines, in the order their first product appears.
    ///
    /// A line, not a product: PTR and beta are builds of a game, not games
    /// beside it, so five World of Warcrafts collapse to three tiles and a
    /// version picker.
    fn lines(&self) -> Vec<ProductLine> {
        let mut lines: Vec<ProductLine> = Vec::new();
        for choice in &self.choices {
            let line = self.catalog.line_of(&choice.code);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        lines
    }

    /// Rebuild the catalog from whatever manifest is in hand.
    fn refresh_catalog(&mut self) {
        let mut catalog = match &self.manifest {
            Some(manifest) => manifest.catalog(),
            None => Catalog::builtin(),
        };
        catalog.reconcile(&self.known_codes);
        self.catalog = catalog;
    }

    /// The catalog: everything that could be added to the header.
    ///
    /// Two halves, because they behave differently and saying so once here
    /// beats a surprise at the button. The WoW family Lightforge downloads;
    /// everything else is Battle.net's to install and start.
    fn browse(&self) -> Element<'_, Message> {
        use lightforge_core::product::CatalogEntry;

        let tracked = |code: &ProductCode| self.choices.iter().any(|c| &c.code == code);
        let mut rows = column![text("Add a game").size(theme::HEADING)]
            .spacing(theme::SM)
            .width(Length::Fixed(400.0));

        let row_for = |entry: &CatalogEntry| -> Element<Message> {
            let mut facts = column![text(entry.label.clone()).size(theme::BODY)];
            if let Some(version) = self.installed_versions.get(entry.code.as_str()) {
                facts = facts.push(
                    row![muted("installed".to_owned(), theme::CAPTION), mono(version.clone())]
                        .spacing(theme::XS)
                        .align_y(Alignment::Center),
                );
            }
            let action: Element<Message> = match tracked(&entry.code) {
                true => secondary("Remove", Message::RemoveProduct(entry.code.clone()))
                    .enabled(self.choices.len() > 1)
                    .into(),
                false => secondary("Add", Message::AddProduct(entry.code.clone()))
                    .enabled(true)
                    .into(),
            };
            container(
                row![facts.width(Length::Fill), action]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
            )
            .padding(theme::SM)
            .style(theme::row)
            .into()
        };

        // Only line heads: a line's other branches are reached from the
        // version picker, not added here as games of their own.
        // Tracked lines stay listed, or there would be no way to remove one.
        let managed: Vec<&CatalogEntry> =
            self.catalog.managed().filter(|e| e.branch == Branch::Live).collect();
        if !managed.is_empty() {
            rows = rows.push(muted("World of Warcraft".to_owned(), theme::LABEL));
            rows = rows.push_maybe(self.hint("Downloaded and launched by Lightforge."));
            for entry in managed {
                rows = rows.push(row_for(entry));
            }
        }

        rows = rows.push(muted("Other Blizzard games".to_owned(), theme::LABEL));
        rows = rows.push_maybe(self.hint("Installed and started through Battle.net. Lightforge only opens them."));
        for entry in self.catalog.hand_offs() {
            rows = rows.push(row_for(entry));
        }

        container(scrollable(rows))
            .padding(theme::MD)
            .max_height(560.0)
            .style(theme::floating)
            .into()
    }

    /// The download manager: every transfer this session knows about.
    ///
    /// One row each, with the controls that row can actually offer — a queued
    /// transfer has no progress to show and a running one cannot be resumed.
    fn manager(&self) -> Element<'_, Message> {
        let mut rows = column![
            row![
                text("Downloads").size(theme::HEADING),
                Space::with_width(Length::Fill),
                muted(
                    match self.queue.len() {
                        1 => "1 in queue".to_owned(),
                        n => format!("{n} in queue"),
                    },
                    theme::CAPTION,
                ),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(380.0));

        for entry in &self.queue {
            let mut item = column![text(entry.label.clone()).size(theme::BODY)].spacing(theme::XS);

            match &entry.stage {
                Stage::Queued => {
                    item = item.push(muted("waiting".to_owned(), theme::CAPTION));
                }
                Stage::Running { .. } => {
                    let progress = entry.progress();
                    item = item.push(progress_bar(0.0..=1.0, progress.fraction()).height(5));
                    item = item.push(
                        row![
                            mono(bytes_of(progress.downloaded_bytes, progress.total_bytes)),
                            dot(),
                            mono(rate_of(progress.bytes_per_second)),
                            dot(),
                            mono(eta_of(progress)),
                        ]
                        .spacing(theme::XS)
                        .align_y(Alignment::Center),
                    );
                }
                Stage::Paused(progress) => {
                    item = item.push(progress_bar(0.0..=1.0, progress.fraction()).height(5));
                    item = item.push(muted("paused".to_owned(), theme::CAPTION));
                }
            }

            let control: Element<Message> = match &entry.stage {
                Stage::Running { .. } => {
                    secondary("Pause", Message::PauseDownload(entry.code.clone()))
                        .enabled(true)
                        .into()
                }
                Stage::Queued | Stage::Paused(_) => {
                    secondary("Resume", Message::ResumeDownload(entry.code.clone()))
                        .enabled(true)
                        .into()
                }
            };

            rows = rows.push(
                container(
                    row![
                        item.width(Length::Fill),
                        column![
                            control,
                            secondary("Remove", Message::CancelDownload(entry.code.clone()))
                                .enabled(true),
                        ]
                        .spacing(theme::XS),
                    ]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
                )
                .padding(theme::SM)
                .style(theme::row),
            );
        }

        container(rows)
            .padding(theme::MD)
            .style(theme::floating)
            .into()
    }

    /// Explanatory prose under a control, drawn only when the hints are on.
    ///
    /// `None` rather than an empty widget so a column simply does not lay the
    /// line out: an invisible child still costs its spacing, which would leave
    /// the controls unevenly spread once the prose is gone.
    fn hint<'a>(&self, content: impl Into<String>) -> Option<iced::widget::Text<'a>> {
        self.hints.then(|| muted(content.into(), theme::CAPTION))
    }

    /// What to call a line on screen.
    ///
    /// The name the user gave it first, then the catalog's, and only then the
    /// line's own — which for a line Lightforge does not model is the bare
    /// product code, and `hs_beta` is not a thing to label a tile with.
    fn name_of(&self, line: &ProductLine) -> String {
        let head = line.head();
        self.choices
            .iter()
            .find(|c| c.code == head)
            .map(|c| c.label.clone())
            .or_else(|| self.catalog.get(&head).map(|e| e.label.clone()))
            .unwrap_or_else(|| line.label().to_owned())
    }

    /// The art a line's tile wears, if any.
    ///
    /// The line's mark is its live build's, since that is the product the art
    /// was published against; a PTR is the same game wearing the same face.
    ///
    /// Except for a hand-off: the manifest's `default` answers for every code
    /// it does not name, so asking would put Retail's mark on Hearthstone.
    fn tile_art(&self, line: &ProductLine) -> Option<&PathBuf> {
        let head = line.head();
        match self.kind_of(&head) {
            Kind::Managed => self.icons.get(&head),
            Kind::HandOff => None,
        }
    }

    /// One tile.
    ///
    /// A line with no icon in the manifest falls back to its label, so the
    /// band is complete and usable before any art exists and fills in one line
    /// at a time as art is published.
    fn tile(&self, line: &ProductLine) -> Element<'_, Message> {
        let selected = line == &self.catalog.line_of(&self.selected.code);
        let face: Element<Message> = match self.tile_art(line) {
            Some(path) => image(image::Handle::from_path(path))
                .content_fit(ContentFit::Contain)
                .width(Length::Fixed(theme::TILE))
                .height(Length::Fixed(theme::TILE))
                .into(),
            None => container(text(self.name_of(line)).size(theme::LABEL))
                .center_x(Length::Shrink)
                .center_y(Length::Fixed(theme::TILE))
                .into(),
        };

        let tile = button(face).padding([theme::XS, theme::SM]);
        let tile = match selected {
            true => tile.style(theme::selected),
            false => tile.style(theme::ghost).on_press(Message::LineSelected(line.clone())),
        };

        tooltip(
            tile,
            container(text(self.name_of(line)).size(theme::LABEL))
                .padding([theme::XS, theme::SM])
                .style(theme::floating),
            tooltip::Position::Bottom,
        )
        .into()
    }

    /// How wide a tile will be.
    ///
    /// Exact for art: a fixed square in fixed padding. A line falling back to
    /// its name cannot be measured without a renderer, so its width is
    /// estimated from the glyph count at a width no glyph of this size
    /// exceeds. Erring wide costs a little unused band; erring narrow would
    /// cut a tile in half against the navigation, so the estimate leans wide
    /// on purpose.
    fn tile_width(&self, line: &ProductLine) -> f32 {
        let padding = theme::SM as f32 * 2.0;
        match self.tile_art(line) {
            Some(_) => theme::TILE + padding,
            None => {
                let glyphs = self.name_of(line).chars().count() as f32;
                glyphs * theme::LABEL as f32 * 0.62 + padding
            }
        }
    }

    /// The game band: the tiles that fit, then Add a game, and whatever did
    /// not fit behind an overflow button.
    ///
    /// `room` is the width the header has left over once the title, the
    /// navigation and the buttons beside it have taken theirs, so the band
    /// never has to know the window's size — see [`Self::header`].
    fn strip(&self, room: f32) -> Element<'_, Message> {
        let lines = self.lines();
        let current = self.catalog.line_of(&self.selected.code);
        let selected = lines.iter().position(|line| line == &current).unwrap_or(0);
        let widths: Vec<f32> = lines.iter().map(|line| self.tile_width(line)).collect();

        // Add a game sits at the end of the band whatever happens, so its
        // width is not the band's to spend.
        let (shown, hidden) = divide_strip(&widths, selected, room - BAND_BUTTON - theme::SM as f32);

        let mut band = row![].align_y(Alignment::Center).spacing(theme::SM);
        for id in shown {
            band = band.push(self.tile(&lines[id]));
        }

        if !hidden.is_empty() {
            let hidden: Vec<ProductLine> = hidden.into_iter().map(|id| lines[id].clone()).collect();
            let more = band_button(
                image(chevron_icon())
                    .width(Length::Fixed(BAND_GLYPH))
                    .height(Length::Fixed(BAND_GLYPH)),
                Message::ToggleStrip,
                self.strip_open,
                format!("{} more game{}", hidden.len(), match hidden.len() {
                    1 => "",
                    _ => "s",
                }),
            );
            // The panel hangs under the button it belongs to rather than in
            // the corner like the download manager: which games are behind it
            // is the button's own answer, and the corner is a long way from
            // the band on a wide window.
            band = band.push(popover::popover(
                more,
                self.strip_open.then(|| self.overflow_menu(&hidden)),
            ));
        }

        band = band.push(Space::with_width(Length::Fixed(theme::SM as f32)));
        band = band.push(band_button(
            image(plus_icon()).width(Length::Fixed(BAND_GLYPH)).height(Length::Fixed(BAND_GLYPH)),
            Message::ToggleBrowse,
            self.browse_open,
            "Add a game".to_owned(),
        ));
        band.into()
    }

    /// The games the band had no room for.
    fn overflow_menu(&self, hidden: &[ProductLine]) -> Element<'_, Message> {
        let mut rows = column![].spacing(theme::XS).width(Length::Fixed(220.0));
        for line in hidden {
            let mut entry = row![].spacing(theme::SM).align_y(Alignment::Center);
            if let Some(path) = self.tile_art(line) {
                entry = entry.push(
                    image(image::Handle::from_path(path))
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(theme::TILE * 0.6))
                        .height(Length::Fixed(theme::TILE * 0.6)),
                );
            }
            entry = entry.push(text(self.name_of(line)).size(theme::BODY));
            rows = rows.push(
                button(entry.width(Length::Fill))
                    .padding([theme::XS, theme::SM])
                    .style(theme::ghost)
                    .on_press(Message::LineSelected(line.clone())),
            );
        }
        container(rows).padding(theme::SM).style(theme::floating).into()
    }

    /// Every branch of the selected line, configured or not.
    ///
    /// `None` when the line has exactly one branch to offer — a picker with a
    /// single entry is furniture.
    fn branch_choices(&self) -> Option<(Vec<BranchChoice>, BranchChoice)> {
        let line = self.catalog.line_of(&self.selected.code);
        let choices: Vec<BranchChoice> = self
            .catalog
            .branches(&line)
            .into_iter()
            .map(|entry| BranchChoice {
                configured: self.choices.iter().any(|c| c.code == entry.code),
                branch: entry.branch,
                code: entry.code.clone(),
                label: entry.label.clone(),
            })
            .collect();

        if choices.len() < 2 {
            return None;
        }
        let current = choices.iter().find(|c| c.code == self.selected.code)?.clone();
        Some((choices, current))
    }

    /// Switch to `line`, keeping whichever of its branches was last in use.
    ///
    /// Falls back to the line's live build, then to whatever of it *is*
    /// configured: selecting a tile must always land somewhere.
    fn select_line(&mut self, line: &ProductLine) -> Task<Message> {
        let remembered = self.last_branch.get(line).cloned();
        let configured = |code: &ProductCode| self.choices.iter().find(|c| &c.code == code).cloned();

        let choice = remembered
            .as_ref()
            .and_then(configured)
            .or_else(|| configured(&line.head()))
            .or_else(|| {
                self.choices
                    .iter()
                    .find(|c| &self.catalog.line_of(&c.code) == line)
                    .cloned()
            });

        match choice {
            Some(choice) => self.select(choice),
            None => Task::none(),
        }
    }

    /// Switch to one product, remembering it as its line's current branch.
    fn select(&mut self, choice: Choice) -> Task<Message> {
        if choice.code == self.selected.code {
            return Task::none();
        }
        self.last_branch.insert(self.catalog.line_of(&choice.code), choice.code.clone());
        self.selected = choice.clone();
        self.local = Local::default();
        self.remote = None;
        self.branding = None;
        self.status = "checking…".to_owned();
        // These are per-product, so they follow the selection rather than
        // carrying Retail's frame cap over to Classic's panel.
        self.load_tweak_inputs();
        self.reload(choice.code)
    }

    /// A screen that is not the dashboard: header, title, content.
    fn page<'a>(
        &'a self,
        title: &'a str,
        subtitle: Option<String>,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let mut heading = column![text(title).size(theme::TITLE)].spacing(theme::XS);
        if let Some(subtitle) = subtitle {
            heading = heading.push(muted(subtitle, theme::LABEL));
        }

        column![
            self.header(),
            column![heading, scrollable(content).height(Length::Fill)]
                .spacing(theme::LG)
                .padding(theme::LG),
        ]
        .into()
    }

    fn dashboard(&self) -> Element<'_, Message> {
        let action = self.action();
        let kind = self.kind_of(&self.selected.code);

        // A hand-off product wears no art. The manifest's `default` catches
        // every code it does not name, so theming Hearthstone would dress it
        // in Retail's logo and key art — worse than no art at all.
        let branding = match kind {
            Kind::Managed => self.branding.as_ref(),
            Kind::HandOff => None,
        };

        // The wordmark hangs under the app title in the top-left rather than
        // floating mid-page: the centre of the dashboard belongs to the art,
        // and a product logo reads as branding when it sits with Lightforge's
        // own rather than competing with the background for the middle.
        let logo: Element<Message> = match branding {
            Some(b) => image(b.logo.clone())
                .content_fit(ContentFit::Contain)
                .height(Length::Fixed(theme::logo_height(b.branding.logo.height)))
                .into(),
            None => column![
                text(self.selected.label.clone()).size(theme::DISPLAY),
                muted("no artwork configured".to_owned(), theme::LABEL),
            ]
            .spacing(theme::XS)
            .into(),
        };

        let content = column![
            self.header(),
            column![
                // Extra room above: at the column's own spacing the wordmark
                // crowds the band and the pair reads as one jammed corner
                // rather than as the app's name with a product under it.
                container(logo)
                    .align_x(Alignment::Start)
                    .width(Length::Fill)
                    .padding([theme::MD, 0]),
                Space::with_height(Length::Fill),
                // A product Battle.net owns gets a panel that offers only what
                // Lightforge can actually do for it. Running the normal one
                // would put an INSTALL button on a game this launcher has no
                // way to install — and, worse, one that would plan a CASC
                // download into the WoW directory.
                match kind {
                    Kind::Managed => self.action_panel(&action),
                    Kind::HandOff => self.handoff_panel(),
                },
            ]
            .spacing(theme::MD)
            .padding(theme::LG),
        ];

        match branding {
            Some(b) => stack![
                image(b.background.clone())
                    .content_fit(ContentFit::Cover)
                    .width(Length::Fill)
                    .height(Length::Fill),
                // A scrim: art is chosen to look good, not to be legible behind
                // text, so the page darkens it towards the bottom where the
                // controls live.
                container(Space::with_height(Length::Fill))
                    .width(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                            iced::gradient::Linear::new(0.0)
                                .add_stop(0.0, iced::Color { a: 0.95, ..theme::BG })
                                .add_stop(0.45, iced::Color { a: 0.35, ..theme::BG })
                                .add_stop(1.0, iced::Color { a: 0.15, ..theme::BG }),
                        ))),
                        ..container::Style::default()
                    }),
                content,
            ]
            .into(),
            None => content.into(),
        }
    }

    /// Status, progress, and every action for the selected product.
    fn action_panel(&self, action: &ActionState) -> Element<'_, Message> {
        let color = match action {
            ActionState::Install => theme::INSTALL,
            ActionState::Update { .. } => theme::UPDATE,
            ActionState::Play => theme::PLAY,
            ActionState::Downloading(_) | ActionState::Paused(_) | ActionState::Queued => {
                theme::PAUSE
            }
            ActionState::Error { .. } => theme::ERROR,
            ActionState::Unknown => theme::PAUSE,
        };

        let mut left = column![].spacing(theme::SM).width(Length::Fill);

        // The branch belongs here rather than in the header: it is not
        // navigation, it decides what every line below it refers to — which
        // version is "latest", which install "installed", which files "Open
        // Files" opens. Only shown where there is a choice to make.
        if let Some((branches, current)) = self.branch_choices() {
            left = left.push(
                row![
                    muted("version".to_owned(), theme::CAPTION),
                    pick_list(branches, Some(current), Message::BranchSelected)
                        .text_size(theme::LABEL)
                        .padding([theme::XS, theme::SM])
                        .style(theme::picker)
                        .menu_style(theme::picker_menu),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }

        left = left.push(match &self.error {
            Some(e) => text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR),
            None => text(self.status.clone()).size(theme::LABEL).color(theme::MUTED),
        });

        left = self.scan_progress(left);

        if let Some(progress) = progress_of(action) {
            left = left.push(progress_bar(0.0..=1.0, progress.fraction()).height(6));
            left = left.push(
                row![
                    mono(bytes_of(progress.downloaded_bytes, progress.total_bytes)),
                    dot(),
                    mono(rate_of(progress.bytes_per_second)),
                    dot(),
                    mono(eta_of(progress)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
            let stage = self
                .download_for(&self.selected.code)
                .map(|d| d.sample.stage.clone())
                .unwrap_or_default();
            if !stage.is_empty() {
                left = left.push(muted(stage, theme::CAPTION));
            }
        } else if let Some(version) = &self.local.version {
            left = left.push(
                row![muted("installed".to_owned(), theme::CAPTION), mono(version.clone())]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
            );
        }

        // What an Install or Update would actually fetch beyond the base game.
        // The size surprise this prevents is the whole reason the gate exists:
        // on the Forever beta the texture pack alone is 37 GiB.
        let opted = self.opted_optional(&self.selected.code);
        if !opted.is_empty() && matches!(action, ActionState::Install | ActionState::Update { .. })
        {
            let names: Vec<String> = opted.iter().map(|t| optional_label(t)).collect();
            left = left.push(muted(
                format!("includes optional content: {}", names.join(", ")),
                theme::CAPTION,
            ));
        }

        left = left.push(
            row![
                // Addons are already per-product — the screen titles itself
                // with the selection and every task takes its code — so this
                // is navigation, not a second copy of it. Disabled until
                // there is an install to put an addon into.
                button(text("Manage Addons").size(theme::LABEL))
                    .on_press_maybe(
                        self.local.version.is_some().then_some(Message::Show(Screen::Addons))
                    )
                    .style(theme::ghost)
                    .padding([theme::XS, theme::SM]),
                button(text("Game Optimizations").size(theme::LABEL))
                    .on_press(Message::Show(Screen::Tweaks))
                    .style(theme::ghost)
                    .padding([theme::XS, theme::SM]),
                button(text("Manage Subscription").size(theme::LABEL))
                    .on_press(Message::ManageSubscription)
                    .style(theme::ghost)
                    .padding([theme::XS, theme::SM]),
            ]
            .spacing(theme::XS),
        );

        let primary = button(
            container(text(action.label()).size(theme::HEADING)).center_x(Length::Fill),
        )
        .style(theme::filled_left(color))
        .padding([theme::MD, theme::LG])
        // 190 + the tab's 50 is 240: the width of the Battle.net button
        // underneath, so the two stack flush rather than nearly flush.
        .width(Length::Fixed(190.0))
        .height(Length::Fixed(theme::CONTROL_HEIGHT));
        let battle_net = button(
            container(text("Launch via Battle.net").size(theme::LABEL)).center_x(Length::Fill),
        )
        .style(theme::outlined)
        .padding([theme::SM, theme::MD])
        .width(Length::Fixed(240.0));

        // A split button, welded to the primary action rather than floating
        // beside it: the cog is a second way into the same thing, which is how
        // Battle.net builds it. Zero spacing and complementary corner radii
        // are what make the pair read as one control.
        //
        // The icon is an image, not a glyph — the UI font has no U+2699 and
        // renders it as a tofu box.
        let gear = button(
            image(gear_icon())
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(18.0)),
        )
        .style(theme::filled_right(color))
        .padding([theme::MD, theme::MD])
        // The same fixed height as the action half, never `Fill`: inside a
        // column, `Fill` resolves against the *column*, and the tab grows into
        // a green stripe the height of the whole stack.
        .height(Length::Fixed(theme::CONTROL_HEIGHT))
        .on_press(Message::ToggleGear);

        // Pinned to the control rather than placed near it. Everything else
        // was layout, and layout cannot put a child over its sibling: in the
        // panel's column the menu sized the card, and above the card its top
        // edge fell between the menu and the button.
        let control = popover::popover(
            row![
                match self.busy || matches!(action, ActionState::Unknown) {
                    true => primary,
                    false => primary.on_press(Message::Primary),
                },
                gear,
            ]
            .align_y(Alignment::Center),
            self.gear_open.then(|| self.gear_menu()),
        );

        let right = column![
            control,
            match self.busy {
                true => battle_net,
                false => battle_net.on_press(Message::BattleNet),
            },
            // Not a hint: it is the reason to press the button above, and a
            // user who does not know the shop needs a Battle.net session is
            // exactly who it is written for.
            //
            // Held to the buttons' width so it wraps instead of setting the
            // column's. Left to itself this line is the widest thing here, so
            // the buttons end up centred inside a column wider than they are
            // — and the cog menu, aligned to the card, no longer sits over
            // the cog.
            muted(
                "Signs in through Battle.net so the in-game shop works.".to_owned(),
                theme::CAPTION,
            )
            .width(Length::Fixed(240.0)),
        ]
        .spacing(theme::SM)
        .align_x(Alignment::Center)
        .width(Length::Shrink);

        container(row![left, right].spacing(theme::XL).align_y(Alignment::Center))
            .padding(theme::LG)
            .style(theme::floating)
            .into()
    }

    /// Per-product performance settings: what this machine can do for the
    /// game, and what the client should be told to do for itself.
    fn tweaks_screen(&self) -> Element<'_, Message> {
        let tweaks = self.tweaks();

        let check = |label: &'static str, which: Tweak, on: bool| {
            iced::widget::checkbox(label, on)
                .on_toggle(move |on| Message::Tweak(which, on))
                .text_size(theme::BODY)
        };
        let heading = |title: &'static str| text(title).size(theme::HEADING);

        // Host tools. Detected, never assumed: the toggles that are missing
        // are missing because the binary is, and saying so beats a switch that
        // silently does nothing.
        let mut host = column![heading("This machine")].spacing(theme::SM);
        match self.tools.game_performance.is_some() {
            true => {
                host = host.push(check(
                    "Hold the CPU on its performance profile while playing",
                    Tweak::GamePerformance,
                    tweaks.wants_game_performance(),
                ));
                host = host.push_maybe(self.hint("Launches through CachyOS' game-performance, which also keeps the session \
                     from idling. On by default where it exists; it stands aside on hardware \
                     with no performance profile."));
            }
            false => {
                host = host.push(muted(
                    "game-performance is not installed, so the CPU power profile is left \
                     to the system."
                        .to_owned(),
                    theme::CAPTION,
                ));
            }
        }
        if self.tools.gamemode.is_some() {
            host = host.push(check("Launch through GameMode", Tweak::GameMode, tweaks.gamemode));
        }
        if self.tools.mangohud.is_some() {
            host = host.push(check("Show the MangoHud overlay", Tweak::MangoHud, tweaks.mangohud));
        }
        let host = container(host).padding(theme::MD).style(theme::card);

        // Graphics. The API is a genuine fork rather than a best value, so it
        // is a picker with the trade-off written next to it.
        let apis = vec![
            ApiChoice(None),
            ApiChoice(Some(GraphicsApi::D3d11)),
            ApiChoice(Some(GraphicsApi::D3d12)),
        ];
        let graphics = container(
            column![
                heading("Graphics"),
                row![
                    muted("Direct3D version".to_owned(), theme::LABEL)
                        .width(Length::Fixed(150.0)),
                    pick_list(apis, Some(ApiChoice(tweaks.graphics_api)), Message::TweakApi)
                        .text_size(theme::BODY)
                        .padding(theme::SM)
                        .style(theme::picker)
                        .menu_style(theme::picker_menu),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            ]
            .push_maybe(self.hint(
                "Which translation layer the game runs on: D3D11 goes through DXVK, D3D12 \
                 through VKD3D. Which is faster depends on the GPU — D3D12 is usually a \
                 small win on AMD and often a loss on NVIDIA, so this is worth measuring \
                 rather than assuming.",
            ))
            .push(
                row![
                    muted("Frame cap".to_owned(), theme::LABEL).width(Length::Fixed(150.0)),
                    text_input("uncapped", &self.fps_input)
                        .on_input(Message::TweakFpsCap)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                    muted("FPS".to_owned(), theme::LABEL),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
            .push_maybe(self.hint(
                "The client's own limit rather than the driver's, so it keeps working if \
                 the Direct3D version above changes.",
            ))
            .push(
                row![secondary("Clear shader cache", Message::ClearShaderCache).enabled(true)]
                    .push_maybe(self.hint(
                        "Shaders this product compiled under an older Proton build. Clearing \
                         them costs one session of stutter and affects no other product.",
                    ))
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
            )
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        // Client settings. Everything here is a line in Config.wtf.
        let client = container(
            column![
                heading("Client settings"),
                check(
                    "Stop profiling addon execution",
                    Tweak::SkipAddonProfiling,
                    tweaks.skip_addon_profiling,
                ),
                check(
                    "Drop water and weather detail in raids and battlegrounds",
                    Tweak::ReduceRaidEffects,
                    tweaks.reduce_raid_effects,
                ),
                check(
                    "Skip non-critical world preloading",
                    Tweak::FasterZoneLoads,
                    tweaks.faster_zone_loads,
                ),
                check("Raw mouse input", Tweak::RawMouse, tweaks.raw_mouse),
            ]
            .push_maybe(self.hint(
                "Also the fix for the cursor jumping back to the middle of the window \
                 after a click, which is a Wine problem rather than a game one.",
            ))
            .push(check(
                "Turn off the full-screen glow and death filter",
                Tweak::NoScreenEffects,
                tweaks.no_screen_effects,
            ))
            .push(
                row![
                    muted("Spell queue".to_owned(), theme::LABEL).width(Length::Fixed(150.0)),
                    text_input("client default", &self.spell_input)
                        .on_input(Message::TweakSpellQueue)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                    muted("ms".to_owned(), theme::LABEL),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
            .push_maybe(self.hint(
                "How early the next ability may be queued. Your latency plus about 50ms \
                 is the usual advice.",
            ))
            .push_maybe(self.hint(
                "These are written to this product's Config.wtf and re-applied before \
                 every launch, because the client rewrites that file from memory when it \
                 exits. Change one of them in-game and the setting here wins next time.",
            ))
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        // Deliberately its own card with its own warning: this one buys
        // scenery with frames, which is the opposite of everything above.
        let visuals = container(
            column![
                heading("Visuals"),
                check(
                    "Maximum view distance and ground clutter",
                    Tweak::MaxDrawDistance,
                    tweaks.max_draw_distance,
                ),
            ]
            .push_maybe(self.hint(
                "This one costs frames rather than winning them. It is here because \
                 people want it, not because it is an optimization.",
            ))
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        // The escape hatch. Everything above is a curated toggle; this is for
        // the variable nobody anticipated.
        let mut env = column![heading("Environment")].spacing(theme::SM);
        for (key, value) in &tweaks.env {
            env = env.push(
                container(
                    row![
                        mono(format!("{key}={value}")).width(Length::Fill),
                        button(text("Remove").size(theme::LABEL))
                            .on_press(Message::TweakEnvRemove(key.clone()))
                            .style(theme::ghost)
                            .padding([theme::XS, theme::SM]),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )
                .padding([theme::XS, theme::SM])
                .style(theme::row),
            );
        }
        env = env.push(
            row![
                text_input("VARIABLE", &self.env_key_input)
                    .on_input(Message::TweakEnvKey)
                    .on_submit(Message::TweakEnvAdd)
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM)
                    .width(Length::Fixed(220.0)),
                text_input("value", &self.env_value_input)
                    .on_input(Message::TweakEnvValue)
                    .on_submit(Message::TweakEnvAdd)
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM)
                    .width(Length::Fill),
                secondary("Add", Message::TweakEnvAdd).enabled(!self.env_key_input.is_empty()),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        );
        env = env.push_maybe(self.hint("Set for this product's launch only, after everything Lightforge assembles — so \
             a variable here overrides the launcher's own."));
        let env = container(env).padding(theme::MD).style(theme::card);

        // Optional downloads. Off by default, which is what Battle.net does:
        // these are opt-in extras, and treating them as wanted is what put a
        // 37 GiB texture pack in a plan nobody asked for.
        let profile = self.config.product(&self.selected.code).map(|p| &p.profile);
        let known = profile.is_some_and(|p| !p.optional_available.is_empty());
        let available =
            offered_only(profile.map(|p| p.optional_available.as_slice()).unwrap_or(&[]));
        let opted = self.opted_optional(&self.selected.code);

        let mut content = column![heading("Optional downloads")].spacing(theme::SM);
        match (known, available.is_empty()) {
            (false, _) => {
                content = content.push_maybe(self.hint("What a build offers is only known once its manifest has been read. \
                     Use Check for Updates, under the cog beside PLAY, to fill this in."));
            }
            (true, true) => {
                content = content.push(muted(
                    "This build offers no optional downloads.".to_owned(),
                    theme::CAPTION,
                ));
            }
            (true, false) => {
                for name in &available {
                    let on = opted.iter().any(|n| n == name);
                    let tag = name.clone();
                    content = content.push(
                        iced::widget::checkbox(optional_label(name), on)
                            .on_toggle(move |on| Message::OptionalContent(tag.clone(), on))
                            .text_size(theme::BODY),
                    );
                }
                content = content.push_maybe(self.hint("Extra content this build offers, off unless asked for — the same \
                     default Battle.net uses. Turning one on adds it to the next download."));
            }
        }
        let content = container(content).padding(theme::MD).style(theme::card);

        let mut body = column![host, graphics, client, content, visuals, env].spacing(theme::MD);
        if let Some(e) = &self.error {
            body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }

        self.page(
            "Game Optimizations",
            Some(format!("{} · applied when this product launches", self.selected.label)),
            body.into(),
        )
    }

    /// The cog menu: what you do *about* a game rather than to play it.
    ///
    /// Laid out above the action panel rather than stacked over the page,
    /// because iced 0.13 has no popover and a floating layer would need the
    /// panel's height as a magic number to sit clear of it. Taking its room
    /// from the spacer above the panel puts it in the right place by
    /// construction, at any window size.
    fn gear_menu(&self) -> Element<'_, Message> {
        let item = |label: &'static str, message: Message, enabled: bool| {
            button(text(label).size(theme::LABEL))
                .style(theme::ghost)
                .padding([theme::SM, theme::MD])
                .width(Length::Fill)
                .on_press_maybe(enabled.then_some(message))
        };

        let installed = self.local.version.is_some();
        let can_scan = installed && !self.busy && !self.scanning;
        // Not while its own download is writing, nor while a scan reads.
        let downloading = self
            .queue
            .iter()
            .any(|d| d.code == self.selected.code && matches!(d.stage, Stage::Running { .. }));
        let can_uninstall = installed
            && !self.busy
            && !self.scanning
            && self.deep_cancel.is_none()
            && !downloading;
        let menu = container(
            column![
                item("Check for Updates", Message::CheckForUpdates, !self.busy),
                item("Scan and Repair", Message::ScanAndRepair, can_scan),
                match self.deep_cancel.is_some() {
                    true => item("Stop Deep Scan", Message::DeepScan, true),
                    false => item("Deep Scan", Message::DeepScan, can_scan),
                },
                item("Open Files in Explorer", Message::OpenFiles, installed),
                item("Copy Debug Info", Message::CopyDebug, true),
                // Last and apart from the rest: the one entry that destroys.
                item("Uninstall", Message::AskUninstall, can_uninstall),
            ]
            .push_maybe(
                self.owed_repair
                    .is_some()
                    .then(|| item("Repair Damaged Files", Message::RepairDamaged, !self.busy)),
            )
            .spacing(theme::XS),
        )
        .padding(theme::XS)
        // The width of the split button beneath it, so the menu reads as
        // belonging to that control rather than floating near it.
        .width(Length::Fixed(240.0))
        .style(theme::floating);

        mouse_area(menu).on_press(Message::MenuClick).into()
    }

    /// Addon management: what is installed, and what could be.
    fn addons_screen(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, which: AddonTab| {
            let entry = button(text(label).size(theme::LABEL)).padding([theme::SM, theme::MD]);
            match self.addon_tab == which {
                true => entry.style(theme::selected),
                false => entry.style(theme::ghost).on_press(Message::AddonTab(which)),
            }
        };

        let mut body = column![row![
            tab("My Addons", AddonTab::Mine),
            tab("Browse", AddonTab::Browse),
        ]
        .spacing(theme::XS)]
        .spacing(theme::MD);

        if self.addons_busy {
            let progress = &self.addon_sample;
            body = body.push(
                row![
                    text(progress.line.clone()).size(theme::LABEL),
                    mono(format!("{}/{}", progress.done, progress.total)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }
        if let Some(e) = &self.error {
            body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }

        body = body.push(match self.addon_tab {
            AddonTab::Mine => self.addons_mine(),
            AddonTab::Browse => self.addons_browse(),
        });

        self.page(
            "Addons",
            Some(format!(
                "{} · {} installed, {} managed by Lightforge{}",
                self.selected.label,
                self.addons.rows.len(),
                self.addons.managed,
                match self.addons.stale {
                    0 => String::new(),
                    n => format!(" · {n} recorded but missing from disk"),
                }
            )),
            body.into(),
        )
    }

    fn addons_mine(&self) -> Element<'_, Message> {
        let outdated = self.updates.iter().filter(|u| u.has_update()).count();

        let toolbar = container(
            column![
                row![
                    text_input("Paste a WoWUp import token…", &self.token_input)
                        .on_input(Message::TokenChanged)
                        .on_submit(Message::ImportToken)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM),
                    action_button("Import", theme::ACCENT, Message::ImportToken)
                        .enabled(!self.addons_busy && !self.token_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                row![
                    secondary("Check for updates", Message::CheckAddonUpdates)
                        .enabled(!self.addons_busy && self.addons.managed > 0),
                    action_button(
                        "Update all",
                        match outdated {
                            0 => theme::PAUSE,
                            _ => theme::UPDATE,
                        },
                        Message::UpdateAddons,
                    )
                    .enabled(!self.addons_busy && self.addons.managed > 0),
                    muted(
                        match (self.updates.is_empty(), outdated) {
                            (true, _) => String::new(),
                            (false, 0) => "everything is up to date".to_owned(),
                            (false, n) => format!("{n} have updates"),
                        },
                        theme::LABEL,
                    ),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ]
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![toolbar].spacing(theme::SM);

        if let Some(report) = &self.addon_report {
            for item in &report.unavailable {
                let mut line = row![column![
                    text(item.name.clone()).size(theme::BODY),
                    muted(item.reason.clone(), theme::CAPTION),
                ]
                .spacing(theme::XS)]
                .spacing(theme::SM)
                .align_y(Alignment::Center);

                line = line.push(Space::with_width(Length::Fill));
                if let Some(url) = &item.website_url {
                    line = line.push(
                        secondary("Open page", Message::OpenUrl(url.clone())).enabled(true),
                    );
                }
                body = body.push(container(line).padding(theme::MD).style(theme::row));
            }
        }

        if self.addons.rows.is_empty() {
            body = body.push(empty(
                "Nothing installed for this product yet.",
                "Search under Browse, or paste an import token above.",
            ));
        }

        for addon in &self.addons.rows {
            // A package's other folders belong to the addon above them; showing
            // BigWigs' eleven folders as eleven addons would be a lie.
            if addon.child_of.is_some() {
                continue;
            }
            let extra = self
                .addons
                .rows
                .iter()
                .filter(|r| r.child_of.as_deref() == Some(addon.folder.as_str()))
                .count();
            let status = addon.managed.as_ref().and_then(|m| {
                self.updates
                    .iter()
                    .find(|u| u.id == m.id && u.provider == m.provider)
            });

            let mut heading = row![text(addon.title.clone()).size(theme::BODY), mono(addon.version.clone())]
                .spacing(theme::SM)
                .align_y(Alignment::Center);
            if status.is_some_and(|s| s.has_update()) {
                heading = heading.push(pill(
                    format!("→ {}", status.and_then(|s| s.available.clone()).unwrap_or_default()),
                    theme::UPDATE,
                ));
            }

            let mut line = row![column![
                heading,
                muted(
                    match (&addon.managed, extra) {
                        (Some(m), 0) => format!("{} · managed", m.provider),
                        (Some(m), n) => format!("{} · managed · +{n} folders", m.provider),
                        (None, _) => "installed by hand".to_owned(),
                    },
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            line = line.push(Space::with_width(Length::Fill));
            if let Some(managed) = &addon.managed {
                line = line.push(
                    secondary(
                        "Remove",
                        Message::RemoveAddon(managed.provider.clone(), managed.id.clone()),
                    )
                    .enabled(!self.addons_busy),
                );
            }
            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }

        body = body.push(mono(self.addons.dir.display().to_string()));
        body.into()
    }

    fn addons_browse(&self) -> Element<'_, Message> {
        let search = container(
            column![
                row![
                    text_input("Search addons…", &self.search_input)
                        .on_input(Message::SearchChanged)
                        .on_submit(Message::RunSearch)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM),
                    action_button("Search", theme::ACCENT, Message::RunSearch)
                        .enabled(!self.searching && !self.search_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ]
            .push_maybe(self.hint(
                "WoWInterface, Tukui and GitHub (type owner/repo). CurseForge needs a \
                 catalog in Settings.",
            ))
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![search].spacing(theme::SM);

        // A provider that could not answer is said out loud: an empty list
        // otherwise reads as "nothing matched".
        for problem in &self.search.problems {
            body = body.push(text(format!("⚠ {problem}")).size(theme::CAPTION).color(theme::UPDATE));
        }

        if self.searching {
            body = body.push(muted("searching…".to_owned(), theme::BODY));
        } else if self.search.listings.is_empty() {
            body = body.push(match self.search.query.is_empty() {
                true => empty(
                    "Search for an addon.",
                    "Results come from WoWInterface and Tukui; type owner/repo for GitHub.",
                ),
                false => empty(
                    &format!("Nothing found for “{}”.", self.search.query),
                    "It may be a CurseForge addon, which needs a catalog configured.",
                ),
            });
        }

        let installed_ids: Vec<(&str, &str)> = self
            .addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .map(|m| (m.provider.as_str(), m.id.as_str()))
            .collect();

        for listing in &self.search.listings {
            let provider = listing.provider.as_str().to_owned();
            let already = installed_ids
                .iter()
                .any(|(p, id)| *p == provider && *id == listing.id);

            let mut line = row![column![
                row![
                    text(listing.name.clone()).size(theme::BODY),
                    pill(provider.clone(), theme::ACCENT),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                muted(
                    format!(
                        "{}{}",
                        listing.author.as_deref().unwrap_or("unknown author"),
                        match listing.downloads {
                            0 => String::new(),
                            n => format!(" · {n} downloads"),
                        }
                    ),
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            line = line.push(Space::with_width(Length::Fill));
            if let Some(url) = &listing.website_url {
                line = line.push(secondary("Page", Message::OpenUrl(url.clone())).enabled(true));
            }
            line = line.push(match already {
                true => pill("installed".to_owned(), theme::PLAY),
                false => action_button(
                    "Install",
                    theme::INSTALL,
                    Message::InstallAddon(provider.clone(), listing.id.clone()),
                )
                .enabled(!self.addons_busy)
                .into(),
            });
            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }
        body.into()
    }

    /// First-run provisioning: what is missing, and one button to fix it.
    /// Proton, the prefix and Battle.net: what is there, and the button that
    /// provisions whatever isn't.
    fn setup_section(&self) -> Element<'_, Message> {
        let mut body = column![]
            .push_maybe(self.hint(
                "Proton, the Wine prefix and Battle.net. Each step is skipped if it is \
                 already done.",
            ))
            .spacing(theme::SM);

        let step = |done: bool, label: &'static str, detail: String| {
            let color = match done {
                true => theme::PLAY,
                false => theme::MUTED,
            };
            container(
                row![
                    container(
                        container(Space::new(Length::Fixed(8.0), Length::Fixed(8.0)))
                            .style(theme::dot(color)),
                    )
                    .padding(theme::SM)
                    .style(theme::badge(color)),
                    column![text(label).size(theme::BODY), mono(detail)].spacing(theme::XS),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(theme::MD)
            .style(theme::row)
        };

        match &self.setup {
            None => body = body.push(muted("checking what is installed…".to_owned(), theme::BODY)),
            Some(status) => {
                body = body.push(step(
                    status.proton.is_some(),
                    "Proton-GE",
                    match &status.proton {
                        Some(build) => format!(
                            "{} — {}",
                            build.version,
                            lightforge_core::setup::describe_source(build)
                        ),
                        None => "not installed".to_owned(),
                    },
                ));
                body = body.push(step(
                    status.prefix.is_ready(),
                    "Wine prefix",
                    match status.prefix.is_ready() {
                        true => status.prefix.prefix.display().to_string(),
                        false => "not built, or the game link is missing".to_owned(),
                    },
                ));
                body = body.push(step(
                    status.battle_net,
                    "Battle.net",
                    match status.battle_net {
                        true => self.layout.battle_net_dir().display().to_string(),
                        false => "not installed in the prefix".to_owned(),
                    },
                ));
            }
        }

        if self.setup_running {
            let mut log = column![].spacing(theme::XS);
            for entry in &self.setup_sample.log {
                log = log.push(muted(entry.clone(), theme::CAPTION));
            }
            log = log.push(match self.setup_sample.fraction() {
                Some(fraction) => Element::from(progress_bar(0.0..=1.0, fraction).height(6)),
                // A download with no content-length still has to look alive.
                None => Element::from(mono(format!(
                    "{:.0} MB",
                    self.setup_sample.done as f64 / 1e6
                ))),
            });
            body = body.push(container(log).padding(theme::MD).style(theme::card));
        }

        let ready = self.setup.as_ref().is_some_and(|s| s.is_ready());
        let label = match (self.setup_running, ready) {
            (true, _) => "Working…",
            (false, true) => "Re-run setup",
            (false, false) => "Set up Lightforge",
        };
        body = body.push(
            action_button(
                label,
                match ready {
                    true => theme::PAUSE,
                    false => theme::INSTALL,
                },
                Message::RunSetup,
            )
            .width(220.0)
            .enabled(!self.setup_running),
        );

        body.into()
    }

    /// Every Proton build on the machine, and how to add or remove one.
    fn proton_builds(&self) -> Element<'_, Message> {
        let mut rows = column![].spacing(theme::SM);

        let installed = self.setup.as_ref().map(|s| s.installed.as_slice()).unwrap_or_default();
        if installed.is_empty() {
            rows = rows.push(muted("none found yet".to_owned(), theme::LABEL));
        }

        for build in installed {
            let pinned_by = self.config.pinned_to(&build.version);
            let managed = build.source == lightforge_core::proton::ProtonSource::Managed;

            let mut facts = row![
                mono(build.version.clone()),
                dot(),
                muted(lightforge_core::setup::describe_source(build).to_owned(), theme::CAPTION),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            // Absent until the measuring task answers, rather than a zero
            // that would read as an empty directory.
            if let Some(size) = self.proton_sizes.get(&build.path) {
                facts = facts.push(dot());
                facts = facts.push(muted(bytes_of(*size, 0), theme::CAPTION));
            }

            // Only managed builds are ours to delete, and only when nothing
            // points at them — the reason is shown instead of a dead button,
            // since "why can't I remove this" is the whole question here.
            let action: Element<Message> = match (managed, pinned_by.is_empty()) {
                (false, _) => muted("not managed here".to_owned(), theme::CAPTION).into(),
                (true, false) => {
                    muted(format!("pinned by {}", pinned_by.join(", ")), theme::CAPTION).into()
                }
                (true, true) => secondary("Remove", Message::RemoveProton(build.version.clone()))
                    .enabled(!self.setup_running)
                    .into(),
            };

            rows = rows.push(
                row![facts, Space::with_width(Length::Fill), action]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
            );
        }

        // The picker is only useful once the release list has been fetched,
        // and saying so beats an empty dropdown.
        let install: Element<Message> = match self.proton_available.is_empty() {
            true => muted(
                "Check for updates to list installable versions.".to_owned(),
                theme::CAPTION,
            )
            .into(),
            false => row![
                pick_list(
                    self.proton_available
                        .iter()
                        .map(|v| ProtonChoice(Some(v.clone())))
                        .collect::<Vec<_>>(),
                    self.proton_choice.clone().map(|v| ProtonChoice(Some(v))),
                    Message::ChooseProtonVersion,
                )
                .placeholder("Choose a version")
                .text_size(theme::BODY)
                .padding(theme::SM),
                secondary("Install", Message::InstallProton)
                    .enabled(self.proton_choice.is_some() && !self.setup_running),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into(),
        };

        rows = rows.push(Space::with_height(Length::Fixed(theme::XS as f32)));
        rows = rows.push(install);
        rows.into()
    }

    /// Settings, laid out the way Battle.net lays out its own: a list of
    /// sections on the left and one section at a time on the right, as flat
    /// rows at one shared width rather than a stack of boxes that each size
    /// themselves.
    fn settings(&self) -> Element<'_, Message> {
        let mut nav = column![].spacing(theme::XS).width(Length::Fixed(180.0));
        for tab in SettingsTab::ALL {
            let entry = button(text(tab.label()).size(theme::BODY))
                .width(Length::Fill)
                .padding([theme::SM, theme::MD]);
            nav = nav.push(match self.settings_tab == tab {
                true => entry.style(theme::selected),
                false => entry.style(theme::ghost).on_press(Message::SettingsTab(tab)),
            });
        }

        let (title, body) = match self.settings_tab {
            SettingsTab::Setup => ("Setup", self.setup_section()),
            SettingsTab::Proton => ("Proton", self.proton_section()),
            SettingsTab::Downloads => ("Downloads", self.downloads_section()),
            SettingsTab::Storage => ("Storage", self.storage_section()),
            SettingsTab::About => ("About", self.about_section()),
        };

        // The error leads rather than trails: a failed setup run is reported
        // here, and at the foot of a scrolled section it would go unseen.
        let mut content = column![].spacing(theme::MD).max_width(SETTINGS_WIDTH);
        if let Some(e) = &self.error {
            content =
                content.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }
        content = content.push(section(title, body));

        column![
            self.header(),
            row![
                nav,
                container(Space::with_width(Length::Fixed(1.0)))
                    .height(Length::Fill)
                    .style(theme::divider),
                scrollable(container(content).padding([0, theme::LG]))
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .spacing(theme::LG)
            .padding(theme::LG),
        ]
        .into()
    }

    /// Which Proton each product runs under, and the update check.
    fn proton_section(&self) -> Element<'_, Message> {
        let installed: Vec<ProtonChoice> = std::iter::once(ProtonChoice(None))
            .chain(
                self.setup
                    .iter()
                    .flat_map(|s| s.installed.iter())
                    .map(|build| ProtonChoice(Some(build.version.clone()))),
            )
            .collect();

        let product_pin = self
            .config
            .product(&self.selected.code)
            .and_then(|p| p.profile.proton_ge_version.clone());

        let picker = |current: Option<String>, on_pick: fn(ProtonChoice) -> Message| {
            pick_list(installed.clone(), Some(ProtonChoice(current)), on_pick)
                .text_size(theme::BODY)
                .padding(theme::SM)
                .width(Length::Fixed(260.0))
                .style(theme::picker)
                .menu_style(theme::picker_menu)
                .into()
        };

        column![
            field_row(
                "Default".to_owned(),
                picker(
                    self.config.default_proton.version().map(str::to_owned),
                    Message::PinDefaultProton,
                ),
            ),
            field_row(self.selected.label.clone(), picker(product_pin, Message::PinProductProton)),
            field_row(
                "Battle.net".to_owned(),
                picker(
                    self.config.battle_net_proton.version().map(str::to_owned),
                    Message::PinBattleNetProton,
                ),
            ),
        ]
        .push_maybe(self.hint(
            "A product with no pin of its own follows the default. Battle.net and Agent \
             are a different workload from the game, so they pin separately.",
        ))
        .push(
            row![
                secondary("Check for updates", Message::CheckProton).enabled(true),
                muted(self.proton_notice.clone().unwrap_or_default(), theme::LABEL),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .push(Space::with_height(Length::Fixed(theme::MD as f32)))
        .push(section("Installed builds", self.proton_builds()))
        .spacing(theme::SM)
        .into()
    }

    /// Speed caps, ours and Battle.net's, and pausing while playing.
    fn downloads_section(&self) -> Element<'_, Message> {
        let speed = |input: &str, placeholder: &str, on_input: fn(String) -> Message| {
            row![
                text_input(placeholder, input)
                    .on_input(on_input)
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM)
                    .width(Length::Fixed(110.0)),
                muted("MB/s".to_owned(), theme::LABEL),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into()
        };

        column![field_row(
            "Speed limit".to_owned(),
            speed(&self.limit_input, "unlimited", Message::DownloadLimitChanged),
        )]
        .push_maybe(
            self.hint("Applies to game downloads. Proton builds and addons are not capped."),
        )
        .push(field_row(
            "Battle.net's limit".to_owned(),
            speed(&self.bnet_limit_input, "none", Message::BattleNetLimitChanged),
        ))
        .push_maybe(self.hint(
            "Battle.net's own cap, for what it downloads. Read from its config and written \
             back; clearing the field removes the setting and restores its default.",
        ))
        .push(
            iced::widget::checkbox(
                "Pause downloads while a game is running",
                self.config.pause_downloads_while_playing,
            )
            .on_toggle(Message::PauseWhilePlaying)
            .text_size(theme::BODY),
        )
        .push_maybe(self.hint(
            "The game you launch always pauses its own download — it and the launcher \
             would be sharing one storage file. This covers the rest of the queue.",
        ))
        .spacing(theme::SM)
        .into()
    }

    /// All the app shows when `config.toml` did not load.
    fn broken_config_screen<'a>(&'a self, error: &'a str) -> Element<'a, Message> {
        let body = column![
            text("Lightforge can't read its settings").size(theme::TITLE),
            muted(
                "Nothing has been changed. Fix the file, or move it aside to start over \
                 with defaults, then restart Lightforge. A copy was saved next to it as \
                 config.toml.broken."
                    .to_owned(),
                theme::BODY,
            ),
            container(mono(error.to_owned()))
                .padding(theme::MD)
                .width(Length::Fill)
                .style(theme::row),
            row![
                secondary("Open config folder", Message::OpenConfigFolder).enabled(true),
            ],
        ]
        .spacing(theme::MD)
        .max_width(SETTINGS_WIDTH);

        container(body).center(Length::Fill).padding(theme::XL).style(theme::scrim).into()
    }

    /// The install dialog's card.
    fn install_card<'a>(&'a self, dialog: &'a InstallDialog) -> Element<'a, Message> {
        let location: Element<Message> = match dialog.movable {
            true => column![text_input("~/Games/Blizzard/games", &self.games_input)
                .on_input(Message::GamesFolderChanged)
                .style(theme::field)
                .size(theme::BODY)
                .padding(theme::SM)]
            .push_maybe(self.hint(
                "World of Warcraft is installed in a folder of its own inside this one. \
                 Every WoW version you add later shares it.",
            ))
            .spacing(theme::XS)
            .into(),
            // Battle.net's own wording and colour for the same fact. A drawn
            // dot rather than a check mark: the UI font has no U+2713.
            false => column![
                mono(self.layout.wow_install().display().to_string()),
                tooltip(
                    row![
                        container(Space::new(Length::Fixed(8.0), Length::Fixed(8.0)))
                            .style(theme::dot(theme::PLAY)),
                        text("Sharing data").size(theme::LABEL).color(theme::PLAY),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                    container(
                        text(
                            "Every WoW version in this folder shares one game data store, \
                             so files already downloaded for another version are not \
                             fetched again."
                        )
                        .size(theme::LABEL),
                    )
                    .max_width(320)
                    .padding([theme::XS, theme::SM])
                    .style(theme::floating),
                    tooltip::Position::Bottom,
                ),
            ]
            .spacing(theme::XS)
            .into(),
        };

        let mut body = column![field_row("Location".to_owned(), location)].spacing(theme::MD);

        let mut ready = false;
        match &dialog.stage {
            DialogStage::Calculating => {
                body = body.push(muted("Calculating size…".to_owned(), theme::LABEL));
            }
            DialogStage::Failed(e) => {
                body = body.push(
                    text(format!("⚠ Could not read this build: {e}"))
                        .size(theme::LABEL)
                        .color(theme::ERROR),
                );
            }
            DialogStage::Ready(offerings) => {
                let extra = offerings
                    .optional_bytes
                    .iter()
                    .find(|(tag, _)| tag == HIGH_RES)
                    .map(|(_, bytes)| *bytes)
                    // Every build names the tag; only some have textures
                    // behind it. Retail's adds nothing, and a "+0 MiB"
                    // switch is not a choice.
                    .filter(|bytes| *bytes > 0);
                if let Some(extra) = extra {
                    body = body.push(field_row(
                        "Optional".to_owned(),
                        row![
                            iced::widget::checkbox(optional_label(HIGH_RES), dialog.high_res)
                                .on_toggle(Message::InstallHighRes)
                                .text_size(theme::BODY),
                            muted(format!("+{}", size_of(extra)), theme::LABEL),
                        ]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center)
                        .into(),
                    ));
                }

                let need = offerings.base_bytes
                    + if dialog.high_res { extra.unwrap_or(0) } else { 0 };
                // Exact: priced against the store, the same test the plan
                // applies, so shared files are already left out. Download
                // size, not disk size — the extracted install files (the
                // executables and the like) come on top, which is most of
                // why Battle.net's "required" reads a little higher.
                body = body.push(field_row(
                    "Download".to_owned(),
                    mono(format!("{} to download", size_of(need))).into(),
                ));
                match dialog.disk {
                    Some(disk) => {
                        body = body.push(field_row(
                            "Free space".to_owned(),
                            mono(size_of(disk.free)).into(),
                        ));
                        ready = disk.free > need;
                        if !ready {
                            body = body.push(
                                text("⚠ Not enough free space on that disk.")
                                    .size(theme::LABEL)
                                    .color(theme::ERROR),
                            );
                        }
                    }
                    None => ready = true,
                }
            }
        }

        // A pair, so one size: the same height and width, labels centred,
        // with only the fill telling the default from the way out.
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding([theme::SM, theme::MD])
        };
        let actions = row![
            Space::with_width(Length::Fill),
            choice("Cancel").style(theme::outlined).on_press(Message::InstallCancel),
            choice("Start install")
                .style(theme::filled(theme::INSTALL))
                .on_press_maybe(ready.then_some(Message::InstallStart)),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![
                section(format!("Install {}", dialog.label), body.into()),
                actions,
            ]
            .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The uninstall confirmation's card.
    fn uninstall_card<'a>(&'a self, dialog: &'a UninstallDialog) -> Element<'a, Message> {
        let mut body = column![].spacing(theme::MD);
        let mut ready = false;
        match &dialog.plan {
            None => {
                body = body.push(muted("Working out what to remove…".to_owned(), theme::LABEL));
            }
            Some(Err(e)) => {
                body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
            }
            Some(Ok(preview)) if preview.plan.is_empty() => {
                body = body.push(muted(
                    "Nothing to remove — it is not installed here.".to_owned(),
                    theme::LABEL,
                ));
            }
            Some(Ok(preview)) => {
                ready = true;
                let plan = &preview.plan;
                let reclaim = preview.reclaim.as_ref().and_then(|r| r.as_ref().ok());
                let deletes = column(
                    plan.flavor_dir
                        .iter()
                        .chain(plan.store.iter())
                        .map(|path| mono(path.display().to_string()).into()),
                )
                .spacing(theme::XS);
                body = body
                    .push(field_row("Deletes".to_owned(), deletes.into()))
                    .push(field_row(
                        "Frees".to_owned(),
                        mono(size_of(plan.bytes + reclaim.map_or(0, |r| r.freed))).into(),
                    ));

                let (note, color) = match plan.is_whole_install() {
                    true => (
                        "This is the last WoW version here, so the shared game data goes with \
                         it."
                            .to_owned(),
                        theme::ERROR,
                    ),
                    false => {
                        let others = plan
                            .remaining
                            .iter()
                            .map(|c| self.label_of(c))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let note = match &preview.reclaim {
                            // Only what nobody else uses goes; an archive
                            // it shares with them keeps its bytes as dead
                            // space until something rewrites it.
                            Some(Ok(r)) if r.stranded > 0 => format!(
                                "Game data it shares with {others} stays. About {} of its own \
                                 sits in archives shared with them and stays as unused space.",
                                size_of(r.stranded)
                            ),
                            Some(Ok(_)) => format!("Game data it shares with {others} stays."),
                            _ => format!(
                                "Could not check which game data only it uses, so the shared \
                                 store is left as it is for {others}."
                            ),
                        };
                        (note, theme::MUTED)
                    }
                };
                body = body.push(text(note).size(theme::LABEL).color(color)).push(muted(
                    "Its addons and settings (Interface, WTF) are deleted with it. Battle.net \
                     stops listing it too."
                        .to_owned(),
                    theme::LABEL,
                ));
            }
        }

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding([theme::SM, theme::MD])
        };
        let actions = row![
            Space::with_width(Length::Fill),
            choice("Cancel")
                .style(theme::outlined)
                .on_press_maybe((!dialog.working).then_some(Message::UninstallCancel)),
            choice(if dialog.working { "Uninstalling…" } else { "Uninstall" })
                .style(theme::filled(theme::ERROR))
                .on_press_maybe((ready && !dialog.working).then_some(Message::UninstallConfirm)),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section(format!("Uninstall {}", dialog.label), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// A product's name as the picker shows it, or its code.
    fn label_of(&self, code: &ProductCode) -> String {
        self.choices
            .iter()
            .find(|c| &c.code == code)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| code.as_str().to_owned())
    }

    /// Where the library lives, how full its disk is, and what is using it.
    fn storage_section(&self) -> Element<'_, Message> {
        let report = self.storage.as_ref();

        let mut location = column![row![
            mono(self.layout.root().display().to_string()),
            Space::with_width(Length::Fill),
            secondary("Open folder", Message::OpenLibrary).enabled(true),
        ]
        .spacing(theme::MD)
        .align_y(Alignment::Center)]
        .spacing(theme::SM);

        if let Some(disk) = report.and_then(|r| r.disk) {
            let used = disk.total.saturating_sub(disk.free);
            let fraction = match disk.total {
                0 => 0.0,
                total => used as f32 / total as f32,
            };
            location = location
                .push(progress_bar(0.0..=1.0, fraction).height(6))
                .push(muted(
                    format!("{} free of {}", size_of(disk.free), size_of(disk.total)),
                    theme::CAPTION,
                ));
        }

        // Editable only while it is a config change — see `relocate`.
        let movable = report.is_some() && storage::library_is_empty(&self.layout);
        location = location.push(Space::with_height(Length::Fixed(theme::XS as f32)));
        location = match movable {
            true => location
                .push(
                    row![
                        text_input("~/Games/Blizzard", &self.location_input)
                            .on_input(Message::LocationChanged)
                            .on_submit(Message::UseLocation)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                        secondary("Use this location", Message::UseLocation).enabled(true),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )
                .push_maybe(self.hint("Games and the Wine prefix are installed here. It can be changed while \
                     nothing is installed yet.")),
            false => location.push_maybe(self.hint("Games and the Wine prefix are installed here. Moving an installed library \
                 is not supported yet.")),
        };

        let mut usage = column![].spacing(theme::SM);
        match report {
            None => usage = usage.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                for item in &report.usage {
                    let clearing = self.cache_notice.as_deref() == Some("clearing…");
                    let action: Element<Message> = match item.clearable {
                        true => secondary("Clear", Message::ClearCache)
                            .enabled(item.bytes > 0 && !clearing)
                            .into(),
                        false => Space::with_width(Length::Shrink).into(),
                    };
                    usage = usage.push(
                        row![
                            column![
                                text(item.label).size(theme::BODY),
                                muted(item.path.display().to_string(), theme::CAPTION),
                            ]
                            .spacing(2),
                            Space::with_width(Length::Fill),
                            mono(size_of(item.bytes)),
                            action,
                        ]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                    );
                }
                usage = usage.push_maybe(self.hint("Clearing the cache removes downloaded art, installers and lists; each is \
                     fetched again when needed. Shader caches are kept — rebuilding them is \
                     what makes a game stutter."));
                if let Some(notice) = &self.cache_notice {
                    usage = usage.push(muted(notice.clone(), theme::LABEL));
                }
                if self.storage_measuring {
                    usage = usage.push(muted("updating…".to_owned(), theme::CAPTION));
                }
            }
        }

        column![
            location,
            Space::with_height(Length::Fixed(theme::MD as f32)),
            section("What's using space", usage.into()),
        ]
        .spacing(theme::SM)
        .into()
    }

    /// The version, and where its configuration lives.
    fn about_section(&self) -> Element<'_, Message> {
        let mut body = column![field_row(
            "Version".to_owned(),
            mono(env!("CARGO_PKG_VERSION").to_owned()).into(),
        )]
        .spacing(theme::SM);
        if let Some(dirs) = &self.dirs {
            body = body.push(field_row(
                "Config file".to_owned(),
                mono(dirs.config_file().display().to_string()).into(),
            ));
        }
        body.into()
    }
}

/// The optional downloads Lightforge offers to switch on.
///
/// This lists what is *shown*, not what is gated: every optional tag a build
/// carries stays off unless opted in, named here or not, so a tag Blizzard
/// adds tomorrow is still excluded — hardcoding the gate is what once put a
/// 37 GiB texture pack in every plan. `Alternate` and `Talebound` exist in
/// the manifests but are not something to offer a player a switch for.
const OFFERED_OPTIONAL: &[&str] = &[HIGH_RES];

/// The high-resolution texture pack's manifest tag.
const HIGH_RES: &str = "HighRes";

/// The tags in `tags` that [`OFFERED_OPTIONAL`] names.
fn offered_only(tags: &[String]) -> Vec<String> {
    tags.iter().filter(|tag| OFFERED_OPTIONAL.contains(&tag.as_str())).cloned().collect()
}

/// A human label for an optional-download tag.
///
/// The manifest's own name is the fallback, for anything added to
/// [`OFFERED_OPTIONAL`] before it gets a phrase.
fn optional_label(tag: &str) -> String {
    match tag {
        "HighRes" => "High-resolution textures".to_owned(),
        other => other.to_owned(),
    }
}

/// Secondary text at a given size.
fn muted<'a>(content: String, size: u16) -> iced::widget::Text<'a> {
    text(content).size(size).color(theme::MUTED)
}

/// The key that reveals the hints, with Control, Alt and Shift held.
///
/// Not a setting and not in any menu: the prose it brings back is written for
/// whoever is building Lightforge, and a user who found it by accident would
/// be reading notes meant for someone else. It lasts as long as the window,
/// so nothing on disk records that it was ever pressed.
const HINTS_CHORD: &str = "h";

/// Whether a key press is the chord that reveals the hints.
///
/// All three modifiers, deliberately: Control and Shift alone are one slip
/// away from shortcuts people use, and the prose behind this is not something
/// a user should meet by fumbling a paste.
fn hints_chord(key: &iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> bool {
    matches!(key.as_ref(), iced::keyboard::Key::Character(HINTS_CHORD))
        && modifiers.control()
        && modifiers.alt()
        && modifiers.shift()
}

/// Height of the game band.
///
/// Pinned rather than left to the tiles: the band fills its cross axis, and a
/// widget that does that is handed the height of its siblings — the buttons,
/// which are shorter than a tile.
const STRIP_HEIGHT: f32 = theme::TILE + theme::XS as f32 * 2.0;

/// Width of a band button, pinned so the band's arithmetic is exact rather
/// than an estimate of how wide a glyph renders.
const BAND_BUTTON: f32 = 48.0;

/// How big a mark on a band button is drawn: the box it sits in is
/// [`BAND_BUTTON`] less its padding, and a mark wider than that would push the
/// button out of the size the band's arithmetic assumes.
const BAND_GLYPH: f32 = BAND_BUTTON - theme::MD as f32 * 2.0;

/// Add a game, and the overflow button beside it: one mark in a fixed box, so
/// the two match and [`BAND_BUTTON`] stays true.
fn band_button<'a>(
    face: impl Into<Element<'a, Message>>,
    message: Message,
    open: bool,
    hint: String,
) -> Element<'a, Message> {
    let face = container(face)
        .center_x(Length::Fixed(BAND_BUTTON - theme::MD as f32 * 2.0))
        .center_y(Length::Fixed(theme::TILE));
    tooltip(
        button(face)
            .padding([theme::XS, theme::MD])
            .style(match open {
                true => theme::selected,
                false => theme::ghost,
            })
            .on_press(message),
        container(text(hint).size(theme::LABEL)).padding([theme::XS, theme::SM]).style(theme::floating),
        tooltip::Position::Bottom,
    )
    .into()
}

/// Which tiles the band shows, and which go behind the overflow button.
///
/// Pure, so the rules can be tested without a window: `widths` is what each
/// tile needs, `room` what the band has for tiles, and the answer is indices
/// into `widths` — shown first, hidden second, both in the order given.
///
/// Nothing is hidden while everything fits. Once something must be, the
/// overflow button costs room too, and the selected line is never what
/// disappears: it is the game the page is showing, so a band that dropped it
/// would stop saying where you are.
fn divide_strip(widths: &[f32], selected: usize, room: f32) -> (Vec<usize>, Vec<usize>) {
    let gap = theme::SM as f32;
    let span = |ids: &[usize]| -> f32 {
        ids.iter().map(|id| widths[*id]).sum::<f32>()
            + gap * ids.len().saturating_sub(1) as f32
    };

    let all: Vec<usize> = (0..widths.len()).collect();
    if all.is_empty() || span(&all) <= room {
        return (all, Vec::new());
    }

    let budget = room - BAND_BUTTON - gap;
    let mut shown: Vec<usize> = Vec::new();
    for id in 0..widths.len() {
        let mut next = shown.clone();
        next.push(id);
        if span(&next) > budget {
            break;
        }
        shown = next;
    }

    // The selected line takes the place of the last tile that fit, and of the
    // one before that if it still does not fit. A band showing only the
    // selected line is the floor: something is always lit.
    if !shown.contains(&selected) {
        shown.pop();
        while !shown.is_empty() {
            let mut with_selected = shown.clone();
            with_selected.push(selected);
            if span(&with_selected) <= budget {
                break;
            }
            shown.pop();
        }
        shown.push(selected);
    }

    let hidden = all.iter().copied().filter(|id| !shown.contains(id)).collect();
    (shown, hidden)
}

/// Machine text — versions, paths, ids. Monospaced so a version number cannot
/// be mistaken for prose, and so columns line up without a table.
fn mono<'a>(content: String) -> iced::widget::Text<'a> {
    text(content)
        .size(theme::CAPTION)
        .font(theme::MONO)
        .color(theme::MUTED)
}

/// A status pill.
fn pill<'a>(content: String, color: iced::Color) -> Element<'a, Message> {
    container(text(content).size(theme::CAPTION))
        .padding([2, theme::SM])
        .style(theme::badge(color))
        .into()
}

/// An empty state: what happened, and what to do about it.
fn empty<'a>(headline: &str, hint: &str) -> Element<'a, Message> {
    container(
        column![
            text(headline.to_owned()).size(theme::BODY),
            muted(hint.to_owned(), theme::CAPTION),
        ]
        .spacing(theme::XS)
        .align_x(Alignment::Center),
    )
    .padding(theme::XL)
    .center_x(Length::Fill)
    .style(theme::card)
    .into()
}

/// A button that is disabled rather than absent when it cannot be used —
/// buttons appearing and disappearing makes a UI feel unstable.
struct Action<'a> {
    inner: button::Button<'a, Message>,
    message: Message,
}

impl<'a> Action<'a> {
    fn enabled(self, enabled: bool) -> button::Button<'a, Message> {
        match enabled {
            true => self.inner.on_press(self.message),
            false => self.inner,
        }
    }

    fn width(mut self, width: f32) -> Self {
        self.inner = self.inner.width(Length::Fixed(width));
        self
    }
}

fn action_button(label: &str, color: iced::Color, message: Message) -> Action<'static> {
    Action {
        inner: button(text(label.to_owned()).size(theme::LABEL))
            .style(theme::filled(color))
            .padding([theme::SM, theme::MD]),
        message,
    }
}

fn secondary(label: &str, message: Message) -> Action<'static> {
    Action {
        inner: button(text(label.to_owned()).size(theme::LABEL))
            .style(theme::outlined)
            .padding([theme::XS, theme::MD]),
        message,
    }
}

/// Settings content stops growing here: rows read as one column at any
/// window width, and a label and its control never drift apart.
const SETTINGS_WIDTH: f32 = 720.0;

/// A heading over a hairline, then its body — Settings' unit of grouping.
fn section<'a>(title: impl Into<String>, body: Element<'a, Message>) -> Element<'a, Message> {
    column![
        text(title.into()).size(theme::HEADING),
        container(Space::with_height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(theme::divider),
        body,
    ]
    .spacing(theme::MD)
    .into()
}

/// A fixed-width label beside its control, so every control in a section
/// starts on the same line.
fn field_row<'a>(label: String, control: Element<'a, Message>) -> Element<'a, Message> {
    row![muted(label, theme::LABEL).width(Length::Fixed(160.0)), control]
        .spacing(theme::MD)
        .align_y(Alignment::Center)
        .into()
}

/// A separator between inline facts.
fn dot<'a>() -> Element<'a, Message> {
    text("·").size(theme::CAPTION).color(theme::MUTED).into()
}

/// Place a transfer sample within its job — see [`Job`].
fn job_progress(job: Job, sample: &TransferState, bytes_per_second: u64) -> state::Progress {
    let stored = sample.progress.bytes_stored;
    let (downloaded, total) = match (job.total, sample.remaining) {
        // Whatever the job planned that this run no longer has to fetch is
        // already done.
        (Some(total), Some(remaining)) => {
            ((total.saturating_sub(remaining) + stored).min(total), total)
        }
        // Resumed and still planning.
        (Some(total), None) => (job.reached.min(total), total),
        // A first run before its plan: nothing to measure against yet.
        (None, _) => (stored, sample.progress.bytes_total),
    };
    state::Progress { downloaded_bytes: downloaded, total_bytes: total, bytes_per_second }
}

/// Progress carried by the two transfer states, if this is one of them.
fn progress_of(action: &ActionState) -> Option<state::Progress> {
    match action {
        ActionState::Downloading(p) | ActionState::Paused(p) => Some(*p),
        _ => None,
    }
}

/// Instantaneous transfer rate, smoothed.
///
/// A raw delta between two samples swings wildly when a large file lands in one
/// tick; an exponential average keeps the readout legible without lagging a
/// real slowdown for long.
#[derive(Debug, Default)]
struct Rate {
    last_bytes: u64,
    last_at: Option<Instant>,
    per_second: u64,
}

impl Rate {
    const SMOOTHING: f64 = 0.3;

    fn sample(&mut self, bytes: u64) {
        let now = Instant::now();
        let Some(previous) = self.last_at else {
            self.last_at = Some(now);
            self.last_bytes = bytes;
            return;
        };
        let elapsed = now.duration_since(previous).as_secs_f64();
        if elapsed < 0.25 {
            return;
        }

        let delta = bytes.saturating_sub(self.last_bytes) as f64;
        let instant = delta / elapsed;
        self.per_second = match self.per_second {
            0 => instant as u64,
            previous => (previous as f64 * (1.0 - Self::SMOOTHING) + instant * Self::SMOOTHING)
                as u64,
        };
        self.last_bytes = bytes;
        self.last_at = Some(now);
    }
}

fn bytes_of(done: u64, total: u64) -> String {
    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    match total {
        0 => format!("{:.2} GiB", gib(done)),
        _ => format!("{:.2} of {:.2} GiB", gib(done), gib(total)),
    }
}

/// Describe a config that would not load, keeping a copy of it aside.
///
/// The whole error chain, not just the top: "could not parse config.toml" is
/// useless without the line and column underneath it.
fn broken_config(error: &lightforge_core::config::ConfigError, path: &Path) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(&format!("\n{cause}"));
        source = cause.source();
    }
    let backup = path.with_extension("toml.broken");
    if std::fs::copy(path, &backup).is_err() {
        message.push_str("\n(could not save a copy to config.toml.broken)");
    }
    format!("{}\n\n{message}", path.display())
}

/// A typed folder as a full path, `~/` expanded.
fn parse_path(typed: &str) -> Result<PathBuf, String> {
    let typed = typed.trim();
    let path = match typed.strip_prefix("~/") {
        Some(rest) => {
            PathBuf::from(std::env::var_os("HOME").ok_or("cannot find your home directory")?)
                .join(rest)
        }
        None => PathBuf::from(typed),
    };
    match path.is_absolute() {
        true => Ok(path),
        false => Err(format!("{typed} is not a full path — start it with / or ~/")),
    }
}

/// A size in the unit it reads best in: a 16 MB cache is not "0.02 GiB".
fn size_of(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    match b {
        b if b >= MIB * 1024.0 * 1024.0 => format!("{:.2} TiB", b / (MIB * 1024.0 * 1024.0)),
        b if b >= MIB * 1024.0 => format!("{:.1} GiB", b / (MIB * 1024.0)),
        b => format!("{:.0} MiB", b / MIB),
    }
}

fn rate_of(per_second: u64) -> String {
    match per_second {
        0 => "—".to_owned(),
        r => format!("{:.1} MB/s", r as f64 / 1e6),
    }
}

fn eta_of(progress: state::Progress) -> String {
    match progress.eta() {
        None => "ETA —".to_owned(),
        Some(eta) => {
            let secs = eta.as_secs();
            match secs {
                0..=59 => format!("ETA {secs}s"),
                60..=3599 => format!("ETA {}m", secs / 60),
                _ => format!("ETA {}h {}m", secs / 3600, (secs % 3600) / 60),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str, modifiers: iced::keyboard::Modifiers) -> bool {
        hints_chord(&iced::keyboard::Key::Character(text.into()), modifiers)
    }

    #[test]
    fn the_hints_need_all_three_modifiers() {
        use iced::keyboard::Modifiers;
        let all = Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT;
        assert!(chord("h", all));
        assert!(!chord("h", Modifiers::CTRL | Modifiers::SHIFT), "Ctrl+Shift+H is a near miss");
        assert!(!chord("h", Modifiers::CTRL), "and plain Ctrl+H is someone else's shortcut");
        assert!(!chord("h", Modifiers::default()), "typing an h is just typing");
        assert!(!chord("g", all), "another letter is not the chord");
    }

    /// A band wide enough for four 54pt tiles and their gaps.
    fn room_for(tiles: usize) -> f32 {
        54.0 * tiles as f32 + theme::SM as f32 * tiles.saturating_sub(1) as f32
    }

    #[test]
    fn a_band_with_room_to_spare_hides_nothing() {
        let widths = [54.0; 3];
        let (shown, hidden) = divide_strip(&widths, 0, room_for(3));
        assert_eq!(shown, vec![0, 1, 2]);
        assert!(hidden.is_empty(), "nothing overflows when everything fits");
    }

    #[test]
    fn the_overflow_button_is_paid_for_out_of_the_band() {
        // Room for four tiles exactly, five to place: the fifth does not fit,
        // and making space for the button costs a fourth tile as well.
        let widths = [54.0; 5];
        let (shown, hidden) = divide_strip(&widths, 0, room_for(4));
        assert_eq!(shown, vec![0, 1, 2]);
        assert_eq!(hidden, vec![3, 4]);
    }

    #[test]
    fn the_selected_game_is_never_the_one_that_disappears() {
        let widths = [54.0; 5];
        let (shown, hidden) = divide_strip(&widths, 4, room_for(4));
        assert!(shown.contains(&4), "the game on screen stays in the band");
        assert_eq!(shown.last(), Some(&4), "it takes the place nearest the menu");
        assert!(!hidden.contains(&4));
        assert_eq!(hidden, vec![2, 3], "the tiles it displaced are the ones behind the menu");
    }

    #[test]
    fn a_band_with_almost_no_room_still_shows_the_selected_game() {
        let widths = [54.0; 4];
        let (shown, hidden) = divide_strip(&widths, 2, 10.0);
        assert_eq!(shown, vec![2], "something is always lit");
        assert_eq!(hidden, vec![0, 1, 3]);
    }

    #[test]
    fn a_wide_name_tile_costs_more_of_the_band_than_an_icon() {
        // Not a layout assertion — just that the estimate scales with the
        // name, since that is what keeps a long label from being cut off.
        let widths = [54.0, 200.0, 54.0];
        let (shown, hidden) = divide_strip(&widths, 0, room_for(4));
        assert_eq!(shown, vec![0], "the wide tile does not fit beside the first");
        assert_eq!(hidden, vec![1, 2]);
    }

    fn planned(remaining: u64, stored: u64) -> TransferState {
        TransferState {
            remaining: Some(remaining),
            progress: lightforge_core::tact::download::Progress {
                bytes_stored: stored,
                ..Default::default()
            },
            ..TransferState::default()
        }
    }

    #[test]
    fn an_update_measures_what_it_fetches_not_the_install() {
        // The symptom this guards: a 10 GiB update to a 90 GiB install opened
        // at "80 of 90 GiB", counting the untouched install as progress.
        let gib = 1024u64 * 1024 * 1024;
        let job = Job { total: Some(10 * gib), reached: 0 };

        let start = job_progress(job, &planned(10 * gib, 0), 0);
        assert_eq!((start.downloaded_bytes, start.total_bytes), (0, 10 * gib));

        let later = job_progress(job, &planned(10 * gib, 4 * gib), 0);
        assert_eq!(later.downloaded_bytes, 4 * gib);
    }

    #[test]
    fn a_resumed_run_continues_its_job_rather_than_restarting() {
        // Pause 3 GiB into a 10 GiB job: the next run plans only the 7 GiB
        // still missing, and the bar must carry on from 3, not from 0.
        let gib = 1024u64 * 1024 * 1024;
        let paused = Download {
            code: ProductCode::Retail,
            label: String::new(),
            stage: Stage::Paused(state::Progress {
                downloaded_bytes: 3 * gib,
                total_bytes: 10 * gib,
                bytes_per_second: 0,
            }),
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job { total: Some(10 * gib), reached: 0 },
        };
        let job = paused.resumed();

        // Still planning: hold where the pause left it.
        let planning = job_progress(job, &TransferState::default(), 0);
        assert_eq!((planning.downloaded_bytes, planning.total_bytes), (3 * gib, 10 * gib));

        // Planned: a run byte moves the job readout by one byte.
        let running = job_progress(job, &planned(7 * gib, gib), 0);
        assert_eq!((running.downloaded_bytes, running.total_bytes), (4 * gib, 10 * gib));
    }

    #[test]
    fn progress_stays_run_relative_until_the_plan_is_known() {
        let sample = TransferState {
            progress: lightforge_core::tact::download::Progress {
                bytes_stored: 500,
                bytes_total: 1000,
                ..Default::default()
            },
            ..TransferState::default()
        };
        let p = job_progress(Job::default(), &sample, 0);
        assert_eq!((p.downloaded_bytes, p.total_bytes), (500, 1000));
    }

    #[test]
    fn a_resume_during_the_wind_down_is_not_swallowed() {
        let (mut app, _) = Lightforge::new();
        // Nothing read from or written to the real queue file.
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        // Paused, then resumed before the paused run reported back: the entry
        // is already a fresh, queued one and the line is still winding down.
        app.queue.push(Download {
            code: code.clone(),
            label: String::new(),
            stage: Stage::Queued,
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job::default(),
        });
        app.winding_down = Some(code.clone());

        let stopped = Complete { complete: false, cancelled: true, files: 0, version: None };
        let _ = app.update(Message::Downloaded(code, Ok(stopped)));

        assert!(app.winding_down.is_none());
        assert!(matches!(app.queue[0].stage, Stage::Running { .. }), "the resume must start");
        assert_eq!(app.status, "downloading…");
    }

    #[test]
    fn a_download_running_at_close_is_saved_as_running() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        app.queue.push(Download {
            code: code.clone(),
            label: String::new(),
            stage: Stage::Queued,
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job { total: Some(12 << 30), reached: 0 },
        });
        let _ = app.pump();
        let running = app.queue_snapshot();
        assert_eq!(running.entries[0].standing, Standing::Running);
        app.saved_queue = running.clone();

        // Closing pauses the transfer to checkpoint it; that pause must not
        // be what the next start restores.
        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert!(matches!(app.queue[0].stage, Stage::Paused(_)));
        assert_eq!(app.saved_queue, running);
    }

    #[test]
    fn a_config_that_will_not_load_is_described_kept_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("lightforge-badcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dirs = AppDirs::with_dirs(dir.join("config"), dir.join("cache"), dir.join("data"));
        std::fs::create_dir_all(dirs.config_dir()).unwrap();
        let broken = "default_proton = \"GE-Proton10-34\"\nprefix_user = [oops\n";
        std::fs::write(dirs.config_file(), broken).unwrap();

        let error = Config::load_or_default(&dirs.config_file()).unwrap_err();
        let message = broken_config(&error, &dirs.config_file());
        assert!(message.contains("line 2"), "names where: {message}");
        let backup = dirs.config_file().with_extension("toml.broken");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), broken);

        let (mut app, _) = Lightforge::new();
        app.dirs = Some(dirs.clone());
        app.queue.clear();
        app.config_error = Some(message);
        app.config.download_limit_mbps = Some(5);
        app.save_config();
        app.persist_queue();
        assert_eq!(std::fs::read_to_string(dirs.config_file()).unwrap(), broken);
        assert!(!dirs.queue_file().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_offered_optional_content_is_shown_or_fetched() {
        let tags = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        // What the Forever beta's manifest carries.
        assert_eq!(offered_only(&tags(&["Alternate", "HighRes", "Talebound"])), tags(&["HighRes"]));
        // A stale opt-in from before the list existed is dropped, not fetched.
        assert!(offered_only(&tags(&["Talebound"])).is_empty());
    }

    #[test]
    fn the_rate_meter_smooths_rather_than_tracking_one_tick() {
        let mut rate = Rate::default();
        rate.sample(0);
        // First real sample seeds the average outright.
        std::thread::sleep(Duration::from_millis(300));
        rate.sample(30_000_000);
        let first = rate.per_second;
        assert!(first > 0);

        // A tick with nothing stored must not drop the readout to zero.
        std::thread::sleep(Duration::from_millis(300));
        rate.sample(30_000_000);
        assert!(rate.per_second > 0 && rate.per_second < first);
    }

    #[test]
    fn eta_is_rendered_in_the_largest_useful_unit() {
        let p = |done: u64, total: u64, rate: u64| state::Progress {
            downloaded_bytes: done,
            total_bytes: total,
            bytes_per_second: rate,
        };
        assert_eq!(eta_of(p(0, 30, 1)), "ETA 30s");
        assert_eq!(eta_of(p(0, 600, 1)), "ETA 10m");
        assert_eq!(eta_of(p(0, 7200, 1)), "ETA 2h 0m");
        // Stalled: no rate means no honest estimate.
        assert_eq!(eta_of(p(0, 100, 0)), "ETA —");
    }

    #[test]
    fn byte_readouts_survive_an_unknown_total() {
        assert_eq!(bytes_of(1024 * 1024 * 1024, 0), "1.00 GiB");
        assert!(bytes_of(0, 1024 * 1024 * 1024).ends_with("of 1.00 GiB"));
    }
}
