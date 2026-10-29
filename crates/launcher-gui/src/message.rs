//! Everything the app reacts to, and the small values those messages carry.

use crate::*;

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Refreshed(ProductCode, Box<Refreshed>),
    Themed(ProductCode, Box<tasks::Themed>),
    /// Open the Battle.net client on its own, the other-titles path.
    OpenBattleNet,
    /// Show or hide the download manager.
    ToggleManager,
    /// Show or hide the catalog of products that could be added.
    ToggleBrowse,
    ToggleStrip,
    ToggleHints,
    /// What Battle.net reports as installed, for the catalog's labels.
    InstalledVersions(HashMap<String, String>),
    /// Time to look at Agent's registry again; see [`RegistryWatch`].
    RegistryPoll,
    /// When the registry last changed, and whether the client is up, when
    /// that was asked.
    RegistryPolled(Option<std::time::SystemTime>, Option<bool>),
    /// The product codes Battle.net's own config uses.
    KnownCodes(Vec<String>),
    /// Tick or untick "never show this again" on the release-notes popup.
    MutePatchNotes(bool),
    /// The same preference from Settings › General, read the other way round.
    ShowReleaseNotes(bool),
    /// Close the release-notes popup, recording what it was told.
    DismissPatchNotes,
    /// The start-up artwork fetch finished, with the number of products whose
    /// art is now cached.
    SplashDone(usize),
    /// GitHub's answer about newer releases, `None` for every reason there is
    /// not to say anything.
    UpdateChecked(Option<lightforge_core::update::Available>),
    /// Close the update notice, remembering the version it named.
    DismissUpdate,
    /// Open the releases page in a browser.
    OpenReleases,
    /// "Update" on the update notice: the route this copy was installed by.
    UpgradeStart,
    /// "Update now" in Settings: the notice again, already updating where
    /// Lightforge can update itself.
    UpdateNow,
    /// The update's work is done, as far as Lightforge can take it.
    UpgradeReady(Result<crate::tasks::Prepared, Problem>),
    /// While a terminal runs the update: has the binary been replaced yet?
    UpgradeWatch,
    /// Start the new version and close this one.
    UpgradeRestart,
    /// "Check for updates" in Settings: ask now, whatever the start-up check
    /// was told and whatever was dismissed before.
    CheckLauncherNow,
    /// The answer to [`Message::CheckLauncherNow`].
    LauncherChecked(Result<Option<lightforge_core::update::Available>, String>),
    /// Turn the start-up check for a new Lightforge on or off, from Settings.
    /// Distinct from `CheckForUpdates`, which asks Blizzard about the game.
    CheckLauncherUpdates(bool),
    /// The controller and touch look, on or off.
    ControllerLook(bool),
    /// Put Lightforge in the Steam library, with its artwork.
    AddToSteam,
    /// Put the splash away and show the launcher. Separate from `SplashDone`
    /// only so the floor can be waited out between the two.
    FinishSplash,
    /// Track a product from the catalog, putting it in the header.
    AddProduct(ProductCode),
    /// Stop tracking one, taking it out of the header.
    RemoveProduct(ProductCode),
    /// Queue the selected product's pre-download.
    PreDownload,
    /// Pause one product's transfer from the manager.
    PauseDownload(ProductCode),
    /// Resume or re-queue one product's transfer from the manager.
    ResumeDownload(ProductCode),
    /// Drop a transfer from the queue entirely.
    CancelDownload(ProductCode),
    /// Open the confirmation for giving a transfer up.
    AskCancelDownload(ProductCode),
    CancelDownloadDismiss,
    /// What removing a cancelled download's files freed, or why it could not.
    CancelCleaned(ProductCode, Result<u64, String>),
    /// Give the transfer up. `delete` only ever comes from a first install.
    CancelDownloadConfirm { delete: bool },
    /// A tile in the product bar was clicked.
    LineSelected(ProductLine),
    /// The cursor came onto a tile, or left one. Tracked so a drag knows
    /// what it is over without asking the layout.
    TileEntered(ProductLine),
    TileLeft(ProductLine),
    /// A tile was pressed: a click or the start of a drag, which the release
    /// decides.
    TileGrabbed(ProductLine),
    /// A tile was right-clicked: open its menu, or close it if it was open.
    TileMenu(ProductLine),
    /// A game in Add a game was clicked: show its page without adding it to
    /// the band. It joins the band when it is installed or played.
    OpenProduct(ProductCode),
    /// The cursor moved while a tile was held, in window coordinates.
    TileDragged(f32),
    /// The held tile was let go.
    TileDropped,
    /// An arrow key, Enter or Escape, or the same from a gamepad: sorted
    /// into the three below once the two cannot be doubling each other.
    Nav(NavFrom, pad::Input),
    /// The window gained or lost focus; a gamepad must not steer it from behind a game.
    WindowFocused(bool),
    /// A second launch asked for this window; see [`crate::instance`].
    Raise,
    /// The window changed size, under gamescope only; see [`fit`].
    WindowResized,
    /// The window's height in points, and the [`Lightforge::ui_scale_gen`]
    /// it was asked under.
    WindowHeight(u64, f32),
    /// An arrow key: move the selection ring; see [`focus`].
    FocusStep(focus::Step),
    /// Enter: press the control with the ring on it.
    FocusPress,
    /// Escape: close what is open, or go back a page.
    FocusBack,
    /// A frame has passed since a key press opened or closed something; see
    /// [`Lightforge::focus_entering`].
    FocusSettle,
    /// Enter in a field on a console: typing is done; see `focus`.
    TypingDone(focus::Target),
    /// Open or close a picker's list, by its key.
    TogglePicker(String),
    /// An entry was chosen from a picker's list: close it, then this.
    Picked(Box<Message>),
    /// The mouse moved, so the ring is no longer where the user is looking.
    FocusHidden,
    /// A branch was picked for the selected line. Adds the product first when
    /// the branch was on offer but not configured.
    BranchSelected(BranchChoice),
    /// Pre-flight finished; `Launch` says what it was clearing the way for.
    PreflightDone(Launch, Result<(), Problem>),
    /// Diagnostic only: subscribed to solely under `LIGHTFORGE_TRACE=frames`.
    ResizeTraced(iced::Size),
    /// The optimistic, cache-only paint that precedes [`Message::Themed`].
    CachedTheme(ProductCode, Box<tasks::Themed>),
    Primary,
    Tick,
    Downloaded(ProductCode, Result<Box<Complete>, Problem>),
    CloseRequested(iced::window::Id),
    /// A launch ended without a game of its own to watch: the client was
    /// opened, the hand-off came to nothing, or it failed.
    Launched(Result<String, Problem>),
    /// A direct launch got as far as it can tell: the game is up, or it
    /// ended before it was ever seen.
    GameStarted(Result<tasks::Started, Problem>),
    /// A session's game exited.
    GameExited(ProductCode, Result<String, Problem>),
    /// STOP was pressed on the selected game: ask first.
    AskStopGame,
    StopGameDismiss,
    StopGameConfirm,
    GameStopped(ProductCode, Result<String, Problem>),
    /// Every few seconds: is the Battle.net client up?
    ClientWatch,
    ClientSeen(bool),
    /// The menu beside a running Battle.net's button.
    ToggleClientMenu,
    /// Bring a running client's window up, without restarting it.
    ShowBattleNet,
    BattleNetShown(Result<String, Problem>),
    /// Quit was chosen: asks what is running before deciding to ask the user.
    AskQuitBattleNet,
    QuitBattleNetChecked(Vec<String>),
    QuitBattleNetDismiss,
    QuitBattleNetConfirm,
    BattleNetQuit(Result<String, Problem>),
    /// A hand-off ran to the point where there is something to say: the
    /// game is up, the client needs a person, or nothing started.
    HandoffStarted(Result<tasks::Handoff, Problem>),
    BattleNet,
    ManageSubscription,
    OpenFiles,
    CopyDebug,
    /// Run [`lightforge_core::doctor`] for the Diagnostics block.
    RunDiagnostics,
    DiagnosticsDone(Vec<lightforge_core::doctor::Check>),
    CopyDiagnostics,
    /// What the renderer picked and what it is running on, answered once at
    /// startup so a report never has to wait for it.
    SystemInfo(Box<iced::system::Information>),
    /// Open the bug report or the feedback dialog.
    OpenReport(ReportKind),
    /// What someone has typed into whichever of the two is open.
    ReportNote(iced::widget::text_editor::Action),
    /// Open the desktop's file dialog and ask for an image.
    ReportShotPick,
    /// What came back from it, or `None` if it was cancelled.
    ReportShotPicked(Option<PathBuf>),
    /// It was read, resized and encoded, or it was not an image.
    ReportShotLoaded(Result<Box<art::Shot>, String>),
    /// Take the screenshot back off the report.
    ReportShotCleared,
    /// Put the whole report on the clipboard, for when sending it failed.
    ReportCopy,
    /// Where they can be reached. Required on a bug report.
    ReportContact(String),
    /// What the report is about, which also decides what a bug carries.
    ReportCategory(report::Category),
    ReportSend,
    /// Whether it actually left the machine.
    ReportSent(bool),
    ReportCancel,
    /// Open Settings at a particular section, for a link that means a
    /// specific page, not "settings somewhere".
    ShowSettingsTab(SettingsTab),
    /// The Settings page scrolled, by any means.
    SettingsScrolled(scrollable::Viewport),
    /// Where each Settings section sits on the page, as last laid out.
    SettingsMeasured(SettingsLayout),
    /// A switch in Game Settings, or in Settings › Battle.net, was flipped.
    Tweak(Scope, Tweak, bool),
    /// The environment-variable editor was typed in.
    EnvEdit(Scope, iced::widget::text_editor::Action),
    /// Typing in the env editor paused; the number says after which edit.
    EnvSettled(u64),
    /// The library picked in Game Settings to move the game to, by id.
    MoveTargetPicked(String),
    /// Move the game in Game Settings to the picked library.
    MoveGame,
    CancelMove,
    /// The game is in the new library and recorded there; the old copy is next.
    GameCopied(ProductCode, Result<lightforge_core::migrate::Moved, String>),
    /// The old copy is gone, with the bytes it freed.
    GameMoved(ProductCode, Result<u64, String>),
    /// Remove was pressed on a copy left in another library, by its id; it
    /// waits for its confirmation.
    AskRemoveLeftover(String),
    CancelRemoveLeftover,
    RemoveLeftover,
    /// Throw away this product's shader cache, e.g. after a Proton swap.
    ClearShaderCache,
    /// `true` when there was a cache to delete.
    ShaderCacheCleared(Result<bool, String>),
    /// Opt in or out of one optional download, by manifest tag name. On starts
    /// the download; off asks whether to delete what it fetched.
    OptionalContent(String, bool),
    /// What the open Game Settings product's optional download weighs.
    OptionalSized(ProductCode, Box<Result<lightforge_core::tact::install::Offerings, String>>),
    /// What deleting an optional download's files would free.
    OptionalPlanned(Result<Box<lightforge_core::tact::reclaim::Reclaim>, String>),
    /// Leave the files an option fetched where they are.
    KeepOptional,
    /// Delete the files the question was asked about.
    RemoveOptional,
    /// The deletion finished, with the bytes it freed.
    OptionalRemoved(Result<u64, Problem>),
    /// The cog beside the primary button.
    ToggleGear,
    /// The cog's "Show in Explorer": the game's folder in the file manager.
    ShowInExplorer,
    /// The cog's "Game Version": the build and where it is installed.
    AskGameVersion,
    CloseGameVersion,
    /// The cog's "Modify Install": the language the game is installed in.
    AskModifyInstall,
    ModifyLocale(String),
    ModifyCancel,
    ModifyStart,
    /// The menu under the Lightforge wordmark.
    ToggleAppMenu,
    /// Back and forward through the pages visited this session.
    GoBack,
    GoForward,
    /// Open one of the project's own links in the browser.
    OpenLink(&'static str),
    /// Quit.
    Exit,
    /// A click that no widget consumed, close whatever is open, the way
    /// every other context menu on the desktop behaves.
    Dismiss,
    /// A click *inside* an open panel. It only stops the click reaching
    /// [`Message::Dismiss`] underneath.
    MenuClick,
    /// Ask the version service what is current, and read what the build
    /// offers.
    CheckForUpdates,
    CheckedForUpdates(ProductCode, Box<Result<lightforge_core::tact::install::Offerings, String>>),
    /// Plan against the build and fetch whatever the store is short, after
    /// checking what is there is not damaged.
    ScanAndRepair,
    Scanned(Box<Result<tasks::Scanned, String>>),
    /// Hash every stored byte against its checksums, or, while that runs,
    /// stop it.
    DeepScan,
    DeepScanned(Box<Result<lightforge_core::tact::verify::Report, String>>),
    /// Drop what the last deep scan found damaged, once nothing is running.
    RepairDamaged,
    Repaired(Result<usize, tasks::Unrepaired>),

    Show(Screen),
    SetupStatus(Option<Box<SetupStatus>>),
    RunSetup,
    /// The first-run wizard; see [`wizard`].
    Wizard(wizard::Msg),
    /// Take back a declined Battle.net and install it after all.
    InstallBattleNet,
    /// Put the 32-bit library command on the clipboard; Lightforge never runs it.
    CopyRuntimeCommand(String),
    /// Re-read the machine after the user has installed the libraries, so the
    /// row clears without restarting the app.
    RecheckRuntime,
    SetupFinished(Result<Box<SetupStatus>, Problem>),
    PinDefaultProton(ProtonChoice),
    /// The build Battle.net and Agent launch under.
    PinBattleNetProton(ProtonChoice),
    /// One product's own pin; the Proton page lists every game.
    PinProductProton(ProductCode, ProtonChoice),
    /// The download cap field changed.
    DownloadLimitChanged(String),
    /// The pause-while-playing switch was flipped.
    PauseWhilePlaying(bool),
    /// Whether the launcher gets out of the way once a game is up.
    MinimizeOnLaunch(bool),
    /// Whether a Battle.net hand-off puts the client in the tray.
    BattleNetMinimized(bool),
    CloseBattleNetOnPlay(bool),
    /// Battle.net's own limit, as read from its config.
    BattleNetLimitRead(Option<u64>),
    /// The Battle.net limit field changed.
    BattleNetLimitChanged(String),
    /// Writing that limit finished.
    BattleNetLimitWritten(Result<(), String>),
    CheckProton,
    ProtonChecked(Result<provision::ProtonReleases, String>),
    /// Open the "Install Proton Version" window.
    OpenProtonInstaller,
    CloseProtonInstaller,
    /// Another tab of that window.
    ProtonInstallerTab(ProtonCategory),
    /// Install one release, by tag.
    InstallProton(String),
    /// Ask Steam to install one of Valve's Protons, by app id.
    InstallSteamProton(u32),
    /// That build finished, or did not.
    ProtonInstalled(Result<(), Problem>),
    /// The tested build fetched in the background; a failure is not shown.
    TestedProtonFetched(Result<(), Problem>),
    /// Delete one managed build, by version.
    RemoveProton(String),
    ProtonRemoved(Result<String, String>),
    /// Measured sizes for the installed builds.
    ProtonSizes(Vec<(PathBuf, u64)>),
    StorageMeasured(Box<storage::Report>),
    InstallOffered(ProductCode, Box<Result<Offerings, String>>),
    InstallHighRes(bool),
    InstallTarget(usize),
    InstallCancel,
    InstallStart,
    AskUninstall,
    UninstallPlanned(ProductCode, Box<Result<tasks::UninstallPreview, String>>),
    UninstallCancel,
    UninstallConfirm,
    /// Launch anyway, under a build older than the one that built the prefix.
    DowngradeConfirm,
    DowngradeCancel,
    Uninstalled(ProductCode, Result<u64, Problem>),
    ClearCache,
    CacheCleared(Result<u64, String>),
    OpenLibrary,
    /// Re-scan the mount table: which partitions could be added, and whether
    /// an absent library has come back somewhere else.
    RefreshDrives,
    /// Add `self.partitions[i]` as a library.
    AddDrive(usize),
    /// Make this library the default install target, and Battle.net's.
    SetDefaultDrive(String),
    /// Take a drive off the list. Nothing on it is deleted.
    RemoveDrive(String),
    /// Clear what this library holds that is only a cache.
    ClearDrive(String),
    DriveCleared(String, Result<u64, String>),
    /// What freeing a library's unused game data would free, by library id.
    UnusedPlanned(String, Result<tasks::Unused, String>),
    FreeUnused(String),
    UnusedFreed(String, Result<u64, String>),
    /// Open a library's folder in the file manager.
    OpenDrive(String),
    OpenConfigFolder,
    LocationChanged(String),
    UseLocation,

    AddonsListed(Box<addonwork::Listing>),
    TokenChanged(String),
    ImportToken,
    UpdateAddons,
    AddonsFinished(Result<Box<lightforge_core::addons::Report>, Problem>),
    RemoveAddon(String, String),
    /// First click on a Remove: arm it. The second one removes.
    ArmRemoveAddon(String, String),
    /// Think better of it.
    CancelRemoveAddon,
    /// Narrow the installed list.
    AddonFilterChanged(String),
    /// A debounced search coming due. Carries the keystroke count it was
    /// scheduled at.
    SearchSettled(u64),
    /// Build a token naming everything installed here.
    ExportAddons,
    /// Put the built token on the clipboard.
    CopyExport,
    /// Typing in the export box. Kept so the field can be selected and copied
    /// from; the token is rebuilt by Export, never by editing.
    ExportEdited(String),
    /// Show the product's AddOns folder in the file manager.
    OpenAddonsDir,
    AddonRemoved(Result<Vec<String>, String>),
    OpenUrl(String),
    /// Drop one "could not install" row. It is a report, not a record:
    /// nothing was installed, so there is nothing to remove but the line.
    DismissUnavailable(String),

    AddonTab(AddonTab),
    /// The channel every addon follows unless it has one of its own.
    AddonChannelDefault(ChannelChoice),
    /// One addon's own channel, or `None` to follow the default again.
    AddonChannelFor(String, String, RowChannel),
    /// Show one addon's own menu, or close whichever is showing.
    AddonMenu(Option<(String, String)>),
    /// Leave one addon out of everything that updates in bulk, or stop doing so.
    AddonIgnore(String, String, bool),
    /// One addon's auto-update choice, or `None` to follow the default again.
    AddonAutoUpdate(String, String, Option<bool>),
    /// Hold one addon at a file id, or `None` to let it track its channel.
    AddonPin(String, String, Option<String>),
    /// Recognise hand-installed addons and manage them from now on.
    AdoptAddons,
    /// What adoption found.
    AdoptFinished(Box<Result<Vec<lightforge_core::addons::manage::Adopted>, String>>),
    /// Open the version picker for one addon.
    PickVersion(String, String, String, Option<String>),
    /// The catalogue answered with the builds on offer.
    VersionsListed(Box<Result<Vec<lightforge_core::addons::catalog::FileInfo>, String>>),
    /// Install one exact build and pin the addon to it.
    InstallVersion(String, String, String),
    /// Close the picker without choosing.
    CancelVersionPicker,
    OpenGameSettings,
    CloseGameSettings,
    /// Something pressed in Game Settings that belongs to the page behind
    /// it: the popup closes, then the message runs as if sent from there.
    FromGameSettings(Box<Message>),
    BackupsListed(Result<Vec<lightforge_core::backup::Backup>, String>),
    BackupNow,
    AskRestore(PathBuf),
    /// Delete was pressed on a backup; it waits for its confirmation.
    AskDeleteBackup(PathBuf),
    /// Back out of whichever confirmation a backup row is showing.
    CancelRestore,
    /// Restore the asked-about backup: every account, or the one named.
    ConfirmRestore(Option<String>),
    ConfirmDeleteBackup,
    BackupFinished(Result<String, String>),
    /// Open the detail view for one addon: provider, id, name.
    ShowAddonDetail(String, String, String),
    /// The catalogue answered with the project, description and changelog.
    AddonDetailLoaded(Box<Result<addonwork::Detail, String>>),
    /// Which half of the detail view is showing.
    AddonDetailTab(DetailTab),
    /// Close the detail view.
    CloseAddonDetail,
    /// Whether the next import pins to the versions the token names.
    ImportPinned(bool),
    /// Put Battle.net on this game's tile, so its Install button is reachable.
    InstallViaBattleNet,
    /// The client's catalogue has been read.
    FamiliesRead(Box<lightforge_core::families::Families>),
    /// Fetch one addon again and replace what it wrote.
    ReinstallAddon(String, String),
    /// Whether addons update themselves unless they say otherwise.
    AddonAutoUpdateDefault(bool),
    /// Update whatever updates itself, for the product on screen.
    AutoUpdateAddons,
    /// `None` when there was nothing to update.
    AutoUpdateFinished(Result<Option<Box<lightforge_core::addons::Report>>, Problem>),
    SettingsTab(SettingsTab),
    SearchChanged(String),
    /// Search Browse for an addon by name: a missing dependency.
    FindAddon(String),
    RunSearch,
    SearchDone(Box<addonwork::SearchOutcome>),
    InstallAddon(String, String),
    CheckAddonUpdates,
    AddonUpdatesChecked(ProductCode, Result<Vec<lightforge_core::addons::manage::UpdateStatus>, String>),
}

