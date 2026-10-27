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

use std::path::PathBuf;

use iced::widget::{column, container, image, row, scrollable, text, text_input, Space};
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

/// How big the Discord and X marks are drawn on the last slide. Larger than the
/// logo menu's glyph: there they label a row of text, here they are the thing
/// being offered.
const MARK: f32 = 32.0;

/// How wide the region and language pickers are: the longest locale name.
const PICKER: f32 = 300.0;

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

/// Handle one wizard message. Returns the task it wants run.
pub fn update(app: &mut Lightforge, msg: Msg) -> iced::Task<Message> {
    match msg {
        Msg::Open => {
            app.wizard = Some(Wizard::default());
            prefetch(app)
        }

        Msg::Scan => {
            app.wizard = Some(Wizard { step: Step::Found, scanning: true, scan_only: true, ..Wizard::default() });
            scan(app)
        }

        Msg::Finish => {
            let pending = app.wizard.as_ref().map(|w| w.pending.clone()).unwrap_or_default();
            app.wizard = None;
            // Marks the first run as seen; Settings keeps the button for a
            // second one.
            app.config.setup_complete = true;
            // A fresh install has nothing to be told what is new about: the
            // notes describe the version it has just been set up with. Marked
            // seen here so the *second* start does not open a popup about the
            // build the wizard was running.
            app.config.patch_notes_seen = Some(env!("CARGO_PKG_VERSION").to_owned());
            app.save_config();
            // Played from Game Mode, so it belongs in the Steam library
            // there. A failure is said in Settings, under the button that
            // tries again; setup is done either way.
            if app.console() {
                app.add_to_steam();
            }
            if !pending.is_empty() {
                link_hand_offs(app, &pending);
            }
            app.go_to(crate::Screen::Dashboard);
            // The band and the panel both change shape when a game is adopted
            // or Battle.net is declined, so the product on screen is re-read.
            app.reload(app.selected.code.clone())
        }

        Msg::Next => advance(app),

        // There is nothing behind the one slide a scan shows: its Back is
        // the way out, and Escape with it.
        Msg::Back if app.wizard.as_ref().is_some_and(|w| w.scan_only) => {
            app.wizard = None;
            app.reload(app.selected.code.clone())
        }

        Msg::Back => {
            if let Some(wizard) = app.wizard.as_mut() {
                if let Some(back) = wizard.step.back() {
                    wizard.step = back;
                }
            }
            iced::Task::none()
        }

        Msg::Region(region) => {
            app.config.region = region;
            app.save_config();
            // Everything already fetched was fetched against the old region's
            // version service, so the panel has to ask again.
            app.reload(app.selected.code.clone())
        }

        Msg::Locale(locale) => {
            app.config.locale = locale;
            app.save_config();
            iced::Task::none()
        }

        Msg::Prefetched(count) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.prefetched = Some(count);
            }
            iced::Task::none()
        }

        Msg::Scanned(found) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.scanning = false;
                wizard.scanned = true;
                wizard.rows = rows_of(found);
            }
            mark_known(app);
            iced::Task::none()
        }

        Msg::Take(at, take) => {
            if let Some(wizard) = app.wizard.as_mut() {
                if let Some(row) = wizard.rows.get_mut(at) {
                    row.take = take && row.blocked.is_none();
                }
            }
            iced::Task::none()
        }

        Msg::PointInput(path) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.pointed = path;
                wizard.point_error = None;
            }
            iced::Task::none()
        }

        Msg::Browse => iced::Task::perform(
            // The XDG portal through `ashpd`, as the screenshot picker does;
            // see `update/report.rs` for why not `rfd`.
            async {
                let request = ashpd::desktop::file_chooser::OpenFileRequest::default()
                    .title("Choose a Wine prefix or a game folder")
                    .modal(true)
                    .directory(true)
                    .multiple(false);
                let files = request.send().await.ok()?.response().ok()?;
                files.uris().first().and_then(|uri| uri.to_file_path().ok())
            },
            |path| Message::Wizard(Msg::Browsed(path)),
        ),

        Msg::Browsed(path) => {
            let Some(path) = path else { return iced::Task::none() };
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.pointed = path.display().to_string();
            }
            update(app, Msg::Point)
        }

        Msg::Point => {
            let Some(wizard) = app.wizard.as_mut() else {
                return iced::Task::none();
            };
            if wizard.pointing {
                return iced::Task::none();
            }
            let path = match crate::parse_path(&wizard.pointed) {
                Ok(path) => path,
                Err(e) => {
                    wizard.point_error = Some(e);
                    return iced::Task::none();
                }
            };
            wizard.pointing = true;
            wizard.point_error = None;
            iced::Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || discover::inspect_pointed(&path)).await.ok().flatten()
                },
                |found| Message::Wizard(Msg::Pointed(found)),
            )
        }

        Msg::Pointed(found) => {
            let Some(wizard) = app.wizard.as_mut() else {
                return iced::Task::none();
            };
            wizard.pointing = false;
            match found {
                Some(found) if !found.is_empty() => {
                    let fresh = rows_of(vec![found]);
                    // Only what is not already listed: pointing at a prefix the
                    // scan found would otherwise double every row in it.
                    for row in fresh {
                        let already = wizard.rows.iter().any(|have| match (&have.kind, &row.kind) {
                            (RowKind::Wow(a), RowKind::Wow(b)) => a.path == b.path,
                            (RowKind::Game(a), RowKind::Game(b)) => a.path == b.path,
                            _ => false,
                        });
                        if !already {
                            wizard.rows.push(row);
                        }
                    }
                    wizard.pointed.clear();
                    wizard.point_error = None;
                    mark_known(app);
                }
                _ => {
                    wizard.point_error = Some(
                        "nothing of Blizzard's in there — point at a Wine prefix, or at a \
                         World of Warcraft folder with a .build.info in it"
                            .to_owned(),
                    )
                }
            }
            iced::Task::none()
        }

        Msg::AddDrive(index) => {
            let outcome = app.add_drive(index);
            if outcome.is_ok() {
                app.refresh_drives();
            }
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.drive_error = outcome.err();
            }
            iced::Task::none()
        }

        Msg::RemoveDrive(id) => {
            // `refresh_drives` puts the partition back in the list below, so
            // a drive added by mistake can be added again straight away.
            let outcome = app.remove_drive(&id);
            if outcome.is_ok() {
                app.refresh_drives();
            }
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.drive_error = outcome.err();
            }
            iced::Task::none()
        }

        Msg::SteamOsUse(answer) => {
            app.config.steamos_use = Some(answer);
            app.save_config();
            // The window changes with the answer, fullscreen for a console,
            // so whoever answered sees what they chose straight away.
            app.window_mode()
        }

        Msg::WantBattleNet(want) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.battle_net = want;
            }
            app.config.battle_net_declined = !want;
            app.save_config();
            // What is outstanding changed, so what the Install step draws has
            // to be read again.
            app.setup_status()
        }
    }
}

