//! Lightforge GUI, the dashboard.
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
mod app;
mod art;
mod fit;
mod focus;
mod pad;
mod popover;
mod provision;
mod report;
mod tasks;
mod theme;
mod trace;
mod update;
mod view;
mod widgets;
mod wizard;

mod format;
mod icons;
mod message;
mod model;

use format::*;
use icons::*;
use message::*;
use model::*;
use widgets::*;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use iced::widget::{
    button, canvas, container, image, mouse_area, progress_bar, responsive,
    rich_text, row, scrollable, span, stack, text, text_editor, text_input, tooltip, Space,
};
use iced::{Alignment, ContentFit, Element, Length, Subscription, Task};

use lightforge_core::error::chain;
use lightforge_core::addons::manage;
use lightforge_core::addons::richtext;
use lightforge_core::addons::Channel;
use lightforge_core::assets::{Manifest, Pipeline, Store};
use lightforge_core::config::{Config, SteamOsUse};
use lightforge_core::debug::DebugInfo;
use lightforge_core::library;
use lightforge_core::net::Http;
use lightforge_core::news;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::{Branch, Catalog, Kind, Product, ProductCode, ProductLine};
use lightforge_core::state::{self, ActionState, Local};
use lightforge_core::proton::{self, ProtonPin};
use lightforge_core::setup::Status as SetupStatus;
use lightforge_core::storage;
use lightforge_core::tact::install::Offerings;
use lightforge_core::transfers::{self, Saved, Standing};
use lightforge_core::tact::download::Cancel;
use lightforge_core::tweaks::{GraphicsApi, HostTools, Preset, Tweaks};

use art::Art;
use tasks::{Problem, Refreshed, SharedDownloads, TransferState};

/// Where a player manages their subscription, per the spec.
const ACCOUNT_URL: &str = "https://account.battle.net/games";

/// How far the News backdrop is darkened over the body of the page.
///
/// Higher than the dashboard's mid-page 0.35 because far more text sits over
/// it: a hundred headline cards rather than a handful of controls. At 0.70
/// the picture still reads as a picture between the cards while the muted
/// summary text keeps the contrast it has on a plain page.
const NEWS_SCRIM: f32 = 0.70;

/// How far it is darkened across the header and the page title.
///
/// The navigation row has no backdrop of its own, and this artwork is pale
/// sky across its top third, the one combination that leaves white text
/// sitting on white. Held, not ramped, so the whole row is on even cover.
const NEWS_SCRIM_TOP: f32 = 0.88;

/// The desktop entry's basename, the Wayland `app_id` and the AppStream
/// component id, which are required to be the same string, see
/// `packaging/README.md`.
const APP_ID: &str = "pro.tuxcore.Lightforge";

/// Opening size, and the floor for resizing, deliberately the same number.
///
/// Below it the dashboard does not squeeze, it loses its fixed right-hand
/// column, and PLAY with it. It is also the smallest screen Lightforge fits:
/// a 1366x768 laptop cannot show 820pt plus a title bar. A tiling compositor
/// can still hand the window a smaller slot; nothing here can prevent that.
const WINDOW: iced::Size = iced::Size::new(1280.0, 820.0);
const WINDOW_MIN: iced::Size = WINDOW;

/// The splash card, the only thing painted while a start's artwork is
/// fetched. Small enough to read as a loading screen rather than as a
/// launcher that has come up broken, which is the complaint it answers: a
/// cold start used to show the full window with an empty card for a second or
/// two per game, while that game's background and wordmark came down.
///
/// The *window* is the launcher's size throughout; see [`main`]. Only this
/// card is drawn, on a transparent background, so what anyone sees is a small
/// panel floating in the middle of the screen.
const SPLASH: iced::Size = iced::Size::new(460.0, 280.0);

/// How long the splash card is kept on screen at the very least.
///
/// A warm cache answers in milliseconds, and a card that appears and vanishes
/// inside two frames reads as a glitch rather than as a start. Short enough
/// that "nothing to fetch" still feels instant, a start is never *slower*
/// than it was, only less ugly.
const SPLASH_FLOOR: Duration = Duration::from_millis(350);

/// A ceiling on the fetch, so a machine with no network, or one behind a
/// captive portal that never answers, still reaches its launcher. Whatever
/// landed inside the budget is kept; the rest is fetched on demand as it
/// always was.
const SPLASH_BUDGET: Duration = Duration::from_secs(12);

// No renderer is forced. iced 0.13's wgpu lagged on Wayland resizes
// (iced-rs/iced#2750); 0.14 fixed it, and wgpu is now the smoother one. With
// `ICED_BACKEND` unset iced tries wgpu and falls back to tiny-skia on any
// init error, so a machine without a working GPU still gets a window;
// `ICED_BACKEND=tiny-skia` stays as the escape hatch for hardware that
// initialises fine but renders badly.

