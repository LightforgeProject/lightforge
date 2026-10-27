//! The state the app keeps between messages: dialogs, transfers, Settings layout.

use crate::*;

/// The release-notes popup, while it is up.
#[derive(Debug, Clone)]
pub(crate) struct PatchNotes {
    pub(crate) release: lightforge_core::releases::Release,
    /// Whether the checkbox is ticked. Written to the config on dismissal
    /// rather than on each click, so a tick someone changes their mind about
    /// before closing the card costs nothing.
    pub(crate) mute: bool,
}

/// Where the "Check for updates" button in Settings stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum LauncherCheck {
    #[default]
    Idle,
    Asking,
    /// Asked, and this is the newest release.
    Newest,
    Failed(String),
}

/// An update of Lightforge itself, from "Update" on.
#[derive(Debug)]
pub(crate) struct Upgrade {
    pub(crate) stage: UpgradeStage,
    /// The download's progress while [`UpgradeStage::Fetching`].
    pub(crate) rx: Option<tokio::sync::watch::Receiver<(u64, Option<u64>)>>,
    pub(crate) sample: (u64, Option<u64>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UpgradeStage {
    /// Downloading and checking, and for the installer's copy installing.
    Fetching,
    /// A terminal runs the package manager. Over once the binary on disk is
    /// no longer the one this process started from, `stamp`.
    InTerminal { command: String, stamp: Option<crate::tasks::Stamp> },
    /// No terminal could be opened: the command, to run by hand.
    NoTerminal { command: String },
    /// The new version is on disk; this process is still the old one.
    Installed,
    Failed(Problem),
}

/// What the splash window is waiting on.
#[derive(Debug, Clone)]
pub(crate) struct Splash {
    /// How many products the prefetch was given, for the line under the mark.
    pub(crate) products: usize,
    /// When the window opened, so [`SPLASH_FLOOR`] can be honoured.
    pub(crate) opened: Instant,
    /// Whether the cache has been read and the splash knows if it waits for
    /// the network. Decided once, on the first cache-only pass.
    pub(crate) decided: bool,
}

/// What INSTALL opens before anything downloads: where it goes, what it
/// costs, and whether to include high-resolution textures.
#[derive(Debug, Clone)]
pub(crate) struct InstallDialog {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    pub(crate) stage: DialogStage,
    pub(crate) high_res: bool,
    /// Every drive this game could go on, main first.
    pub(crate) targets: Vec<InstallTarget>,
    /// Index into `targets`.
    pub(crate) chosen: usize,
}

impl InstallDialog {
    pub(crate) fn target(&self) -> Option<&InstallTarget> {
        self.targets.get(self.chosen)
    }
}

/// One drive offered by the install dialog.
#[derive(Debug, Clone)]
pub(crate) struct InstallTarget {
    pub(crate) id: String,
    pub(crate) label: String,
    /// Where the game would actually land.
    pub(crate) install: PathBuf,
    /// Space on that drive, or `None` when it could not be read.
    pub(crate) disk: Option<storage::Disk>,
    pub(crate) state: library::State,
    /// A WoW install is already there, so this version joins it and shares
    /// its data store rather than starting a second one.
    pub(crate) shares: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum DialogStage {
    /// Reading the build's download manifest.
    Calculating,
    Ready(Box<Offerings>),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportKind {
    /// Something is broken, so the diagnostic bundle goes with it.
    Bug,
    /// A note about the app. Nothing is wrong with the machine, so nothing
    /// about the machine is sent.
    Feedback,
}

/// How far a report has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReportStage {
    Writing,
    Sending,
    Sent,
    /// It did not leave the machine inside the flush window. Said plainly
    /// rather than swallowed: a dialog that claims "sent" when the server was
    /// unreachable is worse than no dialog.
    Failed,
    /// The same text went out a moment ago. Typing anything clears it.
    TooSoon,
}

/// How long the same report is refused for after it is sent.
///
/// Aimed at the person pressing Send again because nothing visibly happened,
/// not at an attacker, the DSN ships in the binary, so real abuse is a
/// server-side limit, not a client-side one.
pub(crate) const REPORT_COOLDOWN: Duration = Duration::from_secs(300);

/// The bug report / feedback dialog.
///
/// Not `Clone` or `Debug`, unlike its neighbours: a [`text_editor::Content`]
/// is neither, because it owns a laid-out buffer rather than a string.
pub(crate) struct ReportDialog {
    pub(crate) kind: ReportKind,
    /// What the user is typing. A multi-line editor rather than a field,
    /// "what were you doing" is a paragraph, and a one-line box that scrolls
    /// sideways invites a one-line answer.
    pub(crate) note: iced::widget::text_editor::Content,
    /// Picked before Send is offered; `None` until then.
    pub(crate) category: Option<report::Category>,
    /// An address or a Discord name, so there is a way to answer. Required
    /// on a bug report, since one nobody can be asked about usually stalls at
    /// the first question; optional on feedback. The only personal thing
    /// either report carries.
    pub(crate) contact: String,
    /// Facts worth filtering issues by, taken off the same bundle the card
    /// shows. Empty for feedback, which carries no diagnostics.
    pub(crate) tags: Vec<(String, String)>,
    /// A screenshot dropped on the dialog, if there is one.
    pub(crate) shot: Option<art::Shot>,
    /// The debug bundle, rendered when the dialog opened. That is where the
    /// four things [`DebugInfo::gather`] needs are in scope. Empty for
    /// feedback, which sends none of it. Already scrubbed of the account
    /// name: every rendered bundle is.
    pub(crate) base: String,
    /// `base` plus what the category attaches: what the card shows and what
    /// is sent, rebuilt whenever the category changes.
    pub(crate) debug: String,
    /// The recent status lines shown in `debug`, scrubbed, as they were when
    /// the dialog opened. What is sent, so the report carries exactly what
    /// the card showed and nothing logged while it was being written. Empty
    /// for feedback, like the bundle.
    pub(crate) history: Vec<String>,
    pub(crate) stage: ReportStage,
}

impl ReportDialog {
    /// Everything Send needs from what was typed and picked. The stage is the
    /// caller's to check: this is about the content, not the moment.
    pub(crate) fn can_send(&self) -> bool {
        let contact = match self.kind {
            ReportKind::Bug => !self.contact.trim().is_empty(),
            ReportKind::Feedback => true,
        };
        self.category.is_some() && contact && !self.note.text().trim().is_empty()
    }
}

/// The confirmation before a game is deleted.
#[derive(Debug, Clone)]
pub(crate) struct UninstallDialog {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    /// `None` while the plan is being worked out.
    pub(crate) plan: Option<Result<tasks::UninstallPreview, String>>,
    /// Deleting: the buttons are held until it answers.
    pub(crate) working: bool,
    /// Bytes the compaction has copied so far, written by the task.
    pub(crate) copied: Arc<std::sync::atomic::AtomicU64>,
}

/// One library's unused game data on Settings > Storage.
pub(crate) enum UnusedData {
    Checking,
    Ready(tasks::Unused),
    /// Freeing it: bytes copied so far, written by the task, and the total.
    Freeing { copied: Arc<std::sync::atomic::AtomicU64>, total: u64 },
    /// What the last check or free had to say: how much it freed, or why not.
    Said(String),
}

/// Below this the row is not shown: a button that frees a few megabytes is
/// noise on the page.
pub(crate) const UNUSED_WORTH_SHOWING: u64 = 100 * 1024 * 1024;

/// The confirmation before a download is given up.
///
/// Two different questions wearing one dialog. Giving up a *first install*
/// leaves a partial store nothing can read and nothing names, so it offers to
/// delete it and says how much that is. Giving up an *update* must not delete
/// anything: the game on disk is playable right now, and what has come down is
/// reused by the next attempt rather than fetched twice.
#[derive(Debug, Clone)]
pub(crate) struct CancelDialog {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    /// The version already installed, when this is an update. `None` makes it
    /// a first install, which is the only case where files are removed.
    pub(crate) installed: Option<String>,
}

/// The confirmation before a launch that would open the shared prefix with an
/// older Proton than the one that last built it.
///
/// Shown rather than refused. The pin is a choice the user made, an older
/// build is sometimes the only one a title runs under, and Lightforge is not
/// in a position to know that this particular pair breaks. What it can do is
/// say what is about to happen to the prefix every other game shares.
#[derive(Debug, Clone)]
pub(crate) struct DowngradeDialog {
    /// Held until it is answered, then started unchanged.
    pub(crate) what: Launch,
    /// The build the launch would run under.
    pub(crate) pick: String,
    /// The build that last created or upgraded the prefix.
    pub(crate) built_by: String,
    /// The game being started, for the sentence.
    pub(crate) label: String,
}

/// A tab of the "Install Proton Version" window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProtonCategory {
    Steam,
    GloriousEggroll,
    CachyOs,
}

impl ProtonCategory {
    pub(crate) const ALL: [Self; 3] = [Self::Steam, Self::GloriousEggroll, Self::CachyOs];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam Proton",
            Self::GloriousEggroll => "Proton-GE",
            Self::CachyOs => "Proton-CachyOS",
        }
    }