/// Leave the current step, doing whatever that step's answers asked for.
fn advance(app: &mut Lightforge) -> iced::Task<Message> {
    let Some(step) = app.wizard.as_ref().map(|w| w.step) else {
        return iced::Task::none();
    };
    match step {
        // Entering the scan: it reads two files per prefix and measures what it
        // finds, so it runs off the UI thread like any other walk of a disk.
        Step::Place => {
            let fresh = app.wizard.as_ref().is_some_and(|w| !w.scanned);
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.step = Step::Found;
                wizard.scanning = fresh;
            }
            match fresh {
                true => scan(app),
                false => iced::Task::none(),
            }
        }

        // After setup the prefix and Battle.net are there already, so a
        // found game is linked as it is added, and the slide stays up to say
        // how that went.
        Step::Found if app.wizard.as_ref().is_some_and(|w| w.scan_only) => {
            adopt_taken(app);
            let pending = app.wizard.as_mut().map(|w| std::mem::take(&mut w.pending)).unwrap_or_default();
            if !pending.is_empty() {
                link_hand_offs(app, &pending);
            }
            iced::Task::none()
        }

        Step::Found => {
            adopt_taken(app);
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.step = Step::Install;
            }
            app.setup_status()
        }

        other => {
            if let Some(wizard) = app.wizard.as_mut() {
                if let Some(next) = other.next() {
                    wizard.step = next;
                }
            }
            iced::Task::none()
        }
    }
}

/// Look through the usual Wine prefixes, off the UI thread: it reads two
/// files per prefix and measures what it finds, like any other walk of a disk.
fn scan(app: &Lightforge) -> iced::Task<Message> {
    let prefix = app.layout.prefix();
    iced::Task::perform(
        async move {
            tokio::task::spawn_blocking(move || discover::scan(&prefix)).await.unwrap_or_default()
        },
        |found| Message::Wizard(Msg::Scanned(found)),
    )
}

/// Untick and explain what Lightforge already has, so a second look offers
/// only what is new rather than the install it has been playing for a week.
///
/// A WoW install is had when its folder is a library's and every product in
/// it is assigned there; a Battle.net game when the link to it is already in
/// the default library.
fn mark_known(app: &mut Lightforge) {
    let config = &app.config;
    let default = config.libraries.iter().find(|l| l.is_default);
    let known = |row: &Row| match &row.kind {
        RowKind::Wow(install) => {
            let library = config.libraries.iter().find(|l| Some(l.games()) == install.path.parent());
            library.is_some_and(|library| {
                install.products.iter().all(|code| {
                    config.product(code).is_some_and(|p| p.profile.library.as_deref() == Some(library.id.as_str()))
                })
            })
        }
        RowKind::Game(game) => default.is_some_and(|library| {
            std::fs::read_link(library.games().join(&game.label)).is_ok_and(|to| to == game.path)
        }),
    };
    let Some(wizard) = app.wizard.as_mut() else {
        return;
    };
    for row in wizard.rows.iter_mut().filter(|row| row.blocked.is_none()) {
        if known(row) {
            row.take = false;
            row.blocked = Some("already in Lightforge".to_owned());
        }
    }
}

/// Adopt every ticked WoW install, and remember the ticked hand-offs for later.
///
/// In place, in the time it takes to write a marker file: the install's parent
/// directory becomes a storage library and the products in it are assigned to
/// it. Nothing is copied and nothing is verified, the files are already there,
/// and `.build.info` says what they are.
///
/// Hand-offs cannot be done here. Linking one needs a prefix to link into and a
/// Battle.net registry to graft its record onto, and neither exists until the
/// next step has run.
fn adopt_taken(app: &mut Lightforge) {
    let Some(rows) = app.wizard.as_ref().map(|w| w.rows.clone()) else {
        return;
    };
    let mut notes = Vec::new();
    let mut pending = Vec::new();

    for row in rows.iter().filter(|row| row.take) {
        match &row.kind {
            RowKind::Wow(install) => match adopt_one(app, install) {
                Ok(mut found) => notes.append(&mut found),
                Err(e) => notes.push(Note::failed(format!(
                    "{} could not be added: {e}",
                    install.path.display()
                ))),
            },
            RowKind::Game(game) => {
                // Into the band here, not at the Games step. Ticking a game on
                // this slide is the user saying they play it; making them say
                // it again two slides later is asking the same question twice.
                // The link itself still waits for the prefix, see `pending`.
                track_hand_off(app, game);
                pending.push((game.clone(), row.prefix.clone()));
            }
        }
    }

    if let Some(wizard) = app.wizard.as_mut() {
        wizard.adopted = notes;
        wizard.pending = pending;
        // Ticked rows have been acted on; leaving them ticked would adopt them
        // again if the user stepped back and forward.
        for row in &mut wizard.rows {
            if row.take {
                row.take = false;
                row.blocked = Some("already added".to_owned());
            }
        }
    }
}

