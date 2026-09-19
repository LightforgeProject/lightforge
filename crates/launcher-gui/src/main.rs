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

mod tasks;
mod theme;

use std::time::{Duration, Instant};

use iced::widget::{
    button, column, container, image, pick_list, progress_bar, row, stack, text, Space,
};
use iced::{Alignment, ContentFit, Element, Length, Subscription, Task};

use lightforge_core::assets::{Branding, Pipeline, Store};
use lightforge_core::config::Config;
use lightforge_core::debug::DebugInfo;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::ProductCode;
use lightforge_core::state::{self, ActionState, Local};
use lightforge_core::tact::download::Cancel;
use lightforge_core::tact::version::Region;

use tasks::{Refreshed, SharedDownloads, Transfer, TransferState};

/// Where a player manages their subscription, per the spec.
const ACCOUNT_URL: &str = "https://account.battle.net/games";

fn main() -> iced::Result {
    iced::application("Lightforge", Lightforge::update, Lightforge::view)
        .subscription(Lightforge::subscription)
        .theme(|_| iced::Theme::Dark)
        .window_size((980.0, 660.0))
        // Closing is handled by the app, so a transfer can checkpoint first.
        .exit_on_close_request(false)
        .run_with(|| {
            let (app, task) = Lightforge::new();
            (app, task)
        })
}

#[derive(Debug, Clone)]
enum Message {
    Selected(Choice),
    Refreshed(ProductCode, Box<Refreshed>),
    Themed(ProductCode, Option<Branding>),
    Primary,
    Tick,
    Downloaded(ProductCode, Result<Complete, String>),
    CloseRequested(iced::window::Id),
    Launched(Result<String, String>),
    BattleNet,
    ManageSubscription,
    OpenFiles,
    CopyDebug,
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

struct Lightforge {
    layout: Layout,
    region: Region,
    pipeline: Option<Pipeline>,
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
}

impl Lightforge {
    fn new() -> (Self, Task<Message>) {
        let layout = Layout::default_for_user().unwrap_or_else(|_| Layout::with_root("."));
        let dirs = AppDirs::discover().ok();
        let config = dirs
            .as_ref()
            .and_then(|d| Config::load_or_default(&d.config_file()).ok())
            .unwrap_or_else(Config::with_default_products);

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

        let app = Self {
            layout,
            region: Region::default(),
            pipeline,
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
        };
        let task = app.reload(selected.code);
        (app, task)
    }