/// Keep large, short-lived allocations out of the heap.
///
/// Decoding pictures makes big buffers that are freed correctly, but glibc
/// raises its mmap threshold as it watches them (up to 32 MiB), so later ones
/// come off the heap and freed pages stay resident, hundreds of MB across 48
/// arenas. Measured on News with 97 images: 444 MB RSS as shipped, 149 MB with
/// fixed trim and mmap thresholds. Pinning the threshold is what stops the
/// drift. glibc only; musl would need measuring again.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    // 128 KiB: large enough that ordinary small allocations are untouched,
    // small enough that every image buffer is mapped and so handed straight
    // back on free.
    const LARGE: libc::c_int = 128 * 1024;
    // SAFETY: `mallopt` takes two ints and touches only the allocator's own
    // parameters. Called before any thread is spawned.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, LARGE);
        libc::mallopt(libc::M_TRIM_THRESHOLD, LARGE);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

fn main() -> iced::Result {
    // The one flag, answered before anything opens a window. Packaging checks
    // that a release ships the build it says it does by asking both binaries,
    // and a bug report is easier with the version than with a screenshot.
    // Same shape as clap's answer for `lightforge`.
    if std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V") {
        println!("lightforge-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    tune_allocator();
    fit::take_over_scale();
    trace::begin();
    // 0.14 takes the boot function where 0.13 took the title, and runs it
    // from `run` rather than `run_with`.
    iced::application(
        || {
            let (app, task) = Lightforge::new();
            trace::mark("app constructed, startup tasks queued");
            (app, task)
        },
        Lightforge::update,
        Lightforge::view,
    )
    .title("Lightforge")
    .subscription(Lightforge::subscription)
    // Annotated: without the type the closure is inferred for one
    // lifetime, and 0.14's `theme` wants it for any.
    .theme(|_state: &Lightforge| theme::base())
    // 1 everywhere but gamescope, where winit's own guess is pinned off and
    // this is the whole scale; see [`fit`].
    .scale_factor(|state: &Lightforge| state.ui_scale)
    // What the surface is cleared to, which is the other half of the splash
    // being a small window. `transparent: true` only gives the surface an
    // alpha channel, iced still clears it to the theme's background, so
    // without this the "transparent" window is an opaque rectangle with a
    // card in the middle of it. Measured: it dimmed everything behind it.
    .style(|state: &Lightforge, theme| iced::theme::Style {
        background_color: match state.splash.is_some() {
            true => iced::Color::TRANSPARENT,
            false => theme.palette().background,
        },
        text_color: theme.palette().text,
    })
    // The window has its final size from the first frame and is never
    // resized; while the art loads it paints only the [`SPLASH`] card on a
    // transparent, frameless surface. Resizing instead broke three things: a
    // Wayland client cannot position itself, so a window mapped small and
    // grown later stays off-centre for good.
    .window(iced::window::Settings {
        size: WINDOW,
        min_size: Some(WINDOW_MIN),
        decorations: false,
        transparent: true,
        position: iced::window::Position::Centered,
        // The Wayland `app_id` / X11 `WM_CLASS`, and the one thing that ties
        // a running window to the installed desktop entry: a compositor
        // matches `pro.tuxcore.Lightforge.desktop` by this string and by
        // nothing else. Left at winit's default the taskbar shows a generic
        // placeholder instead of the launcher's icon.
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: APP_ID.to_owned(),
            ..Default::default()
        },
        ..iced::window::Settings::default()
    })
    // Closing is handled by the app, so a transfer can checkpoint first.
    // Set after `window`, which replaces the settings wholesale.
    .exit_on_close_request(false)
    .run()
}