    /// The downloadable family behind the tab, if there is one.
    pub(crate) fn family(self) -> Option<proton::ProtonFamily> {
        match self {
            Self::GloriousEggroll => Some(proton::ProtonFamily::GloriousEggroll),
            Self::CachyOs => Some(proton::ProtonFamily::CachyOs),
            Self::Steam => None,
        }
    }
}

/// A section of the Settings page, and its entry in the index beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SettingsTab {
    General,
    Setup,
    Proton,
    BattleNet,
    Downloads,
    Addons,
    Storage,
    About,
}

impl SettingsTab {
    /// In page order: the switches people change most first, then the
    /// longer cards, then the ones visited once.
    pub(crate) const ALL: [Self; 8] = [
        Self::General,
        Self::Downloads,
        Self::Addons,
        Self::BattleNet,
        Self::Proton,
        Self::Storage,
        Self::Setup,
        Self::About,
    ];

    /// The id of the section's container on the page, which is how it is
    /// found to be jumped to.
    pub(crate) fn anchor(self) -> &'static str {
        match self {
            Self::General => "settings General",
            Self::Setup => "settings Setup",
            Self::Proton => "settings Proton",
            Self::BattleNet => "settings Battle.net",
            Self::Downloads => "settings Downloads",
            Self::Addons => "settings Addons",
            Self::Storage => "settings Storage",
            Self::About => "settings About",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Setup => "Setup",
            Self::Proton => "Proton",
            Self::BattleNet => "Battle.net",
            Self::Downloads => "Downloads",
            Self::Addons => "Addons",
            Self::Storage => "Storage",
            Self::About => "About",
        }
    }
}

