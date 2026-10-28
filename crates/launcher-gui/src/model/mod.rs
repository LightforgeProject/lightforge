//! The state the app keeps between messages: dialogs, transfers, Settings layout.

use crate::*;

mod dialogs;
mod settings;
mod transfers;

pub(crate) use dialogs::*;
pub(crate) use settings::*;
pub(crate) use transfers::*;

/// The release-notes popup, while it is up.
#[derive(Debug, Clone)]
pub(crate) struct PatchNotes {
    pub(crate) release: lightforge_core::releases::Release,
    /// Whether the checkbox is ticked; saved on dismissal.
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

/// The Diagnostics block in Settings, About.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Diagnostics {
    #[default]
    Idle,
    Running,
    Done(Vec<lightforge_core::doctor::Check>),
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

/// Where a product stands on [`proton::SHOP_BUILD`].
///
/// Three states: a hand-off title has no shop at all.
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
/// A found game lands there only after the client runs. Checked every
/// [`REGISTRY_POLL`] until the client has come and gone, or [`REGISTRY_GRACE`].
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

/// What the hand-off panel's one button does, which depends on how much is
/// known about the product.
///
/// One value decides label, colour and message.
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
    /// Installing is filled but neutral; `None` (just open the client) is outlined.
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
    /// For games only Battle.net installs; the shop works from a direct launch.
    BattleNet,
    /// Open the Battle.net client itself, for everything outside the WoW
    /// family. A pass-through, not a managed install; see `SPEC.md`.
    Client,
    /// Put the client on a game's tile so its Install button can be pressed.
    ///
    /// Separate from [`Launch::BattleNet`]: an install needs a visible window
    /// and waits for no game.
    Install,
}

/// A tile held down in the band.
///
/// `origin` comes from the first movement (a press has no position).
pub(crate) struct TileDrag {
    pub(crate) line: ProductLine,
    pub(crate) origin: Option<f32>,
    pub(crate) last: f32,
    pub(crate) moving: bool,
}