/// Where a press that moves the ring came from; see [`Message::Nav`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavFrom {
    Keys,
    Pad,
}

/// A press from the other source this soon after one from the first is the
/// same press arriving twice. Longer than both a key's repeat and the pad's,
/// so a held direction stays with whichever source started it.
pub(crate) const NAV_DOUBLED: Duration = Duration::from_millis(150);

/// Which profile a launch-settings control edits.
///
/// A product's own page, or the shared Battle.net profile; explicit, not
/// read off the open screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The product the launcher is showing.
    Product,
    /// `Config::battle_net_tweaks`: shared, because the mechanism cannot be
    /// per-game. See that field for why.
    BattleNet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tweak {
    GamePerformance,
    MangoHud,
    Wayland,
    NvidiaGpu,
    Nvapi,
}

/// A Proton build in a picker, or the "follow the default" entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtonChoice(pub(crate) Option<String>, pub(crate) Unpinned);

/// What a picker's "no pin of my own" entry falls through to, which it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unpinned {
    /// The app default: nothing sits above it, so unpinned is the newest build
    /// installed.
    Newest,
    /// A game Lightforge launches itself, and Battle.net's own row: unpinned
    /// follows the app default.
    Default,
    /// A title Battle.net opens: unpinned follows Battle.net's pin, which is
    /// the old behaviour and still the sensible one to come back to.
    BattleNet,
}