/// Add one found install to the config, in place.
///
/// Returns every line worth showing about it, not just one: an install can be
/// adopted *and* have something to say, and the something is the part that
/// explains why Battle.net still cannot see the game.
fn adopt_one(app: &mut Lightforge, install: &WowInstall) -> Result<Vec<Note>, String> {
    let mut notes = Vec::new();

    let (id, label) = match discover::plan(install, &app.config.libraries) {
        Ok(plan) => {
            let adopted = discover::adopt(&plan, &app.layout).map_err(|e| chain(&e))?;
            let id = adopted.library.id.clone();
            let label = adopted.library.label.clone();
            if adopted.repaired {
                notes.push(Note::warning(format!(
                    "{} would not accept a file; its owner-write permission was added",
                    install.path.display()
                )));
            }
            if let Some(reason) = adopted.battle_net {
                notes.push(Note::warning(format!(
                    "Battle.net was not told about {}, so you may have to use its own \
                     Locate button for this one: {reason}",
                    install.path.display()
                )));
            }
            app.config.libraries.push(adopted.library);
            (id, label)
        }
        // The folder is already inside a library Lightforge knows: a second
        // run of the wizard, or an install sitting in a drive that was added
        // by hand. There is nothing to adopt, but the products in it may still
        // be unassigned, which is the whole difference between a tile that
        // says Play and one that offers to download 100 GB again.
        Err(discover::AdoptError::AlreadyOurs) => {
            let parent = install.path.parent().unwrap_or(&install.path);
            let library = app
                .config
                .libraries
                .iter()
                .find(|l| l.games() == parent)
                .ok_or_else(|| "that folder is managed but its drive is gone".to_owned())?;
            (library.id.clone(), library.label.clone())
        }
        Err(e) => return Err(chain(&e)),
    };

    for code in &install.products {
        if !app.config.products.iter().any(|p| &p.code == code) {
            let name = app
                .catalog
                .get(code)
                .map(|e| e.label.clone())
                .unwrap_or_else(|| code.as_str().to_owned());
            app.config.products.push(lightforge_core::Product::new(code.clone(), name));
        }
        if let Some(product) = app.config.product_mut(code) {
            product.profile.library = Some(id.clone());
        }
    }
    app.save_config();
    app.rebuild_layout();
    app.sync_choices();
    // Only when there is a prefix: a library is linked into one when it is
    // built, and `link_libraries` is the one call that must never run against a
    // drive that is not there.
    if lightforge_core::prefix::PrefixStatus::inspect(&app.layout).initialized {
        if let Err(e) = lightforge_core::prefix::link_libraries(&app.layout) {
            notes.push(Note::warning(format!("the prefix could not be relinked: {}", chain(&e))));
        }
    }
    notes.insert(
        0,
        Note::done(format!(
            "{} — kept where it is, on {label}",
            install
                .products
                .iter()
                .map(|code| code.short_name().to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    );
    Ok(notes)
}

/// Put a found Battle.net game in the band, if it is not there already.
///
/// The code on disk and the code in the catalog are not always the same
/// string: Battle.net calls Hearthstone `hs_beta` on a machine where it is
/// installed and the catalog ships it as `hsb`. `Catalog::reconcile` rewrites
/// the catalog to whatever the machine reports, but it only runs once the
/// client's own code list has been read, which a first run may not have done
/// yet. So the family is the fallback: two codes name one game when they
/// resolve to the same Battle.net game family, which is the client's own
/// notion of identity and the only one available.
fn track_hand_off(app: &mut Lightforge, game: &HandOff) {
    let (code, label) = hand_off_entry(&app.catalog, game);
    if app.config.products.iter().any(|p| p.code == code) {
        return;
    }
    app.config.products.push(lightforge_core::Product::new(code, label));
    app.save_config();
    app.sync_choices();
}

/// Which catalog entry a found game is, and what to call it.
///
/// Split out and pure because the matching is the part that can be wrong: a
/// game added under the code on disk when the catalog holds another one is a
/// tile with no artwork that never matches what is installed.
fn hand_off_entry(
    catalog: &lightforge_core::product::Catalog,
    game: &HandOff,
) -> (ProductCode, String) {
    let found: ProductCode = game.code.clone().into();
    if let Some(entry) = catalog.get(&found) {
        return (found, entry.label.clone());
    }
    let by_family = lightforge_core::battlenet::game_uid(&found).and_then(|family| {
        catalog
            .hand_offs()
            .find(|e| {
                lightforge_core::battlenet::game_uid(&e.code).as_deref() == Some(family.as_str())
            })
            .map(|e| (e.code.clone(), e.label.clone()))
    });
    // A game the catalog has never heard of still goes in, under the name of
    // the folder it was found in, which is what the user recognises.
    by_family.unwrap_or_else(|| (found, game.label.clone()))
}

/// Point our Battle.net at the games another one installed.
fn link_hand_offs(app: &mut Lightforge, pending: &[(HandOff, PathBuf)]) {
    let Some(library) = app.config.libraries.iter().find(|l| l.is_default).cloned() else {
        return;
    };
    if app.config.battle_net_declined {
        return;
    }
    let mut linked = Vec::new();
    let mut missed = Vec::new();
    for (game, prefix) in pending {
        match discover::link_hand_off(game, prefix, &app.layout, &library) {
            Ok(true) => linked.push(game.label.clone()),
            // Linked, but Battle.net was not told, it will offer to find the
            // game itself, which works and is not worth an error.
            Ok(false) => missed.push(game.label.clone()),
            Err(e) => missed.push(format!("{} ({e})", game.label)),
        }
    }
    app.status = match (linked.is_empty(), missed.is_empty()) {
        (true, true) => return,
        (false, true) => format!("Battle.net now knows about {}", linked.join(", ")),
        (true, false) => format!("{} — ask Battle.net to locate these", missed.join(", ")),
        (false, false) => format!(
            "Battle.net now knows about {}; ask it to locate {}",
            linked.join(", "),
            missed.join(", ")
        ),
    };
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

// ------------------------------------------------------------------- view --

/// The whole wizard: one card, centred, with the step's own body in it.
pub fn view<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let body = match wizard.step {
        Step::Welcome => welcome(app),
        Step::Place => place(app, wizard),
        Step::Found => found(app, wizard),
        Step::Install => install(app, wizard),
        Step::Games => games(app),
        Step::Done => done(app, wizard),
    };

    let card = column![
        match wizard.scan_only {
            true => Element::from(Space::new()),
            false => dots(wizard.step),
        },
        text(match wizard.scan_only {
            true => "Look for games",
            false => wizard.step.title(),
        })
        .size(theme::TITLE),
        container(body).height(Length::Fill),
        footer(app, wizard),
    ]
    .spacing(theme::LG);

    container(
        container(card)
            .padding(theme::XL)
            .width(Length::Fixed(820.0))
            .height(Length::Fixed(660.0))
            .style(theme::floating),
    )
    .center(Length::Fill)
    .into()
}

/// How far along, as one dot per step.
fn dots(step: Step) -> Element<'static, Message> {
    let at = Step::ALL.iter().position(|s| *s == step).unwrap_or(0);
    let mut row = row![].spacing(theme::SM).align_y(Alignment::Center);
    for (index, _) in Step::ALL.iter().enumerate() {
        let colour = match index <= at {
            true => theme::ACCENT,
            false => theme::MUTED,
        };
        let size = match index == at {
            true => 8.0,
            false => 6.0,
        };
        row = row.push(
            container(Space::new().width(Length::Fixed(size)).height(Length::Fixed(size)))
                .style(theme::dot(colour)),
        );
    }
    row.into()
}

/// Back, and the step's own button.
///
/// No way out halfway: every step can be passed without doing anything, so
/// the button that skipped them all only ever caught a stray press, and on a
/// controller one press of A was enough to lose the setup.
fn footer<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);

    if wizard.scan_only {
        let ticked = wizard.rows.iter().any(|row| row.take);
        return actions
            .push(secondary("Close", Message::Wizard(Msg::Back)).enabled(!app.busy))
            .push(Space::new().width(Length::Fill))
            .push(
                action_button("Add these", theme::INSTALL, Message::Wizard(Msg::Next))
                    .enabled(ticked && !wizard.scanning),
            )
            .into();
    }

    if wizard.step.back().is_some() && wizard.step != Step::Done {
        actions = actions.push(secondary("Back", Message::Wizard(Msg::Back)).enabled(!app.busy));
    }
    actions = actions.push(Space::new().width(Length::Fill));

    // What the step's own button says, and whether it can be pressed yet.
    let (label, ready): (&str, bool) = match wizard.step {
        Step::Welcome => ("Get started", welcome_answered(app)),
        Step::Place => ("Look for my games", true),
        Step::Found => (
            match wizard.rows.iter().any(|row| row.take) {
                true => "Add these and continue",
                false => "Continue",
            },
            !wizard.scanning,
        ),
        // Deliberately pressable while nothing has been provisioned: someone
        // who cannot install Battle.net today should still reach the end.
        Step::Install => ("Continue", !app.setup_running),
        Step::Games => ("Continue", true),
        Step::Done => ("Ready to play", true),
    };

    let message = match wizard.step {
        Step::Done => Msg::Finish,
        _ => Msg::Next,
    };
    actions = actions.push(
        action_button(label, theme::INSTALL, Message::Wizard(message)).enabled(ready),
    );

    actions.into()
}

