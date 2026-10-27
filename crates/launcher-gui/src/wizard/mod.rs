//! The first run.
//!
//! Everything a machine needs before a game can be played, in the order it has
//! to happen, as one flow instead of a Settings page the user has to find:
//! where things go, what is already here, provisioning, which games to keep in
//! the band, done.
//!
//! It is a layer over the app rather than a screen of its own, the same
//! [`Lightforge`] state, the same messages where they already exist. The Place
//! step *is* Settings' install-location field and the Storage page's drive
//! list, the Install step *is* [`crate::provision`], and the Games step is the
//! add-a-game list. Only the three things with nowhere else to live are new:
//! the artwork prefetch, the scan for installs that were here first, and the
//! choice to do without Battle.net.

mod adopt;
mod steps;
mod update;
mod view;
#[cfg(test)]
mod tests;

pub use update::update;
pub use view::view;

use std::path::PathBuf;

use iced::widget::{container, image, row, scrollable, text, text_input, Space};
use iced::{Alignment, ContentFit, Element, Length};

use lightforge_core::config::SteamOsUse;
use lightforge_core::error::chain;
use lightforge_core::discover::{self, Found, HandOff, Origin, WowInstall};
use lightforge_core::product::{Branch, Kind, ProductCode};
use lightforge_core::tact::install::LOCALES;
use lightforge_core::tact::version::Region;

use crate::{focus, theme};
use crate::{
    action_button, clickable, field_row, mono, muted, secondary, size_of, Lightforge, Message,
};


/// Which slide is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// What this is, the one thing the machine may be missing, and the two
    /// choices every later step depends on: region and language.
    Welcome,
    /// Where the prefix goes, and which drives games may go on.
    Place,
    /// Blizzard installs that were here before Lightforge.
    Found,
    /// Proton, the prefix and Battle.net.
    Install,
    /// Which games sit in the band.
    Games,
    Done,
}

impl Step {
    /// Every step, in order, the dots across the top count these.
    pub const ALL: [Step; 6] =
        [Step::Welcome, Step::Place, Step::Found, Step::Install, Step::Games, Step::Done];

    fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome to Lightforge",
            Self::Place => "Where things go",
            Self::Found => "Discover installed Battle.net games",
            Self::Install => "Setting up",
            Self::Games => "Your games",
            Self::Done => "Ready to play",
        }
    }

    fn next(self) -> Option<Step> {
        let at = Self::ALL.iter().position(|s| *s == self)?;
        Self::ALL.get(at + 1).copied()
    }

    fn back(self) -> Option<Step> {
        let at = Self::ALL.iter().position(|s| *s == self)?;
        Self::ALL.get(at.checked_sub(1)?).copied()
    }
}

/// One thing the scan turned up, as a row the user can tick.
#[derive(Debug, Clone)]
pub struct Row {
    /// The prefix it was found in. Empty for a folder the user pointed at.
    pub prefix: PathBuf,
    pub origin: Origin,
    pub kind: RowKind,
    pub take: bool,
    /// Why it cannot be taken, if it cannot.
    pub blocked: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RowKind {
    /// A WoW install Lightforge can adopt and then patch itself.
    Wow(WowInstall),
    /// A game only Battle.net can install, ours can be pointed at it.
    Game(HandOff),
}

impl Row {
    fn bytes(&self) -> u64 {
        match &self.kind {
            RowKind::Wow(install) => install.bytes,
            RowKind::Game(game) => game.bytes,
        }
    }
}

/// State for the duration of the first run, and nothing beyond it.
#[derive(Debug, Clone)]
pub struct Wizard {
    pub step: Step,
    /// Artwork fetched ahead of time, so the first visit to a game's panel is
    /// not two seconds of empty card. `None` until it finishes.
    pub prefetched: Option<usize>,
    pub scanning: bool,
    /// Whether the scan has run at all, an empty `rows` means something quite
    /// different before and after.
    pub scanned: bool,
    pub rows: Vec<Row>,
    /// A folder the user typed in themselves.
    pub pointed: String,
    pub point_error: Option<String>,
    /// The typed folder is being looked through. Off the UI thread: sizing
    /// every install found walks all of it.
    pub pointing: bool,
    /// Why the last add or remove on the drive step was refused. The step is a
    /// layer over the app, so `Lightforge::error` is not on screen here.
    pub drive_error: Option<String>,
    /// Whether Battle.net is wanted. Off means WoW only.
    pub battle_net: bool,
    /// What adoption did, one line each, shown on the way out of the step.
    pub adopted: Vec<Note>,
    /// Hand-offs to link once the prefix and Battle.net exist.
    pub pending: Vec<(HandOff, PathBuf)>,
    /// Opened from Settings to look for games after setup: the one slide,
    /// adopting in place, and closing without touching the first-run state.
    pub scan_only: bool,
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            step: Step::Welcome,
            prefetched: None,
            scanning: false,
            scanned: false,
            rows: Vec::new(),
            pointed: String::new(),
            point_error: None,
            pointing: false,
            drive_error: None,
            battle_net: true,
            adopted: Vec::new(),
            pending: Vec::new(),
            scan_only: false,
        }
    }
}

