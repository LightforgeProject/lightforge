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
mod provision;
mod tasks;
mod theme;
mod trace;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{
    button, column, container, image, pick_list, progress_bar, row, scrollable,
    stack, text, text_input, tooltip, Space,
};
use iced::{Alignment, ContentFit, Element, Length, Subscription, Task};

use lightforge_core::assets::{Branding, Manifest, Pipeline, Store};
use lightforge_core::config::Config;
use lightforge_core::debug::DebugInfo;
use lightforge_core::net::Http;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::{Branch, Catalog, Product, ProductCode, ProductLine};
use lightforge_core::state::{self, ActionState, Local};
use lightforge_core::proton::ProtonPin;
use lightforge_core::setup::Status as SetupStatus;
use lightforge_core::tact::download::Cancel;
use lightforge_core::tact::version::Region;

use tasks::{Refreshed, SharedDownloads, Transfer, TransferState};

/// Where a player manages their subscription, per the spec.
const ACCOUNT_URL: &str = "https://account.battle.net/games";

/// Opening size.
const WINDOW: iced::Size = iced::Size::new(980.0, 660.0);

/// Floor for resizing.
///
/// Set by the header, which is the widest thing that cannot wrap: the title,
/// the product picker (whose longest entry is a user-supplied display name),
/// and four navigation buttons. Below roughly this width they start colliding,
/// and every screen under them is unusable well before the window stops
/// shrinking — so the window refuses rather than letting it happen.
const WINDOW_MIN: iced::Size = iced::Size::new(840.0, 560.0);

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
    /// Agent's registry was read: everything installed that isn't ours.
    HandoffsFound(Vec<tasks::Handoff>),
    /// A hand-off tile was clicked, by index into `handoffs`.
    HandoffSelected(usize),
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

    Show(Screen),
    SetupStatus(Option<Box<SetupStatus>>),
    RunSetup,
    SetupFinished(Result<Box<SetupStatus>, String>),
    PinDefaultProton(ProtonChoice),
    PinProductProton(ProtonChoice),
    AssetsSourceChanged(String),
    SaveSettings,
    CheckProton,
    ProtonChecked(Result<Option<String>, String>),

    AddonsListed(Box<addonwork::Listing>),
    TokenChanged(String),
    ImportToken,
    UpdateAddons,
    AddonsFinished(Result<Box<lightforge_core::addons::Report>, String>),
    RemoveAddon(String, String),
    AddonRemoved(Result<Vec<String>, String>),
    OpenUrl(String),

    AddonTab(AddonTab),
    SearchChanged(String),
    RunSearch,
    SearchDone(Box<addonwork::SearchOutcome>),
    InstallAddon(String, String),
    CheckAddonUpdates,
    AddonUpdatesChecked(Result<Vec<lightforge_core::addons::manage::UpdateStatus>, String>),
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
    Setup,
    Settings,
    Addons,
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
    icons: HashMap<ProductCode, std::path::PathBuf>,
    /// The branch last used in each line, so returning to a tile returns to
    /// the build you were on rather than snapping back to live.
    last_branch: HashMap<ProductLine, ProductCode>,
    /// Which products exist and how they group — built in, then whatever the
    /// manifest states over it.
    catalog: Catalog,
    /// Games Battle.net installed that Lightforge only hands off to.
    handoffs: Vec<tasks::Handoff>,
    /// Which hand-off tile is open, if any. `None` means a WoW product is.
    viewing_handoff: Option<usize>,
    choices: Vec<Choice>,
    selected: Choice,

    local: Local,
    remote: Option<lightforge_core::tact::version::Version>,
    branding: Option<Branding>,

    transfer: Option<Transfer>,
    /// Last sample read off the transfer's watch channel.
    sample: TransferState,
    rate: Rate,
    /// Progress kept after a pause, so the bar doesn't jump back to zero.
    paused_at: Option<state::Progress>,

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
    /// Editable copy of `assets_source`, committed on save.
    assets_input: String,
    proton_notice: Option<String>,

    addons: addonwork::Listing,
    token_input: String,
    addon_rx: Option<tokio::sync::watch::Receiver<addonwork::Progress>>,
    addon_sample: addonwork::Progress,
    addons_busy: bool,
    addon_report: Option<lightforge_core::addons::Report>,
    addon_tab: AddonTab,
    search_input: String,
    search: addonwork::SearchOutcome,
    searching: bool,
    updates: Vec<lightforge_core::addons::manage::UpdateStatus>,
}