/// Whether the first slide can be left. On SteamOS the Deck-or-desktop
/// question has to be answered first.
fn welcome_answered(app: &Lightforge) -> bool {
    !app.steamos || app.config.steamos_use.is_some()
}

fn welcome(app: &Lightforge) -> Element<'_, Message> {
    let mut body = column![
        text(
            "Lightforge installs and launches World of Warcraft on Linux itself, and hands \
             everything else to Battle.net. This takes a couple of minutes, and nothing is \
             downloaded until you say so."
        )
        .size(theme::BODY),
    ]
    .spacing(theme::MD);

    // The one thing on this machine Lightforge cannot fix on the user's behalf,
    // said before anything is downloaded rather than after 400 MB of Proton:
    // Battle.net's installer is 32-bit and it is the last step of setup.
    if app.setup.as_ref().is_some_and(|s| !s.thirty_two_bit) {
        body = body.push(Lightforge::runtime_row());
    }

    let regions: Vec<RegionChoice> = Region::ALL.into_iter().map(RegionChoice).collect();
    let locales: Vec<LocaleChoice> =
        LOCALES.iter().map(|(code, _)| LocaleChoice(code.to_string())).collect();

    body = body.push(field_row(
        "Region".to_owned(),
        app.picker(
            "wizard region".to_owned(),
            regions,
            Some(RegionChoice(app.config.region)),
            "",
            |choice| Message::Wizard(Msg::Region(choice.0)),
            Length::Fixed(PICKER),
            theme::BODY,
        ),
    ));
    body = body.push(field_row(
        "Language".to_owned(),
        app.picker(
            "wizard language".to_owned(),
            locales,
            Some(LocaleChoice(
                lightforge_core::tact::install::current_locale(&app.config.locale).to_owned(),
            )),
            "",
            |choice| Message::Wizard(Msg::Locale(choice.0)),
            Length::Fixed(PICKER),
            theme::BODY,
        ),
    ));
    // Asked, not guessed: the same OS runs a handheld, a box under a
    // television, and a desktop, and nothing on the machine says which.
    if app.steamos {
        let picked = app.config.steamos_use;
        let answer = |label: &'static str, value: SteamOsUse| {
            clickable(
                format!("steamos {value:?}"),
                iced::widget::radio(label, value, picked, |answer| {
                    Message::Wizard(Msg::SteamOsUse(answer))
                })
                .text_size(theme::tap_text(theme::BODY)),
            )
        };
        body = body.push(
            column![
                text("How will you use Lightforge on this SteamOS device?").size(theme::BODY),
                answer("Steam Deck or Steam Machine: Controller Input", SteamOsUse::Console),
                answer("SteamOS on a desktop PC: Mouse and Keyboard Input", SteamOsUse::Desktop),
            ]
            .spacing(theme::SM),
        );
    }

    body = body.extend(
        app.hint(
            "The region decides which of Blizzard's version services a download comes from, \
             and the language decides which of a build's files are fetched. Both are written \
             into the install, so changing the language later means downloading that \
             language's files.",
        )
        .map(Element::from),
    );

    body.into()
}