struct Lightforge {
    layout: Layout,
    dirs: Option<AppDirs>,
    config: Config,
    /// One client, one connection pool, for every request the update loop
    /// starts. `None` only if reqwest failed to build one at startup.
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    /// The branding manifest, read once and themed from thereafter.
    manifest: Option<Arc<Manifest>>,
    /// Every configured product's bar icon, for those that have one.
    icons: HashMap<ProductCode, tasks::Mark>,
    /// The branch last used in each line, so returning to a tile returns to
    /// the build you were on rather than snapping back to live.
    last_branch: HashMap<ProductLine, ProductCode>,
    /// Which products exist and how they group, built in, then whatever the
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
    /// The band's overflow menu, only ever reachable when something is in it.
    strip_open: bool,
    /// The tile whose right-click menu is open.
    tile_menu: Option<ProductLine>,
    /// The tile under the cursor, if any.
    tile_hover: Option<ProductLine>,
    /// The tile held down, while it is.
    tile_drag: Option<TileDrag>,
    /// The control the keyboard has selected, and the ring is on. `None`
    /// until a key is pressed, and again once the mouse moves.
    focus: Option<focus::Target>,
    /// The control that opened the menu the ring is in, to land on again
    /// when the menu closes.
    focus_return: Option<focus::Target>,
    /// Where the ring was when the mouse moved and took it away, for the
    /// next key to bring it back to. Steam's keyboard moves the pointer
    /// when it closes, and the ring then started over at the logo.
    focus_resume: Option<focus::Target>,
    /// Where the last control pressed by Enter was, and on which page, in
    /// case pressing it made it go away: Keep after Remove, Install inside a
    /// dialog.
    focus_pressed_at: Option<(iced::Rectangle, Screen)>,
    /// Waiting for what a key press opened or closed to be drawn, so the ring
    /// can follow it. Counts down the frames it is willing to wait.
    focus_entering: u8,
    /// A region the ring moves into once it is drawn: the page beside the
    /// Settings list, after Enter on one of its tabs.
    focus_into: Option<&'static str>,
    /// Whether the window has the desktop's focus; see
    /// [`Message::WindowFocused`].
    window_focused: bool,
    /// Which source moved the ring last, and when, so a pad that Steam is
    /// also turning into keys moves it once and not twice.
    nav_last: Option<(NavFrom, Instant)>,
    /// The picker whose list is open, by its key; see [`Lightforge::picker`].
    picker_open: Option<String>,
    /// Running on SteamOS, which is what puts the Deck-or-desktop question
    /// in setup. Read once: the OS does not change under a running app.
    steamos: bool,
    /// What the last "Add Lightforge to Steam" did, said under its button.
    steam_shortcut: Option<Result<String, String>>,
    /// Whether the menu under the wordmark is open.
    app_menu_open: bool,
    /// Pages left behind, oldest first, and pages stepped back out of.
    ///
    /// Two stacks rather than an index into one list, because that is exactly
    /// what back and forward are: going back moves the current page onto
    /// `forward`, going forward moves it onto `visited`, and visiting a new
    /// page from anywhere clears `forward`: the branch you did not take.
    visited: Vec<Screen>,
    forward: Vec<Screen>,
    /// Whether the explanatory prose under the controls is drawn.
    ///
    /// Off for the people who use Lightforge: the interface is meant to be
    /// readable without a paragraph under every switch, and a screen of grey
    /// sentences is what it looks like when it is not. On for whoever is
    /// working on it; see [`HINTS_CHORD`].
    hints: bool,
    /// Battle.net's own mark, if the prefix has one, see
    /// [`Layout::battle_net_icon`]. Decoded once, here, and held as the one
    /// handle: iced has no decoder of its own, and a handle rebuilt in `view`
    /// would be a fresh texture on every frame.
    bnet_icon: Option<image::Handle>,
    /// The headlines, once the News page has asked for them.
    ///
    /// Fetched when that page is first opened rather than at startup: a
    /// launcher's job on the first frame is to show the Play button, and a
    /// feed nobody has looked at yet is not worth a request in front of it.
    news: Option<news::Headlines>,
    /// Whether a fetch is in flight, so the page says "loading" once and the
    /// tab cannot start a second one by being clicked twice.
    news_pending: bool,
    /// Set once the first attempt has finished, so an empty result reads as
    /// "nothing came back" rather than as "not asked yet".
    news_asked: bool,
    /// Headline pictures by their URL, decoded once.
    ///
    /// Keyed by URL rather than by index because the list is re-fetched: a
    /// picture whose article is still in the feed keeps the handle it already
    /// had, and a handle that does not change is a texture iced does not
    /// re-upload.
    news_images: HashMap<String, image::Handle>,
    /// The page's backdrop, decoded once off the UI thread. Drawn when it
    /// arrives; the page is perfectly readable in the instant before.
    news_bg: Option<image::Handle>,

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
    owed_repair: Option<(ProductCode, Box<lightforge_core::tact::verify::Report>)>,
    /// The product a store scan, deep or not, is checking, so its result
    /// lands on that product rather than whichever tile is selected by then.
    scan_for: Option<ProductCode>,
    /// A repair of damaged entries is rewriting the store's indices.
    repairing: Option<ProductCode>,
    /// Versions Battle.net reports installed, read when the catalog opens.
    installed_versions: HashMap<String, String>,
    /// Agent's registry is being watched for Battle.net writing to it.
    registry_watch: Option<RegistryWatch>,
    /// Product codes Battle.net itself uses, folded into the catalog.
    known_codes: Vec<String>,
    /// Transfers paused for a play session, to be put back afterwards.
    resume_after_play: Vec<ProductCode>,
    /// Why `config.toml` could not be loaded. While set, the app shows only
    /// that and writes nothing; see [`Lightforge::new`].
    config_error: Option<String>,
    /// The queue as last written to disk, so an unchanged one is not
    /// rewritten on every message.
    saved_queue: transfers::Queue,
    /// Set once the window is closing: the pause that closing performs must
    /// not be what the next start restores.
    queue_frozen: bool,
    /// A cancelled transfer whose task has not reported back yet.
    ///
    /// Pausing flips the UI at once, but the run keeps going for a moment,
    /// it finishes the requests already in flight and checkpoints the store.
    /// Starting anything before it lands would put two writers on the same
    /// CASC store, which is the corruption pre-flight exists to prevent.
    winding_down: Option<ProductCode>,

    downloads: SharedDownloads,
    busy: bool,
    status: String,
    error: Option<Problem>,
    /// The last few status lines, oldest first, for a bug report to carry.
    history: std::collections::VecDeque<String>,
    /// What was last reported and when, so the same text cannot go out twice
    /// in a row by accident.
    last_report: Option<(String, Instant)>,
    /// Set when the window has asked to close and a transfer is still coming
    /// to rest; the window closes once it has.
    closing: Option<iced::window::Id>,