impl Lightforge {
    fn new() -> (Self, Task<Message>) {
        let dirs = AppDirs::discover().ok();
        let config = dirs
            .as_ref()
            .and_then(|d| Config::load_or_default(&d.config_file()).ok())
            .unwrap_or_else(Config::with_default_products);
        // Honours `blizzard_root`, the same way the CLI does.
        let layout = config.layout().unwrap_or_else(|_| Layout::with_root("."));

        let choices: Vec<Choice> = config
            .products
            .iter()
            .map(|p| Choice { code: p.code.clone(), label: p.display_name.clone() })
            .collect();
        let selected = choices.first().cloned().unwrap_or(Choice {
            code: ProductCode::Retail,
            label: "World of Warcraft".to_owned(),
        });

        // Theming is optional: with no `assets_source` configured the dashboard
        // renders its own plain look rather than failing to start.
        let pipeline = match (config.asset_origin(), dirs.as_ref()) {
            (Some(origin), Some(dirs)) => {
                Some(Pipeline::new(origin, Store::new(dirs.asset_cache_dir())))
            }
            _ => None,
        };


        let assets_input = config.assets_source.clone().unwrap_or_default();
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
            handoffs: Vec::new(),
            viewing_handoff: None,
            choices,
            selected: selected.clone(),
            local: Local::default(),
            remote: None,
            branding: None,
            transfer: None,
            sample: TransferState::default(),
            rate: Rate::default(),
            paused_at: None,
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
            assets_input,
            proton_notice: None,
            addons: addonwork::Listing::default(),
            token_input: String::new(),
            addon_rx: None,
            addon_sample: addonwork::Progress::default(),
            addons_busy: false,
            addon_report: None,
            addon_tab: AddonTab::Mine,
            search_input: String::new(),
            search: addonwork::SearchOutcome::default(),
            searching: false,
            updates: Vec::new(),
        };

        // What is provisioned decides which screen opens: a machine that has
        // never run the CLI cannot install or launch anything, and a dashboard
        // whose only working button is an error is worse than saying so.
        let probe = Task::perform(
            provision::status(app.layout.clone(), app.config.clone()),
            |status| Message::SetupStatus(status.map(Box::new)),
        );
        let scan = Task::perform(
            tasks::handoffs(
                app.layout.clone(),
                app.dirs
                    .as_ref()
                    .map(|d| d.asset_cache_dir())
                    .unwrap_or_default(),
            ),
            Message::HandoffsFound,
        );
        let task = Task::batch([app.reload(selected.code), probe, scan]);
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

    /// Every configured product, in the order the bar draws them.
    fn product_codes(&self) -> Vec<ProductCode> {
        self.choices.iter().map(|c| c.code.clone()).collect()
    }

    /// Clear the way for a launch, then let [`Message::PreflightDone`] start it.
    fn preflight(&mut self, what: Launch) -> Task<Message> {
        self.busy = true;
        self.error = None;
        self.status = "pre-flight…".to_owned();
        Task::perform(
            tasks::preflight(
                self.layout.clone(),
                self.selected.code.clone(),
                self.downloads.clone(),
            ),
            move |result| Message::PreflightDone(what, result),
        )
    }

    /// The button state, which is core's decision and not the GUI's.
    fn action(&self) -> ActionState {
        let transfer = match (&self.transfer, self.paused_at) {
            (Some(_), _) => Some(state::Transfer::Running(self.live_progress())),
            (None, Some(progress)) => Some(state::Transfer::Paused(progress)),
            (None, None) => None,
        };
        state::decide(&self.local, self.remote.as_ref(), transfer)
    }

    /// Progress against the whole product, not against this run.
    ///
    /// A resumed run plans only what is missing, so run-relative numbers read
    /// as "starting over" every time a download is paused and picked up again.
    fn live_progress(&self) -> state::Progress {
        product_progress(&self.sample, self.rate.per_second)
    }