impl std::fmt::Display for ProtonChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.0, self.1) {
            (Some(version), _) => {
                f.write_str(version)?;
                // Anything that is not the lineage Lightforge is tested on.
                match proton::ProtonFlavor::classify(version) == proton::ProtonFlavor::GloriousEggroll {
                    true => Ok(()),
                    false => f.write_str(" (untested)"),
                }
            }
            (None, Unpinned::Newest) => f.write_str("Newest installed"),
            (None, Unpinned::Default) => f.write_str("Follow the default"),
            (None, Unpinned::BattleNet) => f.write_str("Follow Battle.net"),
        }
    }
}

/// A product in the picker. `ProductCode` has no `Display`, and the label the
/// user reads is not the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) code: ProductCode,
    pub(crate) label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// One entry in the branch dropdown: a build of the selected line.
///
/// Includes branches not set up yet: picking one adds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchChoice {
    pub(crate) code: ProductCode,
    pub(crate) branch: Branch,
    /// The catalog's name for this product, used when adding it to config.
    pub(crate) label: String,
    /// Battle.net's name for this version where it has one of its own,
    /// `Stress Test`, not `Live`; see `Families::version_name`.
    pub(crate) name: Option<String>,
    pub(crate) configured: bool,
}

impl std::fmt::Display for BranchChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.name.as_deref().unwrap_or(self.branch.label());
        match self.configured {
            true => f.write_str(name),
            // In words: pick_list cannot style one entry.
            false => write!(f, "{name} — add"),
        }
    }
}

/// A channel as a `pick_list` option; the widget needs `Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChannelChoice(pub(crate) Channel);

impl std::fmt::Display for ChannelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.label())
    }
}

/// One addon's channel: its own, or whatever the default happens to be.
///
/// "Default" is a real option: it differs from stable once the default changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowChannel(pub(crate) Option<Channel>);

impl RowChannel {
    pub(crate) const ALL: [Self; 4] = [
        Self(None),
        Self(Some(Channel::Stable)),
        Self(Some(Channel::Beta)),
        Self(Some(Channel::Alpha)),
    ];
}

impl std::fmt::Display for RowChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            None => "Default",
            Some(Channel::Stable) => "Stable",
            Some(Channel::Beta) => "Beta",
            Some(Channel::Alpha) => "Alpha",
        })
    }
}