/// Where the Settings sections sat when the page was last laid out.
///
/// In layout coordinates, which are the page's own: a scrolled page is laid
/// out once at its full height, and scrolling only moves where it is drawn.
/// So a section's `y` less `top` is the offset that brings it to the top of
/// the page, and its bounds compare directly with where [`focus`] recorded a
/// control.
#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsLayout {
    /// Where the page's content starts.
    pub(crate) top: f32,
    /// In page order, which is the order the measuring found them in.
    pub(crate) sections: Vec<(SettingsTab, iced::Rectangle)>,
}

impl SettingsLayout {
    pub(crate) fn bounds(&self, tab: SettingsTab) -> Option<iced::Rectangle> {
        self.sections.iter().find(|(t, _)| *t == tab).map(|(_, bounds)| *bounds)
    }

    /// How far down the page `tab` starts.
    pub(crate) fn offset(&self, tab: SettingsTab) -> Option<f32> {
        self.bounds(tab).map(|bounds| bounds.y - self.top)
    }

    /// The section someone looking at `viewport` is reading, given that the
    /// index marks `marked`.
    pub(crate) fn reading(&self, viewport: &scrollable::Viewport, marked: SettingsTab) -> Option<SettingsTab> {
        self.reading_at(
            viewport.absolute_offset().y,
            viewport.bounds().height,
            viewport.content_bounds().height,
            marked,
        )
    }