    fn start_download(&mut self) -> Task<Message> {
        let code = self.selected.code.clone();
        let cancel = Cancel::new();
        let (tx, rx) = tokio::sync::watch::channel(TransferState::default());

        self.downloads.register(code.clone(), cancel.clone());
        self.transfer = Some(Transfer { rx, cancel: cancel.clone() });
        self.paused_at = None;
        self.rate = Rate::default();
        self.sample = TransferState { stage: "preparing".to_owned(), ..TransferState::default() };
        self.error = None;

        Task::perform(
            tasks::download(self.layout.clone(), code.clone(), self.region, cancel, tx),
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

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LineSelected(line) => self.select_line(&line),

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
                ActionState::Install | ActionState::Update { .. } | ActionState::Error { .. } => {
                    self.start_download()
                }
                ActionState::Downloading(progress) => {
                    // Pause: trip the token. The loop stops between chunks and
                    // the store keeps everything already written.
                    if let Some(transfer) = &self.transfer {
                        transfer.cancel.cancel();
                    }
                    self.paused_at = Some(progress);
                    self.status = "pausing…".to_owned();
                    Task::none()
                }
                ActionState::Paused(_) => self.start_download(),
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
                let Some(transfer) = &self.transfer else {
                    return Task::none();
                };
                self.sample = transfer.rx.borrow().clone();
                // Rate is measured on what this run has stored; the baseline is
                // a constant offset and would only mute the first sample.
                self.rate.sample(self.sample.progress.bytes_stored);
                Task::none()
            }

            Message::CloseRequested(id) => {
                // Closing mid-transfer must not orphan what was fetched: trip
                // the token, let the run checkpoint its store, then close.
                match &self.transfer {
                    Some(transfer) => {
                        transfer.cancel.cancel();
                        self.closing = Some(id);
                        self.status = "finishing the download safely…".to_owned();
                        Task::none()
                    }
                    None => iced::window::close(id),
                }
            }

            Message::Downloaded(code, result) => {
                self.downloads.finished(&code);
                self.transfer = None;
                if let Some(id) = self.closing.take() {
                    return iced::window::close(id);
                }
                match result {
                    Ok(done) if done.complete => {
                        self.paused_at = None;
                        self.status = match done.version {
                            Some(v) => format!("installed {v}"),
                            None => format!("stored {} files", done.files),
                        };
                        self.reload(code)
                    }
                    Ok(done) if done.cancelled => {
                        self.status = "paused".to_owned();
                        Task::none()
                    }
                    Ok(_) => {
                        self.status = "stopped — run again to continue".to_owned();
                        self.paused_at = None;
                        self.reload(code)
                    }
                    Err(e) => {
                        self.paused_at = None;
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::Launched(result) => {
                self.busy = false;
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                // A launch may have paused a transfer; the guard records what
                // is owed, and resuming means planning afresh.
                // Opening Battle.net is how a new game appears, so look
                // again once the user is back.
                let rescan = Task::perform(
                    tasks::handoffs(
                        self.layout.clone(),
                        self.dirs.as_ref().map(|d| d.asset_cache_dir()).unwrap_or_default(),
                    ),
                    Message::HandoffsFound,
                );
                let next = match self.downloads.pending().contains(&self.selected.code) {
                    true => self.start_download(),
                    false => self.reload(self.selected.code.clone()),
                };
                Task::batch([next, rescan])
            }

            Message::BattleNet => self.preflight(Launch::BattleNet),

            Message::OpenBattleNet => self.preflight(Launch::Client),

            Message::HandoffsFound(found) => {
                // An index into a list that just changed means nothing.
                if found != self.handoffs {
                    self.viewing_handoff = None;
                }
                self.handoffs = found;
                Task::none()
            }

            Message::HandoffSelected(index) => {
                self.viewing_handoff = (index < self.handoffs.len()).then_some(index);
                self.error = None;
                Task::none()
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
                // Open on setup when the machine cannot launch anything yet,
                // but never yank the user out of a screen they chose.
                if let Some(status) = &status {
                    if !status.is_ready() && self.screen == Screen::Dashboard && !self.setup_running
                    {
                        self.screen = Screen::Setup;
                    }
                }
                self.setup = status;
                Task::none()
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

            Message::AssetsSourceChanged(value) => {
                self.assets_input = value;
                Task::none()
            }

            Message::SaveSettings => {
                let trimmed = self.assets_input.trim();
                self.config.assets_source = (!trimmed.is_empty()).then(|| trimmed.to_owned());
                self.save_config();

                // Theming comes from the source that was just saved, so rebuild
                // the pipeline and re-resolve rather than waiting for a restart.
                self.pipeline = match (self.config.asset_origin(), self.dirs.as_ref()) {
                    (Some(origin), Some(dirs)) => {
                        Some(Pipeline::new(origin, Store::new(dirs.asset_cache_dir())))
                    }
                    _ => None,
                };
                self.branding = None;
                // The cached manifest and its marks belong to the old source.
                self.manifest = None;
                self.icons.clear();
                self.catalog = Catalog::builtin();
                self.reload(self.selected.code.clone())
            }

            Message::CheckProton => {
                let installed = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.proton.as_ref())
                    .map(|p| p.version.clone());
                match installed {
                    Some(version) => {
                        self.proton_notice = Some("checking…".to_owned());
                        Task::perform(
                            provision::check_proton(self.http.clone(), version),
                            Message::ProtonChecked,
                        )
                    }
                    None => {
                        self.proton_notice = Some("no Proton build installed yet".to_owned());
                        Task::none()
                    }
                }
            }

            Message::ProtonChecked(result) => {
                self.proton_notice = Some(match result {
                    // Notify-only by policy: nothing updates itself.
                    Ok(Some(version)) => format!("{version} is available — install it from setup"),
                    Ok(None) => "up to date".to_owned(),
                    Err(e) => format!("check failed: {e}"),
                });
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
        }
    }

    /// Persist the config, surfacing a write failure rather than swallowing it.
    fn save_config(&mut self) {
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

        // Resize events are only interesting to a human reading a trace, and
        // subscribing to them makes every resize step rebuild the view, so
        // this stays out of a normal run entirely.
        let resizes = match trace::frames() {
            true => iced::window::resize_events().map(|(_, size)| Message::ResizeTraced(size)),
            false => Subscription::none(),
        };

        match self.transfer.is_some() || self.setup_running || self.addons_busy {
            true => Subscription::batch([
                closing,
                resizes,
                iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            ]),
            false => Subscription::batch([closing, resizes]),
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
        let screen = match self.screen {
            Screen::Dashboard => match self.viewing_handoff.and_then(|i| self.handoffs.get(i)) {
                Some(game) => self.handoff_screen(game),
                None => self.dashboard(),
            },
            Screen::Setup => self.setup_screen(),
            Screen::Settings => self.settings(),
            Screen::Addons => self.addons_screen(),
        };
        trace::view_built(started.elapsed());
        screen
    }

    /// Title, product picker, and navigation.
    fn header(&self) -> Element<'_, Message> {
        let nav = |label: &'static str, screen: Screen| {
            let entry = button(text(label).size(theme::LABEL)).padding([theme::SM, theme::MD]);
            match self.screen == screen {
                true => entry.style(theme::selected),
                false => entry.style(theme::ghost).on_press(Message::Show(screen)),
            }
        };

        let mut bar = row![text("LIGHTFORGE").size(theme::TITLE)]
            .align_y(Alignment::Center)
            .spacing(theme::SM);

        if self.screen == Screen::Dashboard {
            bar = bar.push(Space::with_width(Length::Fixed(theme::MD as f32)));
            for tile in self.product_bar() {
                bar = bar.push(tile);
            }
            // After a gap, and only after: these are games Lightforge shows
            // but does not manage, and the space says so before the tooltip
            // has to.
            if !self.handoffs.is_empty() {
                bar = bar.push(Space::with_width(Length::Fixed(theme::MD as f32)));
                for tile in self.handoff_bar() {
                    bar = bar.push(tile);
                }
            }
        }

        bar = bar.push(Space::with_width(Length::Fill));

        // App-level, not per-product: everything outside the WoW family is a
        // hand-off into Battle.net, so it sits with the navigation rather
        // than on a panel that describes one product.
        bar = bar.push(
            tooltip(
                button(text("Battle.net").size(theme::LABEL))
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
        bar = bar.push(nav("Addons", Screen::Addons));
        bar = bar.push(nav("Setup", Screen::Setup));
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
        self.catalog = match &self.manifest {
            Some(manifest) => manifest.catalog(),
            None => Catalog::builtin(),
        };
    }

    /// One tile per configured line, the selected one marked.
    ///
    /// A line with no icon in the manifest falls back to its label, so the bar
    /// is complete and usable before any art exists and fills in one line at a
    /// time as art is published.
    fn product_bar(&self) -> Vec<Element<'_, Message>> {
        let current = self.catalog.line_of(&self.selected.code);

        self.lines()
            .into_iter()
            .map(|line| {
                let selected = line == current;
                let head = line.head();

                // The line's mark is its live build's, since that is the
                // product the art was published against; a PTR is the same
                // game wearing the same face.
                let face: Element<Message> = match self.icons.get(&head) {
                    Some(path) => image(image::Handle::from_path(path))
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(theme::TILE))
                        .height(Length::Fixed(theme::TILE))
                        .into(),
                    None => container(text(line.label().to_owned()).size(theme::LABEL))
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
                    container(text(line.label().to_owned()).size(theme::LABEL))
                        .padding([theme::XS, theme::SM])
                        .style(theme::floating),
                    tooltip::Position::Bottom,
                )
                .into()
            })
            .collect()
    }

    /// One tile per game Battle.net installed that Lightforge doesn't manage.
    fn handoff_bar(&self) -> Vec<Element<'_, Message>> {
        self.handoffs
            .iter()
            .enumerate()
            .map(|(i, game)| {
                let face: Element<Message> = match &game.icon {
                    Some(path) => image(image::Handle::from_path(path))
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(theme::TILE))
                        .height(Length::Fixed(theme::TILE))
                        .into(),
                    None => container(text(game.label.clone()).size(theme::LABEL))
                        .center_x(Length::Shrink)
                        .center_y(Length::Fixed(theme::TILE))
                        .into(),
                };

                let tile = button(face).padding([theme::XS, theme::SM]);
                let tile = match self.viewing_handoff == Some(i) {
                    true => tile.style(theme::selected),
                    false => tile.style(theme::ghost).on_press(Message::HandoffSelected(i)),
                };

                tooltip(
                    tile,
                    container(
                        text(format!("{} — via Battle.net", game.label)).size(theme::LABEL),
                    )
                    .padding([theme::XS, theme::SM])
                    .style(theme::floating),
                    tooltip::Position::Bottom,
                )
                .into()
            })
            .collect()
    }