/// Everything the wizard can be told, beside the app messages it reuses.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Open it again from Settings.
    Open,
    /// Look for games installed since setup, from Settings.
    Scan,
    Next,
    Back,
    Region(Region),
    Locale(String),
    /// The artwork prefetch finished, with the number of products it cached.
    Prefetched(usize),
    Scanned(Vec<Found>),
    /// Tick or untick one found install.
    Take(usize, bool),
    PointInput(String),
    /// Inspect the folder that was typed.
    Point,
    /// What the typed folder turned out to hold.
    Pointed(Option<Found>),
    /// Ask the desktop for a folder to inspect.
    Browse,
    /// The folder the desktop's dialog came back with; `None` when cancelled.
    Browsed(Option<PathBuf>),
    /// Add the partition at this index as a drive for games.
    AddDrive(usize),
    /// Take a drive back off the list. Nothing on it is deleted.
    RemoveDrive(String),
    WantBattleNet(bool),
    /// What this SteamOS machine is for. Only ever sent on SteamOS.
    SteamOsUse(SteamOsUse),
    /// Done, write the config and get out of the way.
    Finish,
}

/// One line about what adoption did to one install.
///
/// Typed rather than a bare string because the three outcomes do not deserve
/// the same ink: an install that was taken is a receipt, an install that was
/// not is the reason someone's game is missing from the launcher, and the
/// difference used to be invisible, both were the same grey caption on a slide
/// people click past.
#[derive(Debug, Clone)]
pub struct Note {
    pub text: String,
    pub level: Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Adopted, nothing to say beyond what happened.
    Done,
    /// Adopted, but something alongside it did not work.
    Warning,
    /// Not adopted. The games are not in the launcher.
    Failed,
}

impl Note {
    fn done(text: String) -> Self {
        Self { text, level: Level::Done }
    }
    fn warning(text: String) -> Self {
        Self { text, level: Level::Warning }
    }
    fn failed(text: String) -> Self {
        Self { text, level: Level::Failed }
    }
}

/// Flatten a scan into the rows the step shows.
fn rows_of(found: Vec<Found>) -> Vec<Row> {
    let mut rows = Vec::new();
    for entry in found {
        for install in entry.wow {
            rows.push(Row {
                prefix: entry.prefix.clone(),
                origin: entry.origin,
                blocked: install.blocked.clone(),
                // Ticked by default: someone with a 120 GB install on this
                // machine is here to use it, and the alternative is a wizard
                // that quietly offers to download it again.
                take: install.blocked.is_none(),
                kind: RowKind::Wow(install),
            });
        }
        for game in entry.hand_offs {
            rows.push(Row {
                prefix: entry.prefix.clone(),
                origin: entry.origin,
                blocked: None,
                take: true,
                kind: RowKind::Game(game),
            });
        }
    }
    rows
}

/// What sends the slide on: its button in the footer, as a place for the ring.
pub fn forward(wizard: &Wizard) -> focus::Target {
    let message = match wizard.step {
        Step::Done => Msg::Finish,
        _ => Msg::Next,
    };
    focus::Target::Key(format!("{:?}", Message::Wizard(message)))
}

/// Fetch every tracked product's artwork into the cache, before anyone looks at
/// a panel. This is the "it looks weird on the first start" fix, done where the
/// user is reading a slide anyway.
pub fn prefetch(app: &Lightforge) -> iced::Task<Message> {
    let codes: Vec<ProductCode> = app.choices.iter().map(|c| c.code.clone()).collect();
    iced::Task::perform(
        crate::tasks::prefetch(app.http.clone(), app.pipeline.clone(), codes),
        |count| Message::Wizard(Msg::Prefetched(count)),
    )
}

/// A region as a picker option; the widget needs `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegionChoice(Region);

impl std::fmt::Display for RegionChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.0.label(), self.0.as_str())
    }
}

/// A locale as a picker option, named in its own language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocaleChoice(pub(crate) String);

impl std::fmt::Display for LocaleChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({})",
            lightforge_core::tact::install::locale_name(&self.0),
            self.0
        )
    }
}
