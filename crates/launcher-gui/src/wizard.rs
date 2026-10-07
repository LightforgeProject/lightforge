//! The first run.
//!
//! Everything a machine needs before a game can be played, in the order it has
//! to happen, as one flow instead of a Settings page the user has to find:
//! where things go, what is already here, provisioning, which games to keep in
//! the band, done.
//!
//! It is a layer over the app rather than a screen of its own — the same
//! [`Lightforge`] state, the same messages where they already exist. The Place
//! step *is* Settings' install-location field and the Storage page's drive
//! list, the Install step *is* [`crate::provision`], and the Games step is the
//! add-a-game list. Only the three things with nowhere else to live are new:
//! the artwork prefetch, the scan for installs that were here first, and the
//! choice to do without Battle.net.

use std::path::PathBuf;

use iced::widget::{column, container, image, row, scrollable, text, text_input, Space};
use iced::{Alignment, ContentFit, Element, Length};

use lightforge_core::discover::{self, Found, HandOff, Origin, WowInstall};
use lightforge_core::product::{Branch, Kind, ProductCode};
use lightforge_core::tact::install::LOCALES;
use lightforge_core::tact::version::Region;

use crate::theme;
use crate::{
    action_button, field_row, mono, muted, secondary, size_of, Lightforge, Message,
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
    /// Every step, in order — the dots across the top count these.
    pub const ALL: [Step; 6] =
        [Step::Welcome, Step::Place, Step::Found, Step::Install, Step::Games, Step::Done];

    fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome to Lightforge",
            Self::Place => "Where things go",
            Self::Found => "What is already here",
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
    /// A game only Battle.net can install — ours can be pointed at it.
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
    /// Whether the scan has run at all — an empty `rows` means something quite
    /// different before and after.
    pub scanned: bool,
    pub rows: Vec<Row>,
    /// A folder the user typed in themselves.
    pub pointed: String,
    pub point_error: Option<String>,
    /// Whether Battle.net is wanted. Off means WoW only.
    pub battle_net: bool,
    /// What adoption did, one line each, shown on the way out of the step.
    pub adopted: Vec<String>,
    /// Hand-offs to link once the prefix and Battle.net exist.
    pub pending: Vec<(HandOff, PathBuf)>,
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
            battle_net: true,
            adopted: Vec::new(),
            pending: Vec::new(),
        }
    }
}

/// Everything the wizard can be told, beside the app messages it reuses.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Open it again from Settings.
    Open,
    Next,
    Back,
    /// Leave it, and do not open it again by itself.
    Later,
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
    WantBattleNet(bool),
    /// Done — write the config and get out of the way.
    Finish,
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

        Msg::Later | Msg::Finish => {
            let finishing = matches!(msg, Msg::Finish);
            let pending = app.wizard.as_ref().map(|w| w.pending.clone()).unwrap_or_default();
            app.wizard = None;
            // Both ways out mark the first run as seen: someone who dismissed
            // the wizard has seen it, and meeting it again on the next start
            // would be nagging. Settings keeps the button.
            app.config.setup_complete = true;
            app.save_config();
            if finishing && !pending.is_empty() {
                link_hand_offs(app, &pending);
            }
            app.go_to(crate::Screen::Dashboard);
            // The band and the panel both change shape when a game is adopted
            // or Battle.net is declined, so the product on screen is re-read.
            app.reload(app.selected.code.clone())
        }

        Msg::Next => advance(app),

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

        Msg::Point => {
            let Some(wizard) = app.wizard.as_mut() else {
                return iced::Task::none();
            };
            let path = match crate::parse_path(&wizard.pointed) {
                Ok(path) => path,
                Err(e) => {
                    wizard.point_error = Some(e);
                    return iced::Task::none();
                }
            };
            match discover::inspect_pointed(&path) {
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
                true => {
                    let prefix = app.layout.prefix();
                    iced::Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || discover::scan(&prefix))
                                .await
                                .unwrap_or_default()
                        },
                        |found| Message::Wizard(Msg::Scanned(found)),
                    )
                }
                false => iced::Task::none(),
            }
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