fn place<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut body = column![text(
        "The Wine prefix and Battle.net live in one place, on your home drive. Games can go \
         on any drive you add."
    )
    .size(theme::BODY)]
    .spacing(theme::MD);

    body = body.push(field_row(
        "Install to".to_owned(),
        row![
            clickable(
                "wizard location",
                text_input("~/Games/Blizzard", &app.location_input)
                    .on_input(Message::LocationChanged)
                    .on_submit(Message::UseLocation)
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
            secondary("Use this", Message::UseLocation).enabled(app.library_empty),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center)
        .into(),
    ));

    let mut drives = column![muted("Drives for games".to_owned(), theme::LABEL)].spacing(theme::SM);
    for library in &app.config.libraries {
        let free = match library.disk() {
            Some(disk) => format!("{} free", size_of(disk.free)),
            None => "not mounted".to_owned(),
        };
        let mut facts = column![text(library.label.clone()).size(theme::BODY)].spacing(theme::XS);
        facts = facts.push(mono(format!("{} · {free}", library.games().display())));
        let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);
        match library.is_default {
            true => actions = actions.push(muted("default".to_owned(), theme::LABEL)),
            false => {
                actions = actions.push(
                    secondary("Make default", Message::SetDefaultDrive(library.id.clone()))
                        .enabled(true),
                )
            }
        }
        // Same two refusals as the Storage page: the default drive goes only
        // after another one is made default, and the last one never goes at
        // all, since every path resolves through it.
        actions = actions.push(
            secondary("Remove", Message::Wizard(Msg::RemoveDrive(library.id.clone())))
                .enabled(!library.is_default && app.config.libraries.len() > 1),
        );
        drives = drives.push(
            container(
                row![facts.width(Length::Fill), actions].spacing(theme::MD).align_y(Alignment::Center),
            )
            .padding(theme::SM)
            .style(theme::row),
        );
    }

    for (index, partition) in app.partitions.iter().enumerate() {
        if partition.in_use {
            continue;
        }
        let free = partition
            .disk
            .map(|disk| format!("{} free", size_of(disk.free)))
            .unwrap_or_else(|| partition.fstype.clone());
        drives = drives.push(
            container(
                row![
                    column![
                        text(partition.label.clone()).size(theme::BODY),
                        mono(format!("{} · {free}", partition.point.display())),
                    ]
                    .spacing(theme::XS)
                    .width(Length::Fill),
                    secondary("Add", Message::Wizard(Msg::AddDrive(index))).enabled(true),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
            .padding(theme::SM)
            .style(theme::row),
        );
    }

    body = body.push(focus::region(
        "wizard drives",
        scrollable(drives)
            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
            .style(theme::scrollbar),
    ));
    if let Some(error) = &wizard.drive_error {
        body = body.push(muted(error.clone(), theme::CAPTION));
    }
    body = body.extend(
        app.hint(
            "Adding a drive creates a Lightforge folder on it. Nothing is moved, and removing              a drive again deletes nothing.",
        )
        .map(Element::from),
    );
    body.into()
}

fn found<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut body = column![].spacing(theme::MD);

    if wizard.scanning {
        body = body.push(text("Looking through the Wine prefixes on this machine…").size(theme::BODY));
        body = body.extend(
            app.hint(
                "Only where a Battle.net prefix is normally kept — Lutris, Bottles, Heroic, \
                 Faugus, Steam and ~/.wine. Nothing leaves this machine.",
            )
            .map(Element::from),
        );
        return body.into();
    }

    // A scan from Settings has no next slide to report on, so what adding
    // did is said here, above the rows it was about.
    if wizard.scan_only {
        body = body.extend(wizard.adopted.iter().map(note));
    }

    body = body.push(
        text(match wizard.rows.len() {
            0 => "No Battle.net games found".to_owned(),
            1 => "1 Battle.net game found".to_owned(),
            n => format!("{n} Battle.net games found"),
        })
        .size(theme::BODY),
    );

    let mut list = column![].spacing(theme::SM);
    for (index, row) in wizard.rows.iter().enumerate() {
        let (name, detail) = match &row.kind {
            RowKind::Wow(install) => (
                format!(
                    "World of Warcraft — {}",
                    install
                        .products
                        .iter()
                        .map(|code| code.short_name().to_owned())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                install.path.display().to_string(),
            ),
            RowKind::Game(game) => (game.label.clone(), game.path.display().to_string()),
        };
        let mut facts = column![
            text(name).size(theme::BODY),
            mono(detail),
            muted(
                format!("{} · found in {}", size_of(row.bytes()), row.origin.label()),
                theme::CAPTION,
            ),
        ]
        .spacing(theme::XS);
        if let Some(blocked) = &row.blocked {
            facts = facts.push(muted(blocked.clone(), theme::CAPTION));
        }
        if let RowKind::Wow(install) = &row.kind {
            if install.needs_rename() {
                facts = facts.extend(
                    app.hint(format!(
                        "its folder will be renamed to \"{}\", which is where a library keeps \
                         an install",
                        lightforge_core::paths::WOW_DIR
                    ))
                    .map(Element::from),
                );
            }
        }
        if matches!(row.kind, RowKind::Game(_)) {
            facts = facts.extend(
                app.hint(
                    "Battle.net's to update, not Lightforge's — ours is pointed at it where it \
                     is.",
                )
                .map(Element::from),
            );
        }

        let tick: Element<Message> = match row.blocked.is_none() {
            true => clickable(
                format!("wizard take {index}"),
                crate::tick(row.take)
                    .on_toggle(move |on| Message::Wizard(Msg::Take(index, on))),
            ),
            false => Space::new().width(Length::Fixed(theme::MD)).into(),
        };
        list = list.push(
            container(
                row![tick, facts.width(Length::Fill)].spacing(theme::MD).align_y(Alignment::Center),
            )
            .padding(theme::SM)
            .style(theme::row),
        );
    }
    body = body.push(focus::region(
        "wizard found",
        scrollable(list)
            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
            .style(theme::scrollbar),
    ));

    // A text box is one line of its text plus its padding; the buttons beside
    // it are set to that, since `Fill` would take the whole page's height.
    // A console's taller buttons are already the bigger of the two.
    let box_height = theme::BODY * 1.3 + theme::SM * 2.0;
    let beside = |label: &str, message: Message| match theme::console() {
        true => secondary(label, message),
        false => crate::widgets::secondary_tall(label, message, box_height),
    };
    body = body.push(field_row(
        "Somewhere else".to_owned(),
        row![
            clickable(
                "wizard pointed",
                text_input("~/Games/battlenet, or a World of Warcraft folder", &wizard.pointed)
                    .on_input(|path| Message::Wizard(Msg::PointInput(path)))
                    .on_submit(Message::Wizard(Msg::Point))
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
            beside("Browse…", Message::Wizard(Msg::Browse)).enabled(true),
            beside(if wizard.pointing { "Looking…" } else { "Look here" }, Message::Wizard(Msg::Point))
                .enabled(!wizard.pointed.is_empty() && !wizard.pointing),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center)
        .into(),
    ));
    if let Some(error) = &wizard.point_error {
        body = body.push(muted(error.clone(), theme::CAPTION));
    }
    body.into()
}

/// One line about what adding an install did.
fn note(note: &Note) -> Element<'_, Message> {
    match note.level {
        Level::Done => muted(note.text.clone(), theme::CAPTION).into(),
        // Loud on purpose. A game that was not added is the one thing on
        // this slide the user has to act on, and it used to look exactly
        // like the receipts around it.
        Level::Warning => text(note.text.clone()).size(theme::LABEL).color(theme::PAUSE).into(),
        Level::Failed => text(note.text.clone()).size(theme::BODY).color(theme::ERROR).into(),
    }
}

fn install<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut body = column![].spacing(theme::MD);

    body = body.extend(wizard.adopted.iter().map(note));

    body = body.push(
        text("Install Proton-GE, the Wine prefix and Battle.net (opt-out possible).")
            .size(theme::BODY),
    );

    body = body.push(clickable(
        "wizard battle.net",
        crate::tick(wizard.battle_net)
            .label("Install Battle.net")
            .on_toggle(|on| Message::Wizard(Msg::WantBattleNet(on)))
            .text_size(theme::tap_text(theme::BODY)),
    ));
    if !wizard.battle_net {
        body = body.push(muted(
            "Important! Only World of Warcraft games will work for now. You can install \
             Battle.net at any time later in the Settings menu."
                .to_owned(),
            theme::CAPTION,
        ));
    }

    // The same section Settings shows, dots and live log and all: there is one
    // setup and it should look like one thing in both places.
    body = body.push(app.setup_section_in_wizard());
    body.into()
}

