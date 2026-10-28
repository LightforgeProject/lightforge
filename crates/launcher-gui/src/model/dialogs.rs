//! The state behind each dialog and card while it is open.

use crate::*;

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

/// Which of the two report dialogs is open.
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