/// Adopt every ticked WoW install, and remember the ticked hand-offs for later.
///
/// In place, in the time it takes to write a marker file: the install's parent
/// directory becomes a storage library and the products in it are assigned to
/// it. Nothing is copied and nothing is verified — the files are already there,
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
            RowKind::Wow(install) => {
                match adopt_one(app, install) {
                    Ok(note) => notes.push(note),
                    Err(e) => notes.push(format!("{} — not adopted: {e}", install.path.display())),
                }
            }
            RowKind::Game(game) => pending.push((game.clone(), row.prefix.clone())),
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

fn adopt_one(app: &mut Lightforge, install: &WowInstall) -> Result<String, String> {
    let plan = discover::plan(install, &app.config.libraries).map_err(|e| e.to_string())?;
    let library = discover::adopt(&plan, &app.layout).map_err(|e| e.to_string())?;
    let id = library.id.clone();
    let label = library.label.clone();
    app.config.libraries.push(library);

    for code in &plan.products {
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
        let _ = lightforge_core::prefix::link_libraries(&app.layout);
    }
    Ok(format!(
        "{} — kept where it is, on {label}",
        plan.products
            .iter()
            .map(|code| code.short_name().to_owned())
            .collect::<Vec<_>>()
            .join(", ")
    ))
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
            // Linked, but Battle.net was not told — it will offer to find the
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
        Step::Welcome => welcome(app, wizard),
        Step::Place => place(app),
        Step::Found => found(app, wizard),
        Step::Install => install(app, wizard),
        Step::Games => games(app),
        Step::Done => done(app, wizard),
    };

    let card = column![
        dots(wizard.step),
        text(wizard.step.title()).size(theme::TITLE),
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

/// Back, the step's own button, and the way out.
fn footer<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);

    if wizard.step.back().is_some() && wizard.step != Step::Done {
        actions = actions.push(secondary("Back", Message::Wizard(Msg::Back)).enabled(!app.busy));
    }
    // The way out sits with Back rather than under the step's own button: it
    // is the quiet choice, and a column of two buttons in the corner reads as
    // one decision with two answers, which this is not.
    if wizard.step != Step::Done {
        actions =
            actions.push(secondary("Set up later", Message::Wizard(Msg::Later)).enabled(true));
    }
    actions = actions.push(Space::new().width(Length::Fill));

    // What the step's own button says, and whether it can be pressed yet.
    let (label, ready): (&str, bool) = match wizard.step {
        Step::Welcome => ("Get started", true),
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

fn welcome<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
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
        iced::widget::pick_list(regions, Some(RegionChoice(app.config.region)), |choice| {
            Message::Wizard(Msg::Region(choice.0))
        })
        .text_size(theme::BODY)
        .padding(theme::SM)
        .into(),
    ));
    body = body.push(field_row(
        "Language".to_owned(),
        iced::widget::pick_list(
            locales,
            Some(LocaleChoice(app.config.locale.clone())),
            |choice| Message::Wizard(Msg::Locale(choice.0)),
        )
        .text_size(theme::BODY)
        .padding(theme::SM)
        .into(),
    ));
    body = body.extend(
        app.hint(
            "The region decides which of Blizzard's version services a download comes from, \
             and the language decides which of a build's files are fetched. Both are written \
             into the install, so changing the language later means downloading that \
             language's files.",
        )
        .map(Element::from),
    );

    body = body.push(Space::new().height(Length::Fill));
    body = body.push(mono(match wizard.prefetched {
        None => "fetching artwork in the background…".to_owned(),
        Some(0) => "no artwork available — the launcher still works".to_owned(),
        Some(n) => format!("artwork ready for {n} game(s)"),
    }));
    body.into()
}