    /// [`SettingsLayout::reading`] for a page scrolled to `y`, showing
    /// `shown` of `height`.
    ///
    /// The section under a reading line near the top of the view. Over the
    /// last screenful of the page that line slides down to the bottom of the
    /// view, because the last few sections are too short to ever reach the
    /// top: a fixed line left the mark on Storage with Setup and About in
    /// full view. Sliding, each later heading takes the mark as it passes,
    /// and the very bottom marks the last section.
    ///
    /// At the very bottom a section the index already marks, Setup after a
    /// click, keeps its mark while its heading is on screen.
    pub(crate) fn reading_at(&self, y: f32, shown: f32, height: f32, marked: SettingsTab) -> Option<SettingsTab> {
        let max = (height - shown).max(0.0);
        if y >= max - 1.0 && self.offset(marked).is_some_and(|top| top >= y && top < y + shown) {
            return Some(marked);
        }
        let near_end = match shown > 0.0 {
            true => (1.0 - (max - y) / shown).clamp(0.0, 1.0),
            false => 0.0,
        };
        let line = y + theme::LG + near_end * (shown - 2.0 * theme::LG).max(0.0);
        self.sections
            .iter()
            .rev()
            .find(|(_, bounds)| bounds.y - self.top <= line)
            .or(self.sections.first())
            .map(|(tab, _)| *tab)
    }
}

/// A section a jump scrolled to, kept at the top while the page settles.
pub(crate) struct SettingsAnchor {
    pub(crate) tab: SettingsTab,
    /// The offset it was last scrolled to.
    pub(crate) y: f32,
    /// The one before, when the page grew and it had to move again.
    pub(crate) prior: Option<f32>,
    pub(crate) since: Instant,
}

/// Find where every Settings section sits on the page.
pub(crate) fn measure_settings() -> Task<Message> {
    use iced::advanced::widget::operation::{Outcome, Scrollable};
    use iced::advanced::widget::{Id, Operation};

    struct Measure(SettingsLayout);

    impl Operation<SettingsLayout> for Measure {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<SettingsLayout>)) {
            operate(self);
        }

        fn container(&mut self, id: Option<&Id>, bounds: iced::Rectangle) {
            let Some(id) = id else { return };
            if let Some(tab) = SettingsTab::ALL.into_iter().find(|tab| *id == Id::new(tab.anchor())) {
                self.0.sections.push((tab, bounds));
            }
        }

        fn scrollable(
            &mut self,
            id: Option<&Id>,
            _bounds: iced::Rectangle,
            content_bounds: iced::Rectangle,
            _translation: iced::Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&Id::new(SETTINGS_PAGE)) {
                self.0.top = content_bounds.y;
            }
        }

        fn finish(&self) -> Outcome<SettingsLayout> {
            Outcome::Some(self.0.clone())
        }
    }

    iced::advanced::widget::operate(Measure(SettingsLayout::default())).map(Message::SettingsMeasured)
}

/// Where a product stands on [`proton::SHOP_BUILD`].
///
/// Three states rather than a boolean: a hand-off title has no WoW shop to
/// care about, and saying nothing is different from saying no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShopBuild {
    /// Not a WoW product. Nothing to recommend.
    NotWow,
    /// Already on it.
    Running,
    /// A WoW product pinned somewhere else, or following a default that is.
    Elsewhere,
}

/// Which half of the addon screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddonTab {
    /// What is installed, and what has updates.
    Mine,
    /// Search every provider and install.
    Browse,
    /// Bring a collection over from WoWUp in one paste.
    Import,
    /// Hand this collection to another machine, or to WoWUp.
    Export,
}

/// Agent's registry, watched while Battle.net may be writing to it.
///
/// The registry turns "Install via Battle.net" into "Play"; a game installed
/// before Lightforge only lands there once the client has run, after "Open
/// Battle.net" has already returned. So its timestamp is checked every
/// [`REGISTRY_POLL`] and it is re-read when it moves. Stops once the client
/// has come and gone, or after [`REGISTRY_GRACE`] if it never appears.
#[derive(Debug, Clone)]
pub(crate) struct RegistryWatch {
    pub(crate) since: Instant,
    pub(crate) stamp: Option<std::time::SystemTime>,
    pub(crate) seen_client: bool,
    pub(crate) polls: u32,
}

