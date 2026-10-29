//! Lightforge GUI, the dashboard.
//!
//! One product at a time, its primary action being the state machine in
//! [`lightforge_core::state`].
//!
//! - **Progress arrives through a `watch` channel, sampled on a timer**: a
//!   message per stored file would drown the loop.
//! - **Pausing is cancelling**: resuming plans against what is on disk, as
//!   pre-flight's pause before Battle.net does.

#![cfg_attr(test, allow(clippy::cast_possible_truncation))]

mod addonwork;
mod app;
mod art;
mod fit;
mod focus;
mod instance;
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
use wizard::LocaleChoice;
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
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::{Branch, Catalog, Kind, Product, ProductCode, ProductLine};
use lightforge_core::state::{self, ActionState, Local};
use lightforge_core::proton::{self, ProtonPin};
use lightforge_core::setup::Status as SetupStatus;
use lightforge_core::storage;
use lightforge_core::tact::install::Offerings;
use lightforge_core::transfers::{self, Saved, Standing};
use lightforge_core::tact::download::Cancel;
use lightforge_core::tweaks::{HostTools, Tweaks};

use art::Art;
use tasks::{Problem, Refreshed, SharedDownloads, TransferState};

/// Where a player manages their subscription, per the spec.
const ACCOUNT_URL: &str = "https://account.battle.net/games";

/// The desktop entry's basename, Wayland `app_id` and AppStream id, which
/// must match (`packaging/README.md`).
const APP_ID: &str = "pro.tuxcore.Lightforge";

/// Opening size and resize floor: below it the right-hand column, and PLAY,
/// would go. Fits a 1366x768 laptop.
const WINDOW: iced::Size = iced::Size::new(1280.0, 820.0);
const WINDOW_MIN: iced::Size = WINDOW;

/// The splash card, drawn alone on a transparent full-size window while the
/// artwork is fetched, so a cold start does not show empty cards.
const SPLASH: iced::Size = iced::Size::new(460.0, 280.0);

/// The splash card's minimum time on screen: shorter reads as a glitch.
const SPLASH_FLOOR: Duration = Duration::from_millis(350);

/// A ceiling on the splash fetch, for a machine with no network; the rest is
/// fetched on demand.
const SPLASH_BUDGET: Duration = Duration::from_secs(12);

// No renderer is forced: iced tries wgpu and falls back to tiny-skia on an
// init error. `ICED_BACKEND=tiny-skia` is the escape hatch for bad rendering.

/// Keep large, short-lived allocations out of the heap.
///
/// glibc raises its mmap threshold as it sees big image buffers, and freed
/// heap pages stay resident. Measured on 97 images: 444 MB RSS, 149 MB with
/// the thresholds pinned. glibc only.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    // 128 KiB: every image buffer is mapped, small allocations are not.
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

/// Log to a file, or carry on without one: a launcher that refuses to open
/// because its log directory is read-only has made things worse.
fn start_logging() {
    let Ok(dirs) = AppDirs::discover() else {
        return;
    };
    if let Err(e) = lightforge_core::logging::init(&dirs, "lightforge-gui") {
        eprintln!("lightforge: logging disabled: {}", chain(&e));
    }
}