    screen: Screen,
    setup: Option<SetupStatus>,
    /// The first run, while it is happening. `None` the rest of the time, and
    /// on every start after the one that finished or dismissed it.
    wizard: Option<wizard::Wizard>,
    /// The release notes for the version that has just started, while they
    /// are on screen. `None` once dismissed, or when this start is not the
    /// first one after an update; see [`Lightforge::patch_notes_due`].
    patch_notes: Option<PatchNotes>,
    /// The small window the app opens in. `None` once it has grown into the
    /// launcher, which happens exactly once per run.
    splash: Option<Splash>,
    /// The UI scale under gamescope, from the window's height; 1 elsewhere.
    ui_scale: f32,
    /// Bumped on every change to `ui_scale`, so a height read under the old
    /// one is recognised and asked for again.
    ui_scale_gen: u64,
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
    cancel_dialog: Option<CancelDialog>,
    /// A newer Lightforge on GitHub, found at start-up. Shown once per
    /// release and never installed; see `core::update`.
    update_notice: Option<lightforge_core::update::Available>,
    /// A cancelled transfer whose files go once the run has wound down.
    ///
    /// Deferred rather than done on the click: the downloader may still have
    /// the store open, and deleting a store out from under its own writer is
    /// not something to race. The `Downloaded` message is the run saying it
    /// has stopped, which is when this fires.
    delete_after_cancel: Vec<ProductCode>,
    /// A given-up download's files being removed right now.
    discarding: Option<ProductCode>,
    /// The product being launched, from pre-flight until it exits. Captured
    /// on the click, so the launch, and what follows it, cannot move to a
    /// tile selected in the meantime.
    launching: Option<ProductCode>,
    /// A launch whose pre-flight is done, held until the download it paused
    /// has finished winding down: the client and a still-checkpointing
    /// downloader would share one store.
    launch_when_quiet: Option<Launch>,
    report_dialog: Option<ReportDialog>,
    /// Filled in once, shortly after startup. `None` until then, and a report
    /// sent in that window simply says so rather than blocking on it.
    system: Option<iced::system::Information>,
    downgrade_dialog: Option<DowngradeDialog>,
    /// Builds the user has already agreed to launch under this session.
    ///
    /// Per build rather than once for all: agreeing to one downgrade is not
    /// agreeing to the next one. Not persisted, a new run of the launcher is
    /// a fair moment to ask again, and the prefix may have moved since.
    downgrade_ok: HashSet<String>,
    /// Partitions that could become a library, as last scanned. Refreshed
    /// when the Storage section opens rather than polled, a drive appearing
    /// while the page is shut is not news.
    partitions: Vec<library::Partition>,
    /// What clearing one drive last freed, by library id.
    drive_notice: Option<(String, String)>,
    /// Each library's unused game data, as last checked, by library id.
    unused: HashMap<String, UnusedData>,
    /// Whether nothing is installed yet, as last checked.
    ///
    /// Cached beside `drive_states` and for the same reason: it is two
    /// `read_dir` calls, and the view asks it on every frame.
    library_empty: bool,
    /// Each library's state as last checked, by id.
    ///
    /// Cached rather than asked in the view: `action()` runs on every frame,
    /// and a marker file read per frame would put a filesystem call in the
    /// render path. Refreshed when drives are rescanned and when a product is
    /// reloaded, which is also when a drive going away would matter.
    drive_states: HashMap<String, library::State>,

    /// Optional performance tools this machine has, detected once at startup.
    /// A toggle for a binary that is not installed is a launch failure waiting
    /// to happen, so the panel hides what is missing.
    tools: HostTools,
    /// Editable copies of the selected product's numeric tweaks, and the two
    /// fields of the environment escape hatch.
    fps_input: String,
    fps_bk_input: String,
    monitor_input: String,
    bnet_monitor_input: String,
    env_key_input: String,
    env_value_input: String,