/// How often the registry's timestamp is checked while it is watched.
pub(crate) const REGISTRY_POLL: Duration = Duration::from_secs(2);

/// Every how many polls the process table is scanned for the client.
pub(crate) const REGISTRY_CLIENT_EVERY: u32 = 5;

/// How long a watch waits for a client that never shows up.
pub(crate) const REGISTRY_GRACE: Duration = Duration::from_secs(120);

impl RegistryWatch {
    pub(crate) fn new() -> Self {
        Self { since: Instant::now(), stamp: None, seen_client: false, polls: 0 }
    }
}

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Screen {
    Dashboard,
    /// Lightforge's own release notes.
    Updates,
    /// Setup lives here too, as the first card.
    Settings,
    Addons,
}

/// The parts of a finished run the UI actually shows. `install::Summary` is not
/// `Clone`, and a message has to be.
#[derive(Debug, Clone)]
pub(crate) struct Complete {
    pub(crate) complete: bool,
    pub(crate) cancelled: bool,
    pub(crate) files: usize,
    pub(crate) version: Option<String>,
    /// Why a finished run is not complete, see `Summary::reasons`.
    pub(crate) reasons: Vec<String>,
}

/// Where one product's transfer has got to.
///
/// Keyed to a product rather than held app-wide, which is the whole point:
/// the old single `Option<Transfer>` meant every product you switched to
/// inherited whichever one happened to be running.
pub(crate) enum Stage {
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
pub(crate) struct Download {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    pub(crate) stage: Stage,
    /// Newest sample off the channel, kept after a pause so the manager can
    /// still say how far it got.
    pub(crate) sample: TransferState,
    pub(crate) rate: Rate,
    pub(crate) job: Job,
}

/// What one transfer set out to fetch, carried across its pauses.
///
/// The bar measures the job, not the product: an update that changes 10 GB of
/// a 90 GB install reads "0 of 10 GB", not "80 of 90 GB". A resumed run plans
/// only what is still missing, so the job's size is fixed by the *first* plan
/// and each later run is placed within it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Job {
    /// Bytes the first run planned. `None` until one has planned.
    pub(crate) total: Option<u64>,
    /// Bytes done when the current run started, shown while a resumed run
    /// is still planning, so the bar holds rather than dropping to zero.
    pub(crate) reached: u64,
}

impl Download {
    pub(crate) fn progress(&self) -> state::Progress {
        job_progress(self.job, &self.sample, self.rate.per_second)
    }

    /// This entry as a resumed job: the size its first run planned, and how
    /// far it got before the pause.
    pub(crate) fn resumed(self) -> Job {
        let reached = match self.stage {
            Stage::Paused(p) => p.downloaded_bytes,
            _ => 0,
        };
        Job { reached, ..self.job }
    }

    pub(crate) fn transfer(&self) -> state::Transfer {
        match &self.stage {
            Stage::Queued => state::Transfer::Queued,
            Stage::Running { .. } => state::Transfer::Running(self.progress()),
            Stage::Paused(progress) => state::Transfer::Paused(*progress),
        }
    }
}

/// Which half of the detail view is on show.
///
/// Changelog first, deliberately: somebody who opens this from an installed
/// addon is asking "what would change if I updated", and somebody browsing
/// gets the description on the other tab one click away. The default is set
/// at the call site for that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailTab {
    Changelog,
    About,
}

/// One addon's project page, while it is open.
pub(crate) struct AddonDetail {
    pub(crate) provider: String,
    pub(crate) id: String,
    /// What to call it before the project answers. The search row and the
    /// ledger both already know the name, and a card headed "…" while a
    /// request is out reads like a failure.
    pub(crate) name: String,
    /// `None` while the catalogue is still answering.
    pub(crate) loaded: Option<addonwork::Detail>,
    pub(crate) problem: Option<String>,
    pub(crate) tab: DetailTab,
}

/// What a backup row is asking to confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupAction {
    Restore,
    Delete,
}