    /// Re-read the disk, the version service, and the theme for one product.
    fn reload(&self, code: ProductCode) -> Task<Message> {
        Task::batch([
            Task::perform(
                tasks::refresh(self.layout.clone(), code.clone(), self.region),
                {
                    let code = code.clone();
                    move |r| Message::Refreshed(code.clone(), Box::new(r))
                },
            ),
            Task::perform(tasks::theme(self.pipeline.clone(), code.clone()), {
                move |b| Message::Themed(code.clone(), b)
            }),
        ])
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
            Message::Selected(choice) => {
                if choice == self.selected {
                    return Task::none();
                }
                self.selected = choice.clone();
                self.local = Local::default();
                self.remote = None;
                self.branding = None;
                self.status = "checking…".to_owned();
                self.reload(choice.code)
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

            Message::Themed(code, branding) => {
                if code == self.selected.code {
                    self.branding = branding;
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
                ActionState::Play => {
                    self.busy = true;
                    self.status = "pre-flight…".to_owned();
                    Task::perform(
                        tasks::play(
                            self.layout.clone(),
                            self.selected.code.clone(),
                            self.downloads.clone(),
                        ),
                        Message::Launched,
                    )
                }
                ActionState::Unknown => Task::none(),
            },

            Message::Tick => {
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
                match self.downloads.pending().contains(&self.selected.code) {
                    true => self.start_download(),
                    false => self.reload(self.selected.code.clone()),
                }
            }

            Message::BattleNet => {
                self.busy = true;
                self.status = "handing off to Battle.net…".to_owned();
                Task::perform(
                    tasks::launch_via_battle_net(
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.downloads.clone(),
                    ),
                    Message::Launched,
                )
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

    fn subscription(&self) -> Subscription<Message> {
        // Only while something is moving: an idle dashboard should wake up for
        // nothing.
        let closing = iced::window::close_requests().map(Message::CloseRequested);
        match self.transfer {
            Some(_) => Subscription::batch([
                closing,
                iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            ]),
            None => closing,
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let action = self.action();

        let header = row![
            text("LIGHTFORGE").size(22),
            Space::with_width(Length::Fill),
            pick_list(
                self.choices.as_slice(),
                Some(self.selected.clone()),
                Message::Selected
            )
            .padding(8),
        ]
        .align_y(Alignment::Center);

        let logo: Element<Message> = match self.branding.as_ref() {
            Some(b) => image(image::Handle::from_path(&b.logo.path))
                .content_fit(ContentFit::Contain)
                .height(Length::Fixed(180.0))
                .into(),
            None => text(self.selected.label.clone()).size(44).into(),
        };

        let content = column![
            header,
            Space::with_height(Length::Fill),
            container(logo).center_x(Length::Fill),
            Space::with_height(Length::Fixed(16.0)),
            self.panel(&action),
        ]
        .spacing(12)
        .padding(24);

        match self.branding.as_ref() {
            Some(b) => stack![
                image(image::Handle::from_path(&b.background.path))
                    .content_fit(ContentFit::Cover)
                    .width(Length::Fill)
                    .height(Length::Fill),
                content,
            ]
            .into(),
            None => content.into(),
        }
    }

    /// The action panel: status, progress, and every button.
    fn panel(&self, action: &ActionState) -> Element<'_, Message> {
        let color = match action {
            ActionState::Install => theme::INSTALL,
            ActionState::Update { .. } => theme::UPDATE,
            ActionState::Play => theme::PLAY,
            ActionState::Downloading(_) | ActionState::Paused(_) => theme::PAUSE,
            ActionState::Error { .. } => theme::ERROR,
            ActionState::Unknown => theme::PAUSE,
        };

        let mut panel = column![].spacing(10);

        // Status line: the error if there is one, else what we last learned.
        panel = panel.push(match &self.error {
            Some(e) => text(format!("⚠ {e}")).size(13).color(theme::ERROR),
            None => text(self.status.clone()).size(13).color(theme::SUBTEXT),
        });

        if let Some(progress) = progress_of(action) {
            panel = panel.push(progress_bar(0.0..=1.0, progress.fraction()).height(10));
            panel = panel.push(
                text(format!(
                    "{}  ·  {}  ·  {}",
                    bytes_of(progress.downloaded_bytes, progress.total_bytes),
                    rate_of(progress.bytes_per_second),
                    eta_of(progress),
                ))
                .size(12)
                .color(theme::SUBTEXT),
            );
            if !self.sample.stage.is_empty() {
                panel = panel.push(text(self.sample.stage.clone()).size(12).color(theme::SUBTEXT));
            }
        }

        // Manage Subscription sits directly above the primary button, per spec:
        // subscription status is something a player checks per title.
        panel = panel.push(
            button(text("Manage Subscription").size(13))
                .on_press(Message::ManageSubscription)
                .style(theme::quiet)
                .padding([6, 12]),
        );

        let primary = button(text(action.label()).size(20))
            .style(theme::filled(color))
            .padding([14, 28])
            .width(Length::Fixed(260.0));
        panel = panel.push(match self.busy || matches!(action, ActionState::Unknown) {
            true => primary,
            false => primary.on_press(Message::Primary),
        });

        let battle_net = button(text("Launch via Battle.net").size(14))
            .style(theme::filled(theme::BATTLE_NET))
            .padding([8, 20])
            .width(Length::Fixed(260.0));
        panel = panel.push(match self.busy {
            true => battle_net,
            false => battle_net.on_press(Message::BattleNet),
        });
        panel = panel.push(
            text("Signs in through Battle.net so the in-game shop works.")
                .size(11)
                .color(theme::SUBTEXT),
        );

        panel = panel.push(
            row![
                button(text("Open Files").size(13))
                    .on_press(Message::OpenFiles)
                    .style(theme::quiet)
                    .padding([6, 12]),
                button(text("Copy Debug Info").size(13))
                    .on_press(Message::CopyDebug)
                    .style(theme::quiet)
                    .padding([6, 12]),
            ]
            .spacing(8),
        );

        container(panel)
            .padding(20)
            .style(|_| container::Style {
                background: Some(theme::PANEL.into()),
                border: iced::Border { radius: 10.0.into(), ..Default::default() },
                ..Default::default()
            })
            .into()
    }
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