fn place<'a>(app: &'a Lightforge) -> Element<'a, Message> {
    let mut body = column![text(
        "The Wine prefix and Battle.net live in one place, on your home drive. Games can go \
         on any drive you add — a 120 GB install does not have to be on the system disk."
    )
    .size(theme::BODY)]
    .spacing(theme::MD);

    body = body.push(field_row(
        "Install to".to_owned(),
        row![
            text_input("~/Games/Blizzard", &app.location_input)
                .on_input(Message::LocationChanged)
                .on_submit(Message::UseLocation)
                .style(theme::field)
                .size(theme::BODY)
                .padding(theme::SM),
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
                    secondary("Add", Message::AddDrive(index)).enabled(true),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
            .padding(theme::SM)
            .style(theme::row),
        );
    }

    body = body.push(
        scrollable(drives)
            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
            .style(theme::scrollbar),
    );
    body = body.extend(
        app.hint("Adding a drive creates a Lightforge folder on it. Nothing is moved.")
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
                 Steam and ~/.wine. Nothing leaves this machine.",
            )
            .map(Element::from),
        );
        return body.into();
    }

    let takeable = wizard.rows.iter().any(|row| row.blocked.is_none());
    body = body.push(
        text(match (wizard.rows.is_empty(), takeable) {
            (true, _) => "No other Blizzard install found. Nothing is missing — Lightforge \
                          downloads World of Warcraft itself."
                .to_owned(),
            (false, _) => format!(
                "Found {} install(s) already on this machine. Anything you keep stays exactly \
                 where it is — nothing is copied or moved.",
                wizard.rows.len()
            ),
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
            true => iced::widget::checkbox(row.take)
                .on_toggle(move |on| Message::Wizard(Msg::Take(index, on)))
                .into(),
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
    body = body.push(
        scrollable(list)
            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
            .style(theme::scrollbar),
    );

    body = body.push(field_row(
        "Somewhere else".to_owned(),
        row![
            text_input("~/Games/battlenet, or a World of Warcraft folder", &wizard.pointed)
                .on_input(|path| Message::Wizard(Msg::PointInput(path)))
                .on_submit(Message::Wizard(Msg::Point))
                .style(theme::field)
                .size(theme::BODY)
                .padding(theme::SM),
            secondary("Look here", Message::Wizard(Msg::Point)).enabled(!wizard.pointed.is_empty()),
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

fn install<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut body = column![].spacing(theme::MD);

    for note in &wizard.adopted {
        body = body.push(muted(note.clone(), theme::CAPTION));
    }

    body = body.push(
        text(
            "Proton-GE, the Wine prefix, and Battle.net. Each one is skipped if it is already \
             done, and you can leave this running while you read.",
        )
        .size(theme::BODY),
    );

    body = body.push(
        iced::widget::checkbox(wizard.battle_net)
            .label("Install Battle.net")
            .on_toggle(|on| Message::Wizard(Msg::WantBattleNet(on)))
            .text_size(theme::BODY),
    );
    if !wizard.battle_net {
        body = body.push(muted(
            "Without it only World of Warcraft works, which is experimental — the in-game \
             shop and support browser need the client. You can install it later from \
             Settings › Setup."
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
        "Which games sit in the band at the top. Add or remove them here or later — the band \
         has a + of its own."
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
        let action: Element<Message> = match tracked(&entry.code) {
            true => secondary("Remove", Message::RemoveProduct(entry.code.clone()))
                .enabled(app.choices.len() > 1)
                .into(),
            false => secondary("Add", Message::AddProduct(entry.code.clone())).enabled(true).into(),
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

    body = body.push(
        scrollable(list)
            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
            .style(theme::scrollbar),
    );
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
            "Found a bug, or want to ask for something? Report a Bug and Send Feedback are \
             both under the Lightforge mark in the top left, next to the Discord link.",
        )
        .size(theme::BODY),
    );
    body = body.push(Space::new().height(Length::Fill));
    body = body.push(mono(format!(
        "{} · games on {} drive(s)",
        app.layout.root().display(),
        app.config.libraries.len()
    )));
    body.into()
}

/// A region as a `pick_list` option; the widget needs `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegionChoice(Region);

impl std::fmt::Display for RegionChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.0.label(), self.0.as_str())
    }
}

/// A locale as a `pick_list` option, named in its own language.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocaleChoice(String);

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