/// What [`Lightforge::can_maintain`] allows right now.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Maintain {
    pub(crate) scan: bool,
    pub(crate) deep_scan: bool,
    pub(crate) uninstall: bool,
    pub(crate) repair: bool,
}

/// What the cog's "Game Version" shows.
#[derive(Debug, Clone)]
pub(crate) struct VersionDialog {
    pub(crate) label: String,
    pub(crate) version: String,
    pub(crate) location: String,
}

/// The cog's "Modify Install": the game's language, before it is changed.
#[derive(Debug, Clone)]
pub(crate) struct ModifyDialog {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    /// What the game is installed in now.
    pub(crate) current: String,
    pub(crate) locale: String,
}

/// The question asked before a running game is stopped.
#[derive(Debug, Clone)]
pub(crate) struct StopDialog {
    pub(crate) label: String,
}

/// The Game Settings popup, for the game that was selected when it opened.
#[derive(Debug, Clone)]
pub(crate) struct GameSettings {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
    /// `None` until the first listing answers.
    pub(crate) backups: Option<Vec<lightforge_core::backup::Backup>>,
    /// A backup or restore is running; every button waits for it.
    pub(crate) working: bool,
    /// The backup whose Restore or Delete was pressed, waiting for its
    /// confirmation, and which of the two it was.
    pub(crate) confirm: Option<(PathBuf, BackupAction)>,
    /// The last outcome: a sentence, or what went wrong.
    pub(crate) note: Option<Result<String, String>>,
    /// What the texture pack weighs, `None` until the manifest has said or
    /// when this build has none behind the option.
    pub(crate) textures_bytes: Option<u64>,
    /// Where deleting the texture pack stands.
    pub(crate) textures: TexturesStep,
}

/// The question asked when the high-resolution textures are switched off.
#[derive(Debug, Clone, Default)]
pub(crate) enum TexturesStep {
    #[default]
    Idle,
    /// Working out what can be deleted.
    Planning,
    /// Waiting for the answer to "delete them too?".
    Ask(Box<lightforge_core::tact::reclaim::Reclaim>),
    /// Deleting, which rewrites archives and takes a while.
    Removing,
}

/// One addon's builds, while the user chooses between them.
#[derive(Debug, Clone)]
pub(crate) struct VersionPicker {
    pub(crate) provider: String,
    pub(crate) id: String,
    pub(crate) name: String,
    /// The build installed now, so the list can say which one that is.
    pub(crate) installed: Option<String>,
    /// `None` while the catalogue is still answering.
    pub(crate) builds: Option<Vec<lightforge_core::addons::catalog::FileInfo>>,
    pub(crate) problem: Option<String>,
}

/// What the hand-off panel's one button does, which depends on how much is
/// known about the product.
///
/// Three states rather than a pair of booleans at the call site: the label,
/// the colour and the message have to agree, and they only do that reliably
/// when one value decides all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffAct {
    /// Installed, and the client knows which tile it is: start it.
    Play,
    /// Known tile, nothing installed: land on it so Install can be pressed.
    Install,
    /// No game family, so nothing can be named. Opens the client and stops.
    OpenClient,
}

impl HandoffAct {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Play => "Play via Battle.net",
            Self::Install => "Install via Battle.net",
            Self::OpenClient => "Open Battle.net",
        }
    }

    /// Blue means ready to play, and only that.
    ///
    /// Installing is a filled button too. It is still the thing to press on
    /// this panel, but a neutral one, so the blue is never spent on a game
    /// that cannot be played yet. `None` draws outlined: opening the client
    /// is not an act on this game at all, so it does not dress as one.
    pub(crate) fn colour(self) -> Option<iced::Color> {
        match self {
            Self::Play => Some(theme::ACCENT),
            Self::Install => Some(theme::PAUSE),
            Self::OpenClient => None,
        }
    }

    /// What the button will really do, for the hints mode.
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Play => {
                "Lightforge starts Battle.net, waits for it to sign in, tells it to launch \
                 this game, and holds here until the game exits. Battle.net installs and \
                 updates it; Lightforge never touches its files."
            }
            Self::Install => {
                "Lightforge opens Battle.net's install dialog for this game, where you \
                 choose the location and start it. The install is Battle.net's own \
                 — this panel shows Play once its registry lists the game."
            }
            Self::OpenClient => {
                "No game family is known for this product, so the client cannot be told \
                 which tile to open — naming the wrong one lands on somebody else's game. \
                 This opens Battle.net and stops there."
            }
        }
    }

    pub(crate) fn message(self) -> Message {
        match self {
            Self::Play => Message::BattleNet,
            Self::Install => Message::InstallViaBattleNet,
            Self::OpenClient => Message::OpenBattleNet,
        }
    }
}