fn main() -> iced::Result {
    // Answered before any window, as clap answers it for `lightforge`;
    // packaging checks it.
    if std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V") {
        println!("lightforge-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    start_logging();
    if !instance::claim(std::env::args().any(|arg| arg == instance::AFTER_UPDATE)) {
        return Ok(());
    }
    tune_allocator();
    fit::take_over_scale();
    trace::begin();
    // 0.14 takes the boot function where 0.13 took the title, and runs it
    // from `run`, not `run_with`.
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
    // `transparent: true` only adds alpha; iced still clears to the theme's
    // background unless told otherwise here.
    .style(|state: &Lightforge, theme| iced::theme::Style {
        background_color: match state.splash.is_some() {
            true => iced::Color::TRANSPARENT,
            false => theme.palette().background,
        },
        text_color: theme.palette().text,
    })
    // Final size from the first frame, never resized: a Wayland client cannot
    // position itself, so a window grown later stays off-centre.
    .window(iced::window::Settings {
        size: WINDOW,
        min_size: Some(WINDOW_MIN),
        decorations: false,
        transparent: true,
        position: iced::window::Position::Centered,
        // The Wayland `app_id` / X11 `WM_CLASS`: how the compositor finds the
        // desktop entry, and so the icon.
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
    /// the build you were on instead of snapping back to live.
    last_branch: HashMap<ProductLine, ProductCode>,
    /// Which products exist and how they group, built in, then whatever the
    /// manifest states over it.
    catalog: Catalog,
    choices: Vec<Choice>,
    selected: Choice,

    local: Local,
    remote: Option<lightforge_core::tact::version::Version>,
    /// The build `/bgdl` lists for the selected product, if any.
    bgdl: Option<lightforge_core::tact::version::Version>,
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
    /// Where the ring was when the mouse took it away, for the next key.
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
    /// Pages left behind, oldest first, and pages stepped back out of. A new
    /// page clears `forward`.
    visited: Vec<Screen>,
    forward: Vec<Screen>,
    /// Whether the explanatory prose under the controls is drawn; off unless
    /// [`HINTS_CHORD`] turns it on.
    hints: bool,
    /// Battle.net's own mark ([`Layout::battle_net_icon`]), decoded once: a
    /// handle built in `view` is a new texture every frame.
    bnet_icon: Option<image::Handle>,
    /// Whether the Battle.net client is up in the prefix, checked every few
    /// seconds: under Wine its tray icon is often nowhere to be seen.
    client_up: bool,
    /// Whether the menu beside a running Battle.net's button is open.
    client_menu_open: bool,
    /// Battle.net is being closed.
    quitting_client: bool,
    quit_client_dialog: Option<QuitClientDialog>,
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
    /// lands on that product, not whichever tile is selected by then.
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
    /// A cancelled transfer still checkpointing; nothing else may write the
    /// store until it reports back.
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
    /// first one after an update; see [`Lightforge::open_patch_notes`].
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
    /// The tested Proton build has been asked for this session, so a failed
    /// fetch (offline) is not retried on every status read.
    tested_proton_asked: bool,
    /// Stable releases that could be installed per family, newest first.
    proton_available: Vec<(proton::ProtonFamily, Vec<String>)>,
    /// Valve Protons this session asked Steam to install.
    steam_proton_asked: HashSet<u32>,
    /// The tab of the "Install Proton Version" window, while it is open.
    proton_installer: Option<ProtonCategory>,
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
    moving: Option<Moving>,
    cancel_dialog: Option<CancelDialog>,
    /// A newer Lightforge on GitHub, found at start-up. Shown once per
    /// release; see `core::update`, and `core::upgrade` for installing it.
    update_notice: Option<lightforge_core::update::Available>,
    /// The newer Lightforge this session knows of, dismissed or not: what
    /// "Update now" in Settings installs.
    newer_release: Option<lightforge_core::update::Available>,
    /// How this copy was installed, which decides what the notice offers.
    /// Read at start, like `exe`: a package manager replacing the binary
    /// leaves this process's own path reading `(deleted)`.
    origin: lightforge_core::upgrade::Origin,
    /// The binary this process started from, for a restart.
    exe: Option<std::path::PathBuf>,
    /// The update of Lightforge itself, once "Update" was pressed.
    upgrade: Option<Upgrade>,
    launcher_check: LauncherCheck,
    diagnostics: Diagnostics,
    /// A cancelled transfer whose files go once `Downloaded` says the run stopped.
    delete_after_cancel: Vec<ProductCode>,
    /// A given-up download's files being removed right now.
    discarding: Option<ProductCode>,
    /// The product being launched, from pre-flight until its game is seen
    /// or the launch ends without one.
    launching: Option<ProductCode>,
    /// Games of ours that are up, at most one per product: its tile's PLAY
    /// is STOP until it exits. Another product can still be launched.
    playing: Vec<Session>,
    stop_dialog: Option<StopDialog>,
    version_dialog: Option<VersionDialog>,
    modify_dialog: Option<ModifyDialog>,
    /// A launch whose pre-flight is done, held until the download it paused
    /// has finished winding down: the client and a still-checkpointing
    /// downloader would share one store.
    launch_when_quiet: Option<Launch>,
    report_dialog: Option<ReportDialog>,
    /// Filled in once, shortly after startup. `None` until then, and a report
    /// sent in that window simply says so instead of blocking on it.
    system: Option<iced::system::Information>,
    downgrade_dialog: Option<DowngradeDialog>,
    /// Builds the user agreed to launch under despite a downgrade, this
    /// session only.
    downgrade_ok: HashSet<String>,
    /// Partitions that could become a library, scanned when Storage opens.
    partitions: Vec<library::Partition>,
    /// What clearing one drive last freed, by library id.
    drive_notice: Option<(String, String)>,
    /// Each library's unused game data, as last checked, by library id.
    unused: HashMap<String, UnusedData>,
    /// Whether nothing is installed yet, cached like `drive_states`.
    library_empty: bool,
    /// Each library's state, by id: cached, since `action()` runs every frame.
    drive_states: HashMap<String, library::State>,

    /// Optional performance tools this machine has, detected at startup.
    tools: HostTools,
    /// The environment-variable editors: Game Settings' and Battle.net's.
    env_editor: iced::widget::text_editor::Content,
    bnet_env_editor: iced::widget::text_editor::Content,
    /// Counts env edits, so only the save scheduled by the last one runs.
    env_edits: u64,
    /// An env edit is in the config but not yet on disk.
    env_unsaved: bool,
    /// Editable copies of the selected product's numeric tweaks, and the two
    /// fields of the environment escape hatch.

    addons: addonwork::Listing,
    token_input: String,
    /// Narrows the installed list; survives a refresh.
    addon_filter: String,
    /// The addon whose Remove has been clicked once. Removing deletes files,
    /// so the second click is the one that means it.
    addon_remove_armed: Option<(String, String)>,
    /// The addon whose per-addon menu is showing. One at a time: the panel is
    /// an overlay, and two of them would draw over each other.
    addon_menu_open: Option<(String, String)>,
    /// Whether the next import reproduces the token's exact versions.
    import_pinned: bool,
    /// The version picker, when it is open: which addon, and the builds the
    /// catalogue offers. `None` in the list means the fetch is still out.
    version_picker: Option<VersionPicker>,
    game_settings: Option<GameSettings>,
    addon_detail: Option<AddonDetail>,
    /// Game families from the client's cache, read at startup. Empty until
    /// the client has run; the hand-off then uses compiled-in families.
    families: lightforge_core::families::Families,
    /// Products whose unattended addon pass ran this session: once each.
    addon_auto_done: Vec<ProductCode>,
    /// An unattended addon pass is in flight; unlike `addons_busy`, it greys nothing.
    addon_auto_running: bool,
    /// What the unattended pass did, said once on the Addons page. `None`
    /// when it has not run or had nothing to do, "0 addons updated" is noise.
    addon_auto_note: Option<String>,
    /// A launch waiting for that pass: the game reads AddOns at login.
    launch_after_addons: Option<Launch>,
    /// Which tab the standing report belongs to: import, or update.
    addon_report_tab: AddonTab,
    /// Counts keystrokes in the search box, so a debounced search runs only
    /// for the last one.
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
        let mut app = Self::load_state();
        let task = app.startup_tasks();
        (app, task)
    }

    /// Every message; a change of the status line is written to the log here,
    /// once for every place that sets it.
    fn update(&mut self, message: Message) -> Task<Message> {
        let before = (self.status.clone(), self.error.clone());
        // Shortly after a Settings jump, any message may change a section's
        // height, so each one re-measures.
        let settling = self.screen == Screen::Settings
            && self.settings_anchor.as_ref().is_some_and(|a| a.since.elapsed() < SETTINGS_JUMP_SETTLE)
            && !matches!(message, Message::SettingsMeasured(_) | Message::SettingsScrolled(_));
        let task = self.step(message);
        let task = match settling {
            true => Task::batch([task, measure_settings()]),
            false => task,
        };

        if self.status != before.0 && !self.status.is_empty() {
            tracing::info!("status: {}", self.status);
            self.remember(self.status.clone());
        }
        if self.error != before.1 {
            if let Some(error) = self.error.clone() {
                // Also written by `Problem::of` when the error had a type;
                // this is the line for what the window actually showed.
                tracing::error!("shown to the user: {error}");
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
        Subscription::batch([
            Self::input_events(),
            self.pointer_events(),
            self.watches(),
            self.tick(),
        ])
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
        // The first run is a layer over the app: nothing else is reachable.
        if let Some(open) = &self.wizard {
            return focus::root(self.with_hints(self.wizard_layer(open)));
        }
        let screen = self.screen_stack();
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

/// Where the app menu's links point. `None` is a destination that does not
/// exist yet: the entry draws dimmed and unclickable until it is filled in.
const DISCORD_URL: Option<&str> = Some("https://discord.gg/vtTgmUV9Bq");
const X_URL: Option<&str> = Some("https://x.com/simplyKennybtw");
/// Where someone can say thanks. Offered, never asked for: these sit beside
/// the places to get help, not in front of anything.
const KOFI_URL: Option<&str> = Some("https://ko-fi.com/simplykenny");
const PAYPAL_URL: Option<&str> = Some("https://paypal.me/simplykenny");

#[cfg(test)]
mod tests;