fn games<'a>(app: &'a Lightforge) -> Element<'a, Message> {
    let mut body = column![text(
        "Select the Blizzard games you play. You can add more later on."
    )
    .size(theme::BODY)]
    .spacing(theme::MD);

    let tracked = |code: &ProductCode| app.choices.iter().any(|c| &c.code == code);
    let mut list = column![].spacing(theme::SM);

    let row_for = |entry: &lightforge_core::product::CatalogEntry| -> Element<Message> {
        let mark: Element<Message> = match app.icons.get(&entry.code) {
            Some(mark) => image(mark.handle.clone())
                .content_fit(ContentFit::Contain)
                .width(Length::Fixed(28.0))
                .height(Length::Fixed(28.0))
                .into(),
            None => Space::new().width(Length::Fixed(28.0)).into(),
        };
        let mut facts = column![text(entry.label.clone()).size(theme::BODY)];
        if app.installed_versions.contains_key(entry.code.as_str()) {
            facts = facts.push(muted("installed".to_owned(), theme::CAPTION));
        }
        let key = format!("game {}", entry.code.as_str());
        let action: Element<Message> = match tracked(&entry.code) {
            true => secondary("Remove", Message::RemoveProduct(entry.code.clone()))
                .keyed(key, app.choices.len() > 1),
            false => secondary("Add", Message::AddProduct(entry.code.clone())).keyed(key, true),
        };
        container(
            row![mark, facts.width(Length::Fill), action]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
        )
        .padding(theme::SM)
        .style(theme::row)
        .into()
    };

    list = list.push(muted("World of Warcraft".to_owned(), theme::LABEL));
    for entry in app.catalog.managed().filter(|e| e.branch == Branch::Live) {
        list = list.push(row_for(entry));
    }

    match app.config.battle_net_declined {
        true => {
            list = list.push(muted("Other Blizzard games".to_owned(), theme::LABEL));
            list = list.push(muted(
                "Battle.net is not installed, so there is nothing to hand these to.".to_owned(),
                theme::CAPTION,
            ));
        }
        false => {
            list = list.push(muted("Other Blizzard games".to_owned(), theme::LABEL));
            for entry in app.catalog.hand_offs().filter(|e| e.kind == Kind::HandOff) {
                list = list.push(row_for(entry));
            }
        }
    }

    body = body.push(focus::region(
        "wizard games",
        scrollable(list)
            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
            .style(theme::scrollbar),
    ));
    body.into()
}