    /// The dashboard for a game Lightforge only hands off to.
    ///
    /// Deliberately bare. There is no version to compare, nothing to update
    /// and nothing to play from here — offering those controls greyed out
    /// would imply Lightforge could manage this game and merely won't.
    fn handoff_screen(&self, game: &tasks::Handoff) -> Element<'_, Message> {
        let mut facts = column![text(game.label.clone()).size(theme::DISPLAY)]
            .spacing(theme::XS)
            .align_x(Alignment::Center);
        if let Some(version) = &game.version {
            facts = facts.push(row![muted("installed".to_owned(), theme::LABEL), mono(version.clone())]
                .spacing(theme::SM)
                .align_y(Alignment::Center));
        }
        facts = facts.push(muted(
            "Installed through Battle.net. Lightforge opens it, nothing more.".to_owned(),
            theme::CAPTION,
        ));

        let mut middle = column![].spacing(theme::MD).align_x(Alignment::Center);
        if let Some(path) = &game.icon {
            middle = middle.push(
                image(image::Handle::from_path(path))
                    .content_fit(ContentFit::Contain)
                    .height(Length::Fixed(theme::LOGO)),
            );
        }
        middle = middle.push(facts);

        let open = button(
            container(text("Open via Battle.net").size(theme::BODY)).center_x(Length::Fill),
        )
        .padding([theme::MD, theme::LG])
        .width(Length::Fixed(240.0))
        .style(theme::outlined);

