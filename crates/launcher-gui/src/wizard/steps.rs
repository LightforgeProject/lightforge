//! The body of each step.

use super::view::{mark, note};
use super::*;
// Named, not globbed: std's own `column!` would otherwise win the glob.
use iced::widget::column;

/// How wide the region and language pickers are: the longest locale name.
const PICKER: f32 = 300.0;

/// Whether the first slide can be left. On SteamOS the Deck-or-desktop
/// question has to be answered first.
pub(super) fn welcome_answered(app: &Lightforge) -> bool {
    !app.steamos || app.config.steamos_use.is_some()
}

pub(super) fn welcome(app: &Lightforge) -> Element<'_, Message> {
    let mut body = column![
        text(
            "Lightforge installs and launches World of Warcraft on Linux itself, and hands \
             everything else to Battle.net. This takes a couple of minutes, and nothing is \
             downloaded until you say so."
        )
        .size(theme::BODY),
    ]
    .spacing(theme::MD);

    // Said before 400 MB of Proton: the Battle.net installer is 32-bit.
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

pub(super) fn place<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
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
        // As on the Storage page: not the default, never the last.
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

pub(super) fn found<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
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

pub(super) fn install<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
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

pub(super) fn games<'a>(app: &'a Lightforge) -> Element<'a, Message> {
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

pub(super) fn done<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
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

    // The logo menu's two marks, dimmed without a destination.
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

    // Asked once, here, below the help links.
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