fn done<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let ready = app.setup.as_ref().is_some_and(|status| status.is_ready());
    let mut body = column![text(match ready {
        true => "That is everything. Pick a game and press Install.",
        false => "You can start from here. Settings › Setup finishes whatever is still \
                  outstanding, and nothing is lost by doing it later.",
    })
    .size(theme::BODY)]
    .spacing(theme::MD);

    if app.console() {
        body = body.push(muted(
            "Lightforge goes into your Steam library as we close this, with its own \
             artwork. It shows there the next time Steam starts."
                .to_owned(),
            theme::CAPTION,
        ));
    }

    if !wizard.pending.is_empty() {
        body = body.push(muted(
            format!(
                "{} will be pointed out to Battle.net as we close this.",
                wizard
                    .pending
                    .iter()
                    .map(|(game, _)| game.label.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            theme::CAPTION,
        ));
    }

    body = body.push(
        text(
            "Found a bug, have any questions or want to send feedback? Contact me on \
             Discord or X, or use the options in the Lightforge logo dropdown.",
        )
        .size(theme::BODY),
    );

    // The same two marks the logo menu carries, and dimmed by the same rule:
    // no destination yet means no press. They sit in the middle of the slide
    // because this is the one page whose job is to hand the user somewhere.
    body = body.push(
        container(
            row![
                mark(crate::discord_icon(), crate::DISCORD_URL),
                mark(crate::x_icon(), crate::X_URL),
            ]
            .spacing(theme::MD),
        )
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(theme::LG),
    );

    // Asked once, at the end of the one flow where the work is visibly done,
    // and never again anywhere a person is trying to get something finished.
    // Below the rule and below the marks for getting help, because help comes
    // first: this slide's job is to hand someone somewhere, and this is the
    // least important of the places.
    body = body.push(
        container(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(theme::divider),
    );
    body = body.push(
        container(
            column![
                text(
                    "Lightforge is free and always will be. If it saved you an evening, \
                     you can buy me a coffee.",
                )
                .size(theme::LABEL)
                .color(theme::MUTED)
                .center(),
                row![
                    mark(crate::kofi_icon(), crate::KOFI_URL),
                    mark(crate::paypal_icon(), crate::PAYPAL_URL),
                ]
                .spacing(theme::MD),
            ]
            .spacing(theme::SM)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(theme::MD),
    );
    body.into()
}

/// One of the last slide's marks, as a place for the ring.
fn mark(icon: iced::widget::image::Handle, url: Option<&'static str>) -> Element<'static, Message> {
    let press = url.map_or(focus::Press::Off, |url| focus::Press::Send(Message::OpenLink(url)));
    focus::ring(focus::Target::Link(url.unwrap_or_default()), press, crate::link_square(icon, url, MARK))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn install(path: &str, blocked: Option<&str>) -> WowInstall {
        WowInstall {
            path: PathBuf::from(path),
            products: vec![ProductCode::Retail],
            bytes: 1024,
            blocked: blocked.map(str::to_owned),
        }
    }

    fn game(label: &str) -> HandOff {
        HandOff {
            code: "hs_beta".to_owned(),
            label: label.to_owned(),
            path: PathBuf::from("/games").join(label),
            version: None,
            bytes: 2048,
        }
    }

    /// Only SteamOS is asked, and there the first slide waits for an answer.
    #[test]
    fn steamos_has_to_say_what_it_is_for_before_setup_goes_on() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.config.steamos_use = None;

        app.steamos = false;
        assert!(welcome_answered(&app), "a desktop distro was held on a SteamOS question");

        app.steamos = true;
        assert!(!welcome_answered(&app));
        let _ = update(&mut app, Msg::SteamOsUse(SteamOsUse::Console));
        assert_eq!(app.config.steamos_use, Some(SteamOsUse::Console));
        assert!(welcome_answered(&app));
    }

    /// Setup by keys: the first key lands on the slide's own button, Enter
    /// walks the slides, and Escape steps back and never leaves.
    #[test]
    fn setup_walks_by_enter_and_steps_back_by_escape() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.splash = None;
        app.steamos = false;
        app.wizard = Some(Wizard::default());
        let at = |x, y| iced::Rectangle { x, y, width: 120.0, height: 30.0 };
        let send = |message| focus::Press::Send(message);

        let slide = |app: &Lightforge| {
            focus::reset();
            let wizard = app.wizard.as_ref().unwrap();
            focus::put(focus::Target::Key("picker wizard region".to_owned()), at(200.0, 200.0), send(Message::MenuClick), 0);
            focus::put(forward(wizard), at(600.0, 600.0), send(Message::Wizard(match wizard.step {
                Step::Done => Msg::Finish,
                _ => Msg::Next,
            })), 0);
        };

        slide(&app);
        let _ = app.update(Message::FocusStep(focus::Step::Down));
        assert_eq!(app.focus, Some(forward(app.wizard.as_ref().unwrap())));
        let _ = app.update(Message::FocusPress);
        assert_eq!(app.wizard.as_ref().unwrap().step, Step::Place);

        let _ = app.update(Message::FocusBack);
        assert_eq!(app.wizard.as_ref().unwrap().step, Step::Welcome);
        let _ = app.update(Message::FocusBack);
        assert!(app.wizard.is_some(), "Escape on the first slide left setup");

        // Opened from Settings, where Escape otherwise means "back to Play".
        app.screen = crate::Screen::Settings;
        let _ = app.update(Message::FocusBack);
        assert!(app.wizard.is_some(), "Escape in setup over Settings left setup");
        assert_eq!(app.screen, crate::Screen::Settings);
    }

    /// A second look lists what is new, and greys out what Lightforge
    /// already plays: the WoW install in one of its libraries, and the game
    /// already linked into the default one.
    #[test]
    fn a_scan_after_setup_does_not_offer_what_is_already_here() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        let root = std::env::temp_dir().join(format!("lightforge-wizard-known-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let games = root.join("games");
        std::fs::create_dir_all(&games).unwrap();
        std::os::unix::fs::symlink(game("Hearthstone").path, games.join("Hearthstone")).unwrap();

        let library = lightforge_core::library::Library::main_at(games.clone(), "Main");
        let mut retail = lightforge_core::Product::new(ProductCode::Retail, "Retail");
        retail.profile.library = Some(library.id.clone());
        app.config.libraries = vec![library];
        app.config.products = vec![retail];
        app.wizard = Some(Wizard { step: Step::Found, scanning: true, scan_only: true, ..Wizard::default() });

        let found = Found {
            prefix: PathBuf::from("/lutris/pfx"),
            origin: Origin::Pointed,
            battle_net: None,
            wow: vec![
                install(games.join("World of Warcraft").to_str().unwrap(), None),
                install("/elsewhere/World of Warcraft", None),
            ],
            hand_offs: vec![game("Hearthstone"), game("Diablo IV")],
        };
        let _ = update(&mut app, Msg::Scanned(vec![found]));

        let rows = &app.wizard.as_ref().unwrap().rows;
        let had: Vec<bool> = rows.iter().map(|row| row.blocked.as_deref() == Some("already in Lightforge")).collect();
        assert_eq!(had, [true, false, true, false]);
        assert!(rows[1].take && rows[3].take, "what is new is offered ticked");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_typed_folder_is_looked_through_once_and_off_the_ui_thread() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.wizard = Some(Wizard { pointed: "/nowhere/World of Warcraft".to_owned(), ..Wizard::default() });

        let _ = update(&mut app, Msg::Point);
        assert!(app.wizard.as_ref().unwrap().pointing, "running in the background");
        let _ = update(&mut app, Msg::Point);
        assert!(app.wizard.as_ref().unwrap().pointing, "a second click starts nothing new");

        let _ = update(&mut app, Msg::Pointed(None));
        let wizard = app.wizard.as_ref().unwrap();
        assert!(!wizard.pointing);
        assert!(wizard.point_error.as_deref().is_some_and(|e| e.starts_with("nothing of Blizzard's")));
    }

    /// Opened from Settings, the slide closes on Back, and closing it is not
    /// finishing setup: the release notes stay owed.
    #[test]
    fn closing_a_scan_leaves_the_first_run_state_alone() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.config.patch_notes_seen = Some("0.0.1".to_owned());
        let _ = update(&mut app, Msg::Scan);
        let wizard = app.wizard.as_ref().unwrap();
        assert!(wizard.scan_only && wizard.scanning);
        assert_eq!(wizard.step, Step::Found);

        let _ = update(&mut app, Msg::Back);

        assert!(app.wizard.is_none());
        assert_eq!(app.config.patch_notes_seen.as_deref(), Some("0.0.1"));
    }

    #[test]
    fn the_steps_run_from_welcome_to_done_and_stop_there() {
        assert_eq!(Step::ALL.first(), Some(&Step::Welcome));
        assert_eq!(Step::ALL.last(), Some(&Step::Done));
        assert!(Step::Welcome.back().is_none(), "nothing before the first slide");
        assert!(Step::Done.next().is_none(), "nothing after the last");

        // Every step is reachable from the one before it, in order.
        let mut step = Step::Welcome;
        for expected in &Step::ALL[1..] {
            step = step.next().expect("another step");
            assert_eq!(&step, expected);
            assert_eq!(step.back().unwrap().next().unwrap(), step);
        }
    }

    /// Battle.net calls Hearthstone `hs_beta` on a machine where it is
    /// installed; the catalog ships it as `hsb`. Adding it under the code on
    /// disk when the catalog holds the other one is a tile with no artwork
    /// that never matches what is installed.
    #[test]
    fn a_found_game_is_matched_to_the_catalog_even_under_another_code() {
        let catalog = lightforge_core::product::Catalog::builtin();
        let hearthstone = catalog
            .hand_offs()
            .find(|e| e.label == "Hearthstone")
            .expect("the catalog ships Hearthstone");

        // The code the catalog itself uses resolves to itself.
        let exact = game_named(hearthstone.code.as_str(), "Hearthstone");
        assert_eq!(hand_off_entry(&catalog, &exact), (hearthstone.code.clone(), "Hearthstone".to_owned()));

        // And the client's own spelling lands on the same entry, by family.
        for other in ["hs_beta", "hsb"] {
            let found = game_named(other, "Hearthstone");
            let (code, label) = hand_off_entry(&catalog, &found);
            assert_eq!(label, "Hearthstone", "{other} should be Hearthstone");
            assert_eq!(
                lightforge_core::battlenet::game_uid(&code),
                lightforge_core::battlenet::game_uid(&hearthstone.code),
                "{other} should resolve to the same game family"
            );
        }
    }

    /// A game nothing knows about is still the user's game, so it goes in
    /// under the folder name they would recognise.
    #[test]
    fn a_game_the_catalog_has_never_heard_of_keeps_its_folder_name() {
        let catalog = lightforge_core::product::Catalog::builtin();
        let found = game_named("something_new", "Some New Game");
        let (code, label) = hand_off_entry(&catalog, &found);
        assert_eq!(code.as_str(), "something_new");
        assert_eq!(label, "Some New Game");
    }

    fn game_named(code: &str, label: &str) -> HandOff {
        HandOff {
            code: code.to_owned(),
            label: label.to_owned(),
            path: PathBuf::from("/games").join(label),
            version: None,
            bytes: 0,
        }
    }

    #[test]
    fn a_scan_becomes_one_ticked_row_per_thing_found() {
        let found = Found {
            prefix: PathBuf::from("/home/u/Games/battlenet/pfx"),
            origin: Origin::Lutris,
            battle_net: None,
            wow: vec![install("/mnt/nvme2/Games/World of Warcraft", None)],
            hand_offs: vec![game("Hearthstone")],
        };
        let rows = rows_of(vec![found]);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.take), "what is here is taken by default");
        assert_eq!(rows[0].bytes(), 1024);
        assert_eq!(rows[1].bytes(), 2048);
        // The prefix rides along on every row: a hand-off's record is lifted
        // out of the registry in the prefix it was found in.
        assert!(rows.iter().all(|row| row.prefix.ends_with("pfx")));
    }

    #[test]
    fn an_install_that_cannot_be_adopted_is_shown_but_not_ticked() {
        let found = Found {
            prefix: PathBuf::new(),
            origin: Origin::Pointed,
            battle_net: None,
            wow: vec![install("/mnt/x/World of Warcraft", Some("already added"))],
            hand_offs: Vec::new(),
        };
        let rows = rows_of(vec![found]);
        assert_eq!(rows.len(), 1, "it is still listed, so the user can see why");
        assert!(!rows[0].take);
        assert_eq!(rows[0].blocked.as_deref(), Some("already added"));
    }
}