/// What a pre-flight pass is clearing the way for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Launch {
    /// Straight to the game executable.
    Game,
    /// Hand off to Battle.net, which starts the game itself.
    ///
    /// Not for the shop's sake. That was the old assumption, disproven on
    /// 2026-10-02: the shop is the game's own CEF and works from a direct
    /// launch. This is for a game only Battle.net can install and update.
    BattleNet,
    /// Open the Battle.net client itself, for everything outside the WoW
    /// family. A pass-through, not a managed install; see `SPEC.md`.
    Client,
    /// Put the client on a game's tile so its Install button can be pressed.
    ///
    /// The same hand-off as [`Launch::BattleNet`] and deliberately a separate
    /// intent: an install needs a visible window and a person, where a launch
    /// needs neither and waits for a game that an install will not produce
    /// for an hour.
    Install,
}

/// A tile held down in the band.
///
/// `origin` is unknown at the press, which carries no position, and is taken
/// from the first movement after it. `last` is where the cursor was on the
/// previous movement, so a move can tell which way it is heading.
pub(crate) struct TileDrag {
    pub(crate) line: ProductLine,
    pub(crate) origin: Option<f32>,
    pub(crate) last: f32,
    pub(crate) moving: bool,
}

/// Transfer rate as a human should read it: an average with a real time
/// constant, published at a pace the eye can follow.
///
/// A raw delta between two samples swings wildly when a large file lands in one
/// tick, so the rate is averaged. Measuring often keeps the ETA honest, but a
/// number that changes three times a second reads as flicker however well it is
/// averaged, so what the view shows is held between refreshes.
#[derive(Debug, Default)]
pub(crate) struct Rate {
    pub(crate) last_bytes: u64,
    pub(crate) last_at: Option<Instant>,
    /// The average, moved by every sample.
    pub(crate) average: f64,
    /// What the view shows: the average as it stood at `shown_at`.
    pub(crate) per_second: u64,
    pub(crate) shown_at: Option<Instant>,
}

impl Rate {
    /// How long the average takes to cover about two thirds of a step change.
    pub(crate) const TAU: f64 = 2.0;
    /// How long a shown value stands before the average replaces it.
    pub(crate) const HOLD: Duration = Duration::from_secs(1);
    /// Shorter windows are mostly chunk-arrival noise, not a rate.
    pub(crate) const MIN_WINDOW: f64 = 0.25;

    pub(crate) fn sample(&mut self, bytes: u64) {
        let now = Instant::now();
        let Some(previous) = self.last_at else {
            self.last_at = Some(now);
            self.last_bytes = bytes;
            return;
        };
        let elapsed = now.duration_since(previous).as_secs_f64();
        if elapsed < Self::MIN_WINDOW {
            return;
        }

        let instant = bytes.saturating_sub(self.last_bytes) as f64 / elapsed;
        // Weighting by the time that passed rather than by the tick keeps the
        // smoothing the same whether ticks arrive on time, late or in a burst.
        self.average = match self.shown_at {
            None => instant,
            Some(_) => {
                let alpha = 1.0 - (-elapsed / Self::TAU).exp();
                self.average + (instant - self.average) * alpha
            }
        };
        self.last_bytes = bytes;
        self.last_at = Some(now);

        // The first real sample is shown at once -- an empty readout while the
        // average warms up would look stalled.
        if self.shown_at.is_none_or(|at| now.duration_since(at) >= Self::HOLD) {
            self.per_second = self.average as u64;
            self.shown_at = Some(now);
        }
    }
}