        column![
            self.header(),
            column![
                Space::with_height(Length::Fill),
                container(middle).center_x(Length::Fill),
                Space::with_height(Length::Fill),
                container(match self.busy {
                    true => open,
                    false => open.on_press(Message::OpenBattleNet),
                })
                .center_x(Length::Fill),
                match &self.error {
                    Some(e) => container(
                        text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR),
                    )
                    .center_x(Length::Fill),
                    None => container(Space::with_height(Length::Fixed(0.0))),
                },
            ]
            .spacing(theme::MD)
            .padding(theme::LG),
        ]
        .into()
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
        let leaving = self.viewing_handoff.take().is_some();
        if choice.code == self.selected.code && !leaving {
            return Task::none();
        }
        self.last_branch.insert(self.catalog.line_of(&choice.code), choice.code.clone());
        self.selected = choice.clone();
        self.local = Local::default();
        self.remote = None;
        self.branding = None;
        self.status = "checking…".to_owned();
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

        // The wordmark hangs under the app title in the top-left rather than
        // floating mid-page: the centre of the dashboard belongs to the art,
        // and a product logo reads as branding when it sits with Lightforge's
        // own rather than competing with the background for the middle.
        let logo: Element<Message> = match self.branding.as_ref() {
            Some(b) => image(image::Handle::from_path(&b.logo.path))
                .content_fit(ContentFit::Contain)
                .height(Length::Fixed(theme::logo_height(b.logo.height)))
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
                self.action_panel(&action),
            ]
            .spacing(theme::MD)
            .padding(theme::LG),
        ];

        match self.branding.as_ref() {
            Some(b) => stack![
                image(image::Handle::from_path(&b.background.path))
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
            ActionState::Downloading(_) | ActionState::Paused(_) => theme::PAUSE,
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
            if !self.sample.stage.is_empty() {
                left = left.push(muted(self.sample.stage.clone(), theme::CAPTION));
            }
        } else if let Some(version) = &self.local.version {
            left = left.push(
                row![muted("installed".to_owned(), theme::CAPTION), mono(version.clone())]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
            );
        }

        left = left.push(
            row![
                button(text("Manage Subscription").size(theme::LABEL))
                    .on_press(Message::ManageSubscription)
                    .style(theme::ghost)
                    .padding([theme::XS, theme::SM]),
                button(text("Open Files").size(theme::LABEL))
                    .on_press(Message::OpenFiles)
                    .style(theme::ghost)
                    .padding([theme::XS, theme::SM]),
                button(text("Copy Debug Info").size(theme::LABEL))
                    .on_press(Message::CopyDebug)
                    .style(theme::ghost)
                    .padding([theme::XS, theme::SM]),
            ]
            .spacing(theme::XS),
        );

        let primary = button(
            container(text(action.label()).size(theme::HEADING)).center_x(Length::Fill),
        )
        .style(theme::filled(color))
        .padding([theme::MD, theme::LG])
        .width(Length::Fixed(240.0));
        let battle_net = button(
            container(text("Launch via Battle.net").size(theme::LABEL)).center_x(Length::Fill),
        )
        .style(theme::outlined)
        .padding([theme::SM, theme::MD])
        .width(Length::Fixed(240.0));

        let right = column![
            match self.busy || matches!(action, ActionState::Unknown) {
                true => primary,
                false => primary.on_press(Message::Primary),
            },
            match self.busy {
                true => battle_net,
                false => battle_net.on_press(Message::BattleNet),
            },
            muted(
                "Signs in through Battle.net so the in-game shop works.".to_owned(),
                theme::CAPTION,
            ),
        ]
        .spacing(theme::SM)
        .align_x(Alignment::Center)
        .width(Length::Shrink);

        container(row![left, right].spacing(theme::XL).align_y(Alignment::Center))
            .padding(theme::LG)
            .style(theme::floating)
            .into()
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
                muted(
                    "WoWInterface, Tukui and GitHub (type owner/repo). CurseForge needs a catalog in Settings."
                        .to_owned(),
                    theme::CAPTION,
                ),
            ]
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
    fn setup_screen(&self) -> Element<'_, Message> {
        let mut body = column![].spacing(theme::SM);

        let step = |done: bool, label: &'static str, detail: String| {
            container(
                row![
                    container(text(if done { "✓" } else { "•" }).size(theme::BODY))
                        .padding([theme::XS, theme::SM])
                        .style(theme::badge(match done {
                            true => theme::PLAY,
                            false => theme::MUTED,
                        })),
                    column![text(label).size(theme::BODY), mono(detail)].spacing(theme::XS),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
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

        if let Some(e) = &self.error {
            body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
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

        self.page(
            "Set up Lightforge",
            Some(
                "Proton, the Wine prefix and Battle.net. Each step is skipped if it is already done."
                    .to_owned(),
            ),
            body.into(),
        )
    }

    /// Proton pinning, asset source, and the notify-only update check.
    fn settings(&self) -> Element<'_, Message> {
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

        fn field_row<'a>(
            label: String,
            control: Element<'a, Message>,
        ) -> iced::widget::Row<'a, Message> {
            row![muted(label, theme::LABEL).width(Length::Fixed(150.0)), control]
                .spacing(theme::MD)
                .align_y(Alignment::Center)
        }

        let proton = container(
            column![
                text("Proton").size(theme::HEADING),
                field_row(
                    "Default".to_owned(),
                    pick_list(
                        installed.clone(),
                        Some(ProtonChoice(
                            self.config.default_proton.version().map(str::to_owned)
                        )),
                        Message::PinDefaultProton,
                    )
                    .text_size(theme::BODY)
                    .padding(theme::SM)
                    .into(),
                ),
                field_row(
                    self.selected.label.clone(),
                    pick_list(installed, Some(ProtonChoice(product_pin)), Message::PinProductProton)
                        .text_size(theme::BODY)
                        .padding(theme::SM)
                        .into(),
                ),
                muted(
                    "A product with no pin of its own follows the default.".to_owned(),
                    theme::CAPTION,
                ),
                row![
                    secondary("Check for updates", Message::CheckProton).enabled(true),
                    muted(self.proton_notice.clone().unwrap_or_default(), theme::LABEL),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ]
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let theming = container(
            column![
                text("Theming").size(theme::HEADING),
                text_input(
                    "https://lightforge.tuxcore.pro/assets/manifest.json",
                    &self.assets_input
                )
                .on_input(Message::AssetsSourceChanged)
                .on_submit(Message::SaveSettings)
                .style(theme::field)
                .size(theme::BODY)
                .padding(theme::SM),
                muted(
                    "A URL, or a local directory to theme from. Empty means no theming."
                        .to_owned(),
                    theme::CAPTION,
                ),
                action_button("Save", theme::ACCENT, Message::SaveSettings).enabled(true),
            ]
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![proton, theming].spacing(theme::MD);
        if let Some(dirs) = &self.dirs {
            body = body.push(mono(format!("config: {}", dirs.config_file().display())));
        }
        if let Some(e) = &self.error {
            body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }

        self.page("Settings", None, body.into())
    }
}

/// Secondary text at a given size.
fn muted<'a>(content: String, size: u16) -> iced::widget::Text<'a> {
    text(content).size(size).color(theme::MUTED)
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

/// A separator between inline facts.
fn dot<'a>() -> Element<'a, Message> {
    text("·").size(theme::CAPTION).color(theme::MUTED).into()
}

/// Map a transfer sample onto the whole product.
fn product_progress(sample: &TransferState, bytes_per_second: u64) -> state::Progress {
    let (downloaded, total) = match sample.product_bytes {
        // Before the plan is known, run-relative is all there is.
        0 => (sample.progress.bytes_stored, sample.progress.bytes_total),
        product => (
            (sample.baseline + sample.progress.bytes_stored).min(product),
            product,
        ),
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

    #[test]
    fn a_resumed_run_shows_product_progress_not_run_progress() {
        // The symptom this guards: pause at 3 GiB, resume, and the bar reads
        // 0.00 GiB because the new plan excludes what is already on disk.
        let gib = 1024u64 * 1024 * 1024;
        let mut sample = TransferState {
            product_bytes: 75 * gib,
            baseline: 3 * gib,
            ..TransferState::default()
        };

        // Nothing stored yet this run, but 3 GiB are already there.
        let resumed = product_progress(&sample, 0);
        assert_eq!(resumed.downloaded_bytes, 3 * gib);
        assert_eq!(resumed.total_bytes, 75 * gib);
        assert!(resumed.fraction() > 0.0);

        // A run byte moves the product readout by one byte.
        sample.progress.bytes_stored = gib;
        assert_eq!(product_progress(&sample, 0).downloaded_bytes, 4 * gib);
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
        let p = product_progress(&sample, 0);
        assert_eq!((p.downloaded_bytes, p.total_bytes), (500, 1000));
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