    addons: addonwork::Listing,
    token_input: String,
    /// Narrows the installed list. Held here rather than derived, because it
    /// is the one piece of the addon page the user types into and expects to
    /// survive a refresh of the list underneath it.
    addon_filter: String,
    /// The addon whose Remove has been clicked once. Removing deletes files,
    /// so the second click is the one that means it.
    addon_remove_armed: Option<(String, String)>,
    /// The addon whose per-addon menu is showing. One at a time: the panel is
    /// an overlay, and two of them would draw over each other.
    addon_menu_open: Option<(String, String)>,
    /// Whether the next import reproduces the token's exact versions. Off by
    /// default: most tokens are someone saying which addons they use, not
    /// which builds of them.
    import_pinned: bool,
    /// The version picker, when it is open: which addon, and the builds the
    /// catalogue offers. `None` in the list means the fetch is still out.
    version_picker: Option<VersionPicker>,
    game_settings: Option<GameSettings>,
    addon_detail: Option<AddonDetail>,
    /// Whether the environment-variable editor is expanded, per panel.
    ///
    /// Two flags rather than one because they want different defaults. On a
    /// product's page the card is folded away. It is the escape hatch, below
    /// four cards of switches. In Settings › Battle.net it is open from the
    /// start: a hand-off game has no client settings, so environment
    /// variables are not a last resort there but most of what the page is
    /// for, and hiding them behind a Show button reads as "nothing here".
    env_open: bool,
    bnet_env_open: bool,
    /// Game families, as Blizzard's own catalogue names them, read out of the
    /// client's cache at startup. Empty on a prefix whose client has never
    /// run, which falls the hand-off back to the compiled-in families.
    families: lightforge_core::families::Families,
    /// Products whose unattended pass has already run this session. Addons
    /// are per product, so switching to Classic is the first chance to bring
    /// Classic's up to date, but only the first chance: clicking back and
    /// forth along the band must not re-ask CurseForge each time.
    addon_auto_done: Vec<ProductCode>,
    /// An unattended addon pass is in flight. Separate from `addons_busy`,
    /// which disables the buttons a person pressed: this one nobody asked
    /// for, so it must not grey the page out.
    addon_auto_running: bool,
    /// What the unattended pass did, said once on the Addons page. `None`
    /// when it has not run or had nothing to do, "0 addons updated" is noise.
    addon_auto_note: Option<String>,
    /// A launch waiting for that pass to finish. The game reads AddOns once,
    /// at login, and extracting into that directory while it does is how a
    /// half-written addon gets loaded.
    launch_after_addons: Option<Launch>,
    /// Which tab the standing report belongs to. An import's failures are read
    /// next to the import that caused them; an update's belong with the
    /// addons. Same type, two meanings, so it is remembered rather than
    /// guessed at from the report's contents.
    addon_report_tab: AddonTab,
    /// Counts keystrokes in the search box. A debounced search carries the
    /// number it was scheduled at and does nothing if another key has landed
    /// since, which is what stops a six-letter word becoming six searches.
    search_seq: u64,
    /// The token the Export button built, held so it can be read and copied.
    export_token: String,
    addon_rx: Option<tokio::sync::watch::Receiver<addonwork::Progress>>,
    addon_sample: addonwork::Progress,
    addons_busy: bool,
    addon_report: Option<lightforge_core::addons::Report>,
    addon_tab: AddonTab,
    /// The section the index marks: the one in view, or the one just jumped
    /// to.
    settings_tab: SettingsTab,
    /// A section to scroll to once the page has been measured.
    settings_jump: Option<SettingsTab>,
    /// How many times the jump has found its section not yet laid out.
    settings_jump_tries: u8,
    settings_anchor: Option<SettingsAnchor>,
    settings_layout: SettingsLayout,
    settings_viewport: Option<scrollable::Viewport>,
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
        // app stops on it rather than quietly running on defaults, which
        // point at the default library, and then saving those over it.
        let loaded = dirs.as_ref().map(|d| Config::load_or_default(&d.config_file()));
        let config_error = match (&loaded, dirs.as_ref()) {
            (Some(Err(e)), Some(dirs)) => Some(broken_config(e, &dirs.config_file())),
            _ => None,
        };
        let mut config = match loaded {
            Some(Ok(config)) => config,
            _ => Config::with_default_products(),
        };
        // A config written before multi-drive storage has no libraries in it.
        // Seeded here rather than lazily, because every path below this line
        // resolves through one. Deliberately *not* saved: starting the app is
        // not a reason to write to someone's config, and the seed does not
        // need writing to be stable; see `Config::ensure_libraries`. It goes
        // to disk with the first change the user makes.
        let _ = config.ensure_libraries();
        let config = config;
        // Honours `blizzard_root`, the same way the CLI does.
        let layout = config.layout().unwrap_or_else(|_| Layout::with_root("."));
        let bnet_icon = layout.battle_net_icon().and_then(|path| art::try_mark(&path));

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
            tile_menu: None,
            tile_hover: None,
            tile_drag: None,
            focus: None,
            focus_return: None,
            focus_resume: None,
            focus_pressed_at: None,
            focus_entering: 0,
            picker_open: None,
            focus_into: None,
            window_focused: true,
            nav_last: None,
            steamos: lightforge_core::runtime::OsRelease::read().is_steamos(),
            steam_shortcut: None,
            app_menu_open: false,
            visited: Vec::new(),
            forward: Vec::new(),
            hints: false,
            bnet_icon,
            news: None,
            news_pending: false,
            news_asked: false,
            news_images: HashMap::new(),
            news_bg: None,
            gear_open: false,
            scan_rx: None,
            scan_sample: lightforge_core::tact::verify::Progress::default(),
            scanning: false,
            deep_cancel: None,
            owed_repair: None,
            scan_for: None,
            repairing: None,
            installed_versions: HashMap::new(),
            // From the start: a client left running from before, or the
            // first one after an install that predates Lightforge, writes
            // the registry with nobody having pressed anything here.
            registry_watch: Some(RegistryWatch::new()),
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
            wizard: None,
            patch_notes: None,
            splash: None,
            ui_scale: 1.0,
            ui_scale_gen: 0,
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
            cancel_dialog: None,
            update_notice: None,
            delete_after_cancel: Vec::new(),
            discarding: None,
            launching: None,
            launch_when_quiet: None,
            report_dialog: None,
            system: None,
            history: std::collections::VecDeque::new(),
            last_report: None,
            downgrade_dialog: None,
            downgrade_ok: HashSet::new(),
            partitions: Vec::new(),
            drive_notice: None,
            unused: HashMap::new(),
            library_empty: false,
            drive_states: HashMap::new(),
            tools: HostTools::detect(),
            fps_input: String::new(),
            fps_bk_input: String::new(),
            monitor_input: String::new(),
            bnet_monitor_input: String::new(),
            env_key_input: String::new(),
            env_value_input: String::new(),
            addons: addonwork::Listing::default(),
            token_input: String::new(),
            addon_filter: String::new(),
            addon_remove_armed: None,
            addon_menu_open: None,
            version_picker: None,
            game_settings: None,
            addon_detail: None,
            env_open: false,
            bnet_env_open: true,
            families: lightforge_core::families::Families::default(),
            import_pinned: false,
            addon_auto_done: Vec::new(),
            addon_auto_running: false,
            addon_auto_note: None,
            launch_after_addons: None,
            addon_report_tab: AddonTab::Mine,
            search_seq: 0,
            export_token: String::new(),
            addon_rx: None,
            addon_sample: addonwork::Progress::default(),
            addons_busy: false,
            addon_report: None,
            addon_tab: AddonTab::Mine,
            settings_tab: SettingsTab::General,
            settings_jump: None,
            settings_jump_tries: 0,
            settings_anchor: None,
            settings_layout: SettingsLayout::default(),
            settings_viewport: None,
            search_input: String::new(),
            search: addonwork::SearchOutcome::default(),
            searching: false,
            updates: Vec::new(),
        };
        let mut app = app;
        // Nothing starts on a config that did not load: no probes against a
        // library it may not describe, and above all no restored download.
        if app.config_error.is_some() {
            // The frame still goes on: the broken-config screen is prose
            // someone has to read, close and act on, and a window with no
            // way to close it is not the thing to hand them.
            return (
                app,
                iced::window::oldest().and_then(iced::window::toggle_decorations),
            );
        }
        app.load_tweak_inputs();
        // Before anything resolves a path: a drive that came back at a
        // different mount point is found again here, so a game on it reads as
        // installed rather than as missing. Cheap, one pass over
        // `/proc/self/mounts` and one marker file per absent library.
        app.refresh_drives();
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
        // Addons that update themselves do it now, so they are current by
        // the time anyone presses Play. Quiet, and nothing waits for it
        // except a launch; see `preflight`.
        let addons = Task::done(Message::AutoUpdateAddons);
        // Which tile each product belongs to, for the games Battle.net runs.
        let families = Task::perform(
            tasks::game_families(app.layout.clone(), app.config.prefix_user.clone(), app.config.region),
            |families| Message::FamiliesRead(Box::new(families)),
        );
        // The first run, exactly once: the flag is written when the wizard is
        // finished or dismissed, so a machine that has seen it never sees it
        // again by itself. Nothing is written here, starting the app is not a
        // reason to touch someone's config.
        if !app.config.setup_complete {
            app.wizard = Some(wizard::Wizard::default());
        }
        // Every tracked product's art into the cache before the launcher is
        // worth looking at, so the first visit to a panel is not two seconds
        // of empty card. The splash *is* this fetch, made visible, and the
        // wizard's first slide reads its count off the same run rather than
        // starting a second one.
        let artwork = app.open_splash();
        let task = Task::batch([
            app.reload(selected.code),
            addons,
            families,
            probe,
            installed,
            codes,
            bnet_limit,
            resume,
            artwork,
            iced::system::information().map(|info| Message::SystemInfo(Box::new(info))),
        ]);
        (app, task)
    }

    /// Every message, with the status line watched on the way out.
    ///
    /// Lightforge only traces to stdout, so the status line is the closest
    /// thing to a log it has, and whatever it said in the minute before a
    /// bug report is usually the whole story. Recording it here rather than at
    /// the hundred-odd places that assign it means nothing can be forgotten.
    fn update(&mut self, message: Message) -> Task<Message> {
        let before = (self.status.clone(), self.error.clone());
        // Anything may change how tall a Settings section is, a scan
        // finishing, a list arriving, so while a jump is holding its section
        // at the top, every message re-measures. Only for a moment after the
        // jump: after that the page has settled, and a reader scrolling it
        // is the only thing that moves the index.
        let settling = self.screen == Screen::Settings
            && self.settings_anchor.as_ref().is_some_and(|a| a.since.elapsed() < SETTINGS_JUMP_SETTLE)
            && !matches!(message, Message::SettingsMeasured(_) | Message::SettingsScrolled(_));
        let task = self.step(message);
        let task = match settling {
            true => Task::batch([task, measure_settings()]),
            false => task,
        };

        if self.status != before.0 && !self.status.is_empty() {
            self.remember(self.status.clone());
        }
        if self.error != before.1 {
            if let Some(error) = self.error.clone() {
                self.remember(format!("error: {error}"));
            }
        }
        task
    }

    /// Keep the last [`HISTORY`] things the launcher said.
    fn remember(&mut self, line: String) {
        if self.history.back() == Some(&line) {
            return;
        }
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(line);
    }

    fn step(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.persist_queue();
        task
    }

    fn subscription(&self) -> Subscription<Message> {
        // Only while something is moving: an idle dashboard should wake up for
        // nothing.
        let closing = iced::window::close_requests().map(Message::CloseRequested);

        // The hint chord, and the keys that move the selection ring. Only
        // keys no widget took, so arrows in a text field stay the field's,
        // and only these produce a message, so typing wakes nothing else.
        let hints = iced::keyboard::listen().filter_map(|event| {
            use iced::keyboard::key::Named;
            let iced::keyboard::Event::KeyPressed { ref key, modifiers, .. } = event else {
                return None;
            };
            if hints_chord(key, modifiers) {
                return Some(Message::ToggleHints);
            }
            let input = match key.as_ref() {
                iced::keyboard::Key::Named(Named::ArrowUp) => pad::Input::Step(focus::Step::Up),
                iced::keyboard::Key::Named(Named::ArrowDown) => pad::Input::Step(focus::Step::Down),
                iced::keyboard::Key::Named(Named::ArrowLeft) => pad::Input::Step(focus::Step::Left),
                iced::keyboard::Key::Named(Named::ArrowRight) => pad::Input::Step(focus::Step::Right),
                iced::keyboard::Key::Named(Named::Enter) => pad::Input::Press,
                iced::keyboard::Key::Named(Named::Escape) => pad::Input::Back,
                _ => return None,
            };
            Some(Message::Nav(NavFrom::Keys, input))
        });

        // Gamepads, for when Steam hands us a pad rather than keys. Read on a
        // thread of their own for the whole run; see [`pad`].
        let pads = Subscription::run(pad::inputs).map(|input| Message::Nav(NavFrom::Pad, input));
        let window_focus = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused(true)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocused(false)),
            iced::Event::Window(iced::window::Event::Resized(_)) if fit::owned() => {
                Some(Message::WindowResized)
            }
            _ => None,
        });

        // While the ring is up, the mouse moving takes it away, the way a
        // browser drops its focus outline once you reach for the mouse.
        let pointing = match self.focus.is_some() {
            true => iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { .. }) => {
                    Some(Message::FocusHidden)
                }
                _ => None,
            }),
            false => Subscription::none(),
        };

        // Resize events are only interesting to a human reading a trace, and
        // subscribing to them makes every resize step rebuild the view, so
        // this stays out of a normal run entirely.
        let resizes = match trace::frames() {
            true => iced::window::resize_events().map(|(_, size)| Message::ResizeTraced(size)),
            false => Subscription::none(),
        };

        // A held tile follows the cursor, and only a held one: hover alone
        // comes from the tiles' own mouse areas, so moving across the window
        // wakes nothing.
        let dragging = match self.tile_drag.is_some() {
            true => iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::TileDragged(position.x))
                }
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::TileDropped)
                }
                _ => None,
            }),
            false => Subscription::none(),
        };

        let registry = match self.registry_watch.is_some() {
            true => iced::time::every(REGISTRY_POLL).map(|_| Message::RegistryPoll),
            false => Subscription::none(),
        };

        // The splash's bar sweeps off the clock, so it needs the tick as much
        // as a live transfer does.
        match self.splash.is_some()
            || self.running().is_some()
            || self.setup_running
            || self.addons_busy
            || self.scanning
            || self.uninstall_dialog.as_ref().is_some_and(|d| d.working)
            || self.unused.values().any(|u| matches!(u, UnusedData::Freeing { .. }))
        {
            true => Subscription::batch([
                closing,
                hints,
                pads,
                window_focus,
                resizes,
                dragging,
                pointing,
                registry,
                iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            ]),
            false => Subscription::batch([
                closing, hints, pads, window_focus, resizes, dragging, pointing, registry,
            ]),
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
        focus::set_current(self.focus.clone());
        theme::set_console(self.console());
        if let Some(error) = &self.config_error {
            return self.broken_config_screen(error);
        }
        // Before the wizard: the window is still the small one, and the
        // wizard's card does not fit in it.
        if let Some(splash) = &self.splash {
            return self.splash_screen(splash);
        }
        // The first run is a layer over the app rather than a `Screen`: there
        // is no way out of it except the two buttons in its footer, so nothing
        // else may be reachable while it is up.
        if let Some(open) = &self.wizard {
            let card = wizard::view(self, open);
            // An open dropdown closes on a click anywhere else, as it does
            // on every other page. Wrapped either way: see below.
            let card = mouse_area(card);
            let card = Element::from(match self.panel_open() {
                true => card.on_press(Message::Dismiss),
                false => card,
            });
            return focus::root(self.with_hints(
                container(card).width(Length::Fill).height(Length::Fill).style(theme::backdrop).into(),
            ));
        }
        let screen = match self.screen {
            Screen::Dashboard => self.dashboard(),
            Screen::News => self.news_screen(),
            Screen::Updates => self.updates_screen(),
            Screen::Settings => self.settings(),
            Screen::Addons => self.addons_screen(),
            Screen::Tweaks => self.tweaks_screen(),
        };

        // The manager hangs under its button on every screen, not just the
        // dashboard: a download keeps running while you are in Settings, and
        // that is exactly when you want to look at it. Its button is at the
        // end of the ribbon, so a stacked layer pinned to the top right lands
        // under it, padded clear of the ribbon it drops out of. (Add-a-game
        // used to share this corner; it now hangs off the `+` itself, see
        // [`Lightforge::strip`].)
        let overlay: Option<Element<Message>> =
            match self.manager_open && !self.queue.is_empty() {
                true => Some(self.manager()),
                false => None,
            };
        // Clicks inside a panel are absorbed so they never reach the dismissal
        // beneath it; without this, clicking the gap between two rows of the
        // download manager would close it.
        let overlay = overlay.map(|panel| focus::layer(mouse_area(panel).on_press(Message::MenuClick)));

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
        let card = match (
            &self.install_dialog,
            &self.uninstall_dialog,
            &self.cancel_dialog,
            &self.downgrade_dialog,
            &self.version_picker,
            &self.addon_detail,
            &self.report_dialog,
            &self.game_settings,
            &self.patch_notes,
            &self.update_notice,
        ) {
            (Some(dialog), ..) => Some(self.install_card(dialog)),
            (_, Some(dialog), ..) => Some(self.uninstall_card(dialog)),
            (_, _, Some(dialog), ..) => Some(self.cancel_card(dialog)),
            (_, _, _, Some(dialog), ..) => Some(self.downgrade_card(dialog)),
            (_, _, _, _, Some(picker), ..) => Some(self.version_card(picker)),
            (_, _, _, _, _, Some(detail), ..) => Some(self.detail_card(detail)),
            (_, _, _, _, _, _, Some(dialog), ..) => Some(self.report_card(dialog)),
            (_, _, _, _, _, _, _, Some(settings), ..) => Some(self.game_settings_card(settings)),
            // Last, so a dialog someone opened themselves is never buried
            // under the one the app opened for them. The notes come before
            // the update notice: they are about the build that is running,
            // which is the one the user can do something with right now.
            (.., Some(notes), _) => Some(self.patch_notes_card(notes)),
            (.., Some(available)) => Some(self.update_card(available)),
            _ => None,
        };
        let screen = match card {
            Some(card) => stack![
                screen,
                focus::layer(
                    mouse_area(container(card).center(Length::Fill).style(theme::scrim))
                        // A click no control in the dialog wanted closes a
                        // dropdown open in it, as a click on a page does;
                        // otherwise it only stops at the dialog.
                        .on_press(match self.panel_open() {
                            true => Message::Dismiss,
                            false => Message::MenuClick,
                        }),
                ),
            ]
            .into(),
            None => screen,
        };

        // Anything open closes on a click that no widget wanted. `mouse_area`
        // hands the event to its child first and only fires when the child
        // ignored it, so a button still does its own job rather than being
        // swallowed by the dismissal.
        //
        // Wrapped whether or not anything is open, and only the press comes
        // and goes. Adding the wrapper when a dropdown opened changed the
        // widget tree above the page, iced dropped the page's scroll state,
        // and Settings jumped back to the top.
        let screen = mouse_area(screen);
        let screen = Element::from(match self.panel_open() {
            true => screen.on_press(Message::Dismiss),
            false => screen,
        });

        // The lit corner goes behind everything, last, so every screen gets it
        // and nothing has to opt in. Where a page lays key art over the whole
        // window the art simply wins, which is the right order: the glow is
        // for the chrome, not for the picture.
        let screen = Element::from(
            container(screen)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::backdrop),
        );

        trace::view_built(started.elapsed());
        focus::root(self.with_hints(screen))
    }

}

/// Blizzard's shop, which redirects to the viewer's region by itself.
const SHOP_URL: &str = "https://shop.battle.net";

/// Asks a running Steam to show its on-screen keyboard.
const STEAM_KEYBOARD: &str = "steam://open/keyboard";
/// And to put it away: the pair SDL uses, found in Steam's own libSDL3.
const STEAM_KEYBOARD_CLOSE: &str = "steam://close/keyboard";

/// Where the News page's headlines come from, and where its button goes.
const WOWHEAD_URL: &str = "https://www.wowhead.com";

/// Where the app menu's links point. `None` is a destination that does not
/// exist yet: the entry draws dimmed and unclickable until it is filled in.
const DISCORD_URL: Option<&str> = Some("https://discord.gg/vtTgmUV9Bq");
const X_URL: Option<&str> = Some("https://x.com/simplyKennybtw");
/// Where someone can say thanks. Offered, never asked for: these sit beside
/// the places to get help rather than in front of anything.
const KOFI_URL: Option<&str> = Some("https://ko-fi.com/simplykenny");
const PAYPAL_URL: Option<&str> = Some("https://paypal.me/simplykenny");

#[cfg(test)]
mod tests;
