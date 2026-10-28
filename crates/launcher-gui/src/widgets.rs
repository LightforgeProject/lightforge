//! The shared building blocks every screen draws with, and their sizes.

use crate::*;
use iced::widget::column;

/// How wide a centred message card gets: a readable measure for prose.
pub(crate) const MESSAGE_WIDTH: f32 = 720.0;

/// Width of the size column on the Storage page, so figures right-align.
pub(crate) const SIZE_COLUMN: f32 = 96.0;

/// Width of the action slot beside a size, kept even where there is no button.
pub(crate) const ACTION_COLUMN: f32 = 76.0;

pub(crate) fn muted<'a>(content: String, size: f32) -> iced::widget::Text<'a> {
    text(content).size(size).color(theme::MUTED)
}

/// Centre a line of text on its *letters* rather than on its line box.
///
/// Centring the line box leaves the empty descent below, so a word without
/// descenders (`GAMES`) sits high. The line height collapses to the glyphs,
/// and padding above takes the half-descent, at any size.
pub(crate) fn optically_centred<'a>(text: iced::widget::Text<'a>) -> Element<'a, Message> {
    container(text.line_height(iced::widget::text::LineHeight::Relative(1.0)))
        // Top only. Padding on both sides would grow the box symmetrically and
        // move the letters nowhere.
        .padding(iced::Padding { top: OPTICAL_NUDGE, right: 0.0, bottom: 0.0, left: 0.0 })
        .into()
}

/// Shortest query worth sending to the providers while typing.
pub(crate) const SEARCH_MIN: usize = 2;

/// How long the search box waits for the next keystroke before going out.
pub(crate) const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

/// How far down a descenderless line is pushed to sit on the optical middle.
/// Roughly a quarter of the font's descent at label sizes; see
/// [`optically_centred`].
pub(crate) const OPTICAL_NUDGE: f32 = 2.0;

/// The key that reveals the hints, with Control, Alt and Shift held: a
/// developer aid, not a setting, and not saved.
pub(crate) const HINTS_CHORD: &str = "h";

/// Width of a band button, pinned so the band's arithmetic is exact rather
/// than an estimate of how wide a glyph renders.
pub(crate) const BAND_BUTTON: f32 = 48.0;

/// How far a held tile has to travel before it is a drag rather than a click
/// with a shaky hand.
pub(crate) const DRAG_SLOP: f32 = 6.0;

/// How many frames whatever a key press opens or closes is given to be drawn
/// before the ring stops waiting to follow it.
pub(crate) const FOCUS_ENTER_FRAMES: u8 = 10;

/// The scrolled page beside the Settings list, as a focus region.
pub(crate) const SETTINGS_PAGE: &str = "settings";

/// How many frames a jump into Settings waits for its section to be laid out.
pub(crate) const SETTINGS_JUMP_TRIES: u8 = 30;

/// How long after a jump the page is still reporting where it was before it.
pub(crate) const SETTINGS_JUMP_STALE: Duration = Duration::from_millis(400);

/// How long a jump keeps its section at the top while the page fills in.
pub(crate) const SETTINGS_JUMP_SETTLE: Duration = Duration::from_secs(3);

/// How far an arrow with nothing to move to scrolls the page instead.
pub(crate) const FOCUS_SCROLL: f32 = 160.0;

/// Ask again in a frame's time whether a menu the keyboard opened is drawn.
pub(crate) fn focus_settle() -> Task<Message> {
    // Made inside the future: a timer needs the runtime it will run on.
    Task::perform(async { tokio::time::sleep(Duration::from_millis(16)).await }, |()| {
        Message::FocusSettle
    })
}

/// Battle.net's own mark on the button at the end of the band.
///
/// Half a tile, larger than [`BAND_GLYPH`]: at 16pt it read as a favicon.
pub(crate) const BNET_GLYPH: f32 = BAND_BUTTON / 2.0;

/// Mark before a link under the action panel, a shade over its 12pt word.
pub(crate) const LINK_GLYPH: f32 = 14.0;

/// Mark beside a label in the app menu, and the social squares' mark.
pub(crate) const MENU_GLYPH: f32 = 16.0;

/// The arrow on a tab that opens a browser, trimmed to its ink: just under
/// the cap height of [`TAB_LABEL`] bold.
pub(crate) const OUTBOUND_GLYPH: f32 = 8.0;

/// How tall the wordmark is drawn in the top bar, above [`theme::TITLE`] so
/// the hammer keeps its detail.
pub(crate) const WORDMARK_HEIGHT: f32 = 28.0;

/// The navigation tabs' type, small beside the wordmark; air ([`TAB_GAP`])
/// keeps them readable.
pub(crate) const TAB_LABEL: f32 = theme::LABEL;

/// The air between two tabs in the navigation row.
///
/// Here, not in the tabs' padding: the selected pill is drawn around padding,
/// which made equal gaps look unequal.
pub(crate) const TAB_GAP: f32 = theme::LG;

/// Extra air between groups in the navigation row: wordmark, arrows, pages.
pub(crate) const GROUP_GAP: f32 = theme::XS;

/// The wordmark's width over its height, fixed by the art.
pub(crate) const WORDMARK_ASPECT: f32 = 563.0 / 72.0;

/// The wordmark as drawn, from its height and the artwork's aspect.
pub(crate) const WORDMARK_WIDTH: f32 = WORDMARK_HEIGHT * WORDMARK_ASPECT;

/// The chevron beside the wordmark, the app menu's own arrow.
pub(crate) const APP_MENU_CHEVRON: f32 = BAND_GLYPH * 0.75;

/// Width of the Settings sidebar, chosen so the rule beside it sits exactly
/// under the wordmark's chevron, the header's only vertical mark:
///
/// ```text
/// chevron centre = LG + SM + wordmark + SM + half the chevron
/// rule           = LG + this + LG          (the leading LGs cancel)
/// ```
pub(crate) const SETTINGS_NAV_WIDTH: f32 =
    theme::SM + WORDMARK_WIDTH + theme::SM + APP_MENU_CHEVRON / 2.0 - theme::LG;

/// Air between that label and its arrow, tight: it is inside the button and
/// would widen the gap beside "Battle.net Shop".
pub(crate) const OUTBOUND_GAP: f32 = 2.0;

/// How far a control that has nowhere to go is faded: visibly present, plainly
/// not available. One value so every dimmed thing dims by the same amount.
pub(crate) const DIMMED: f32 = 0.35;

/// How many status lines a bug report carries: about a minute of a busy launcher.
pub(crate) const HISTORY: usize = 20;

/// One outbound mark as a square button, Discord and X.
///
/// Shared by the logo menu and the first run's last slide: a link with no
/// destination yet draws dimmed and unclickable.
pub(crate) fn link_square(
    icon: image::Handle,
    url: Option<&'static str>,
    size: f32,
) -> iced::widget::Button<'static, Message> {
    button(
        image(icon)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .opacity(match url {
                Some(_) => 1.0_f32,
                None => DIMMED,
            }),
    )
    .padding(theme::SM)
    .style(theme::ghost)
    .on_press_maybe(url.map(Message::OpenLink))
}

/// The A and B on the console's button hints.
pub(crate) const HINT_GLYPH: f32 = 22.0;

/// A product's mark in the Add a game list.
///
/// Smaller than [`theme::TILE`], to fit inside a two-line row.
pub(crate) const BROWSE_MARK: f32 = theme::TILE * 0.6;

/// How big a mark on a band button is drawn: the box it sits in is
/// [`BAND_BUTTON`] less its padding, and a mark wider than that would push the
/// button out of the size the band's arithmetic assumes.
pub(crate) const BAND_GLYPH: f32 = BAND_BUTTON - theme::MD * 2.0;

/// Add a game, and the overflow button beside it: one mark in a fixed box, so
/// the two match and [`BAND_BUTTON`] stays true.
pub(crate) fn band_button<'a>(
    face: impl Into<Element<'a, Message>>,
    message: Message,
    open: bool,
    hint: String,
) -> Element<'a, Message> {
    let face = container(face)
        .center_x(Length::Fixed(BAND_BUTTON - theme::MD * 2.0))
        .center_y(Length::Fixed(theme::tile()));
    tooltip(
        button(face)
            .padding([theme::XS, theme::MD])
            .style(match open {
                true => theme::selected,
                false => theme::ghost,
            })
            .on_press(message),
        container(text(hint).size(theme::LABEL)).padding([theme::XS, theme::SM]).style(theme::floating),
        tooltip::Position::Bottom,
    )
    .into()
}

/// Machine text, versions, paths, ids. Monospaced so a version number cannot
/// be mistaken for prose, and so columns line up without a table.
pub(crate) fn mono<'a>(content: String) -> iced::widget::Text<'a> {
    text(content)
        .size(theme::CAPTION)
        .font(theme::MONO)
        .color(theme::MUTED)
}

/// Draw one block of an addon's description or changelog.
///
/// The whole renderer (see `lightforge_core::addons::richtext`); an unknown
/// kind draws as a paragraph.
pub(crate) fn rich_block(block: &richtext::Block) -> Element<'_, Message> {
    use richtext::Kind;

    match &block.kind {
        Kind::Heading => {
            // Two sizes, not six: "version" and "section".
            let size = match block.level {
                0..=2 => theme::HEADING,
                _ => theme::BODY,
            };
            container(text(block.text.clone()).size(size)).padding([theme::XS, 0.0]).into()
        }

        Kind::Bullet => row![
            // Its own text, so a wrapped line hangs under the text.
            muted("•".to_owned(), theme::BODY),
            rich_paragraph(block),
        ]
        .spacing(theme::SM)
        .padding(iced::Padding::default().left(theme::MD * f32::from(block.indent)))
        .into(),

        Kind::Code => container(
            text(block.text.clone()).size(theme::CAPTION).font(iced::Font::MONOSPACE),
        )
        .padding(theme::SM)
        .width(Length::Fill)
        .style(theme::row)
        .into(),

        // Not drawn: an image cache for a card open ten seconds.
        Kind::Image => Space::new().into(),

        Kind::Para | Kind::Other(_) => rich_paragraph(block),
    }
}

/// A block's text with its links clickable.
///
/// Cut at the proxy's offsets, not by searching the text.
pub(crate) fn rich_paragraph(block: &richtext::Block) -> Element<'_, Message> {
    let inline = block.inline_links();
    let trailing: Vec<_> = block.trailing_links().collect();

    if inline.is_empty() && trailing.is_empty() {
        return text(block.text.clone()).size(theme::BODY).into();
    }

    let mut spans = Vec::with_capacity(inline.len() * 2 + 1);
    let mut cursor = 0;
    for link in &inline {
        if link.start > cursor {
            spans.push(span(block.text[cursor..link.start].to_owned()));
        }
        spans.push(
            span(block.text[link.start..link.end].to_owned())
                .color(theme::ACCENT)
                .underline(true)
                .link(link.url.clone()),
        );
        cursor = link.end;
    }
    if cursor < block.text.len() {
        spans.push(span(block.text[cursor..].to_owned()));
    }

    // A link the proxy could not seat inside the text keeps its destination
    // as a reference after it, rather than being lost.
    for link in trailing {
        spans.push(span(" "));
        spans.push(
            span(link.text.clone())
                .color(theme::ACCENT)
                .underline(true)
                .link(link.url.clone()),
        );
    }

    rich_text(spans).size(theme::BODY).on_link_click(Message::OpenUrl).into()
}

/// A state at a button's size, for where one stands beside buttons.
///
/// Padded like [`action_button`] and [`secondary`], so heights match.
pub(crate) fn tag<'a>(content: String, color: iced::Color) -> Element<'a, Message> {
    // Never wrapped: a two-line tag makes its row taller than the rest.
    container(
        text(content)
            .size(theme::LABEL)
            .wrapping(iced::widget::text::Wrapping::None),
    )
        .padding([theme::SM, theme::MD])
        .style(theme::tag(color))
        .into()
}

/// A status pill.
pub(crate) fn pill<'a>(content: String, color: iced::Color) -> Element<'a, Message> {
    container(text(content).size(theme::CAPTION))
        .padding([2.0, theme::SM])
        .style(theme::badge(color))
        .into()
}

/// An empty state: what happened, and what to do about it.
pub(crate) fn empty<'a>(headline: &str, hint: &str) -> Element<'a, Message> {
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

/// A button that is disabled rather than absent when it cannot be used,
/// buttons appearing and disappearing makes a UI feel unstable.
pub(crate) struct Action<'a> {
    pub(crate) inner: button::Button<'a, Message>,
    pub(crate) message: Message,
}

impl<'a> Action<'a> {
    /// The button, pressable or not, and a place for the selection ring
    /// named by what it sends, which is unique wherever it is drawn.
    pub(crate) fn enabled(self, enabled: bool) -> Element<'a, Message> {
        let key = format!("{:?}", self.message);
        self.keyed(key, enabled)
    }

    /// [`Action::enabled`] under a name of its own, for a button whose
    /// message changes when pressed, Add turning into Remove: named by what
    /// it sends, the ring would lose it the moment Enter landed.
    pub(crate) fn keyed(self, key: String, enabled: bool) -> Element<'a, Message> {
        let target = focus::Target::Key(key);
        match enabled {
            true => focus::ring(
                target,
                focus::Press::Send(self.message.clone()),
                self.inner.on_press(self.message),
            ),
            false => focus::ring(target, focus::Press::Off, self.inner),
        }
    }

    pub(crate) fn width(mut self, width: f32) -> Self {
        self.inner = self.inner.width(Length::Fixed(width));
        self
    }
}

pub(crate) fn action_button(label: &str, color: iced::Color, message: Message) -> Action<'static> {
    Action {
        inner: button(text(label.to_owned()).size(theme::tap_text(theme::LABEL)))
            .style(theme::filled(color))
            .padding(theme::tap([theme::SM, theme::MD])),
        message,
    }
}

/// The line under a Browse hit whose author turned third-party downloads off.
pub(crate) fn opted_out_note(words: &'static str) -> Element<'static, Message> {
    text(words).size(theme::CAPTION).color(theme::UPDATE).into()
}

/// The quieter button: same size as [`action_button`], different colour.
///
/// Padded identically: side by side, only the colour differs.
pub(crate) fn secondary(label: &str, message: Message) -> Action<'static> {
    Action {
        inner: button(text(label.to_owned()).size(theme::tap_text(theme::LABEL)))
            .style(theme::outlined)
            .padding(theme::tap([theme::SM, theme::MD])),
        message,
    }
}

/// [`secondary`] at a fixed height, the label centred in it: a button's
/// content sits at the top of any height it was not sized to, so this one is
/// told to fill and centre.
pub(crate) fn secondary_tall(label: &str, message: Message, height: f32) -> Action<'static> {
    Action {
        inner: button(
            text(label.to_owned())
                .size(theme::tap_text(theme::LABEL))
                .height(Length::Fill)
                .align_y(Alignment::Center),
        )
        .style(theme::outlined)
        .padding(theme::tap([0.0, theme::MD]))
        .height(Length::Fixed(height)),
        message,
    }
}

/// An error as the user sees it: what went wrong, what to do about it, and
/// the one button that does it, when there is one.
pub(crate) fn problem(problem: &Problem) -> Element<'_, Message> {
    use lightforge_core::advice::Fix;

    let mut body = column![text(format!("⚠ {}", problem.message)).size(theme::LABEL).color(theme::ERROR)]
        .spacing(theme::XS);
    let Some(advice) = &problem.advice else { return body.into() };
    body = body.push(text(advice.todo.clone()).size(theme::LABEL).color(theme::TEXT));

    let fix = advice.fix.as_ref().map(|fix| match fix {
        Fix::DeepScan => ("Run Deep Scan", Message::DeepScan),
        Fix::ProtonSettings => ("Open Proton settings", Message::ShowSettingsTab(SettingsTab::Proton)),
        Fix::StorageSettings => ("Open Storage settings", Message::ShowSettingsTab(SettingsTab::Storage)),
        Fix::CopyCommand(command) => ("Copy the command", Message::CopyRuntimeCommand(command.clone())),
    });
    if let Some((label, message)) = fix {
        body = body.push(secondary(label, message).keyed(format!("problem fix {label}"), true));
    }
    body.into()
}

/// A heading over a hairline, then its body, Settings' unit of grouping.
pub(crate) fn section<'a>(title: impl Into<String>, body: Element<'a, Message>) -> Element<'a, Message> {
    column![text(title.into()).size(theme::HEADING + 2.0), body]
        .spacing(theme::MD)
        .into()
}

/// Width of each fact column on an addon row. One number, so the header and
/// the rows cannot drift apart.
pub(crate) const ADDON_COLUMN: f32 = 92.0;

/// Width reserved for a row's controls, so the columns beside them line up
/// with each other and with the header.
///
/// The widest row: channel picker, Update, armed Delete and Keep, menu glyph.
pub(crate) const ADDON_ACTIONS: f32 = 380.0;

/// One fact in an addon row's column, or a dash where there is none.
pub(crate) fn cell<'a>(value: Option<String>) -> Element<'a, Message> {
    match value {
        Some(text) => muted(text, theme::CAPTION),
        None => muted("—".to_owned(), theme::CAPTION),
    }
    .width(Length::Fixed(ADDON_COLUMN))
    .into()
}

/// A column heading over those cells.
pub(crate) fn column_head<'a>(label: &str) -> Element<'a, Message> {
    muted(label.to_owned(), theme::CAPTION)
        .width(Length::Fixed(ADDON_COLUMN))
        .into()
}

/// A fixed-width label beside its control, so every control in a section
/// starts on the same line.
pub(crate) fn field_row<'a>(label: String, control: Element<'a, Message>) -> Element<'a, Message> {
    row![muted(label, theme::LABEL).width(Length::Fixed(160.0)), control]
        .spacing(theme::MD)
        .align_y(Alignment::Center)
        .into()
}

/// One setting: its name, a line on what it does, and its control at the
/// right edge. The same shape for a switch, a field or a button, so a page
/// of them reads down one column of names and one column of controls.
///
/// `more` is the longer explanation, shown only while hints are on.
pub(crate) fn setting<'a>(
    title: impl Into<String>,
    about: impl Into<String>,
    more: Option<iced::widget::Text<'a>>,
    control: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let about = about.into();
    let words = column![text(title.into()).size(theme::tap_text(theme::BODY))]
        .extend((!about.is_empty()).then(|| Element::from(muted(about, theme::LABEL))))
        .extend(more.map(Element::from))
    .spacing(theme::XS)
    .width(Length::Fill);
    row![words, control.into()]
        .spacing(theme::LG)
        .align_y(Alignment::Center)
        .padding(theme::tap([theme::MD, theme::MD]))
        .into()
}

/// Settings that belong together, in one card with hairlines between them.
pub(crate) fn setting_group<'a>(rows: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
    let mut group = column![];
    for (at, row) in rows.into_iter().enumerate() {
        if at > 0 {
            group = group.push(
                container(Space::new().height(Length::Fixed(1.0)))
                    .width(Length::Fill)
                    .style(theme::divider),
            );
        }
        group = group.push(row);
    }
    container(group).width(Length::Fill).style(theme::card).into()
}

/// A group's name above its card, quieter than the section's, with room for
/// a control that acts on the whole group at the right.
pub(crate) fn group_label<'a>(title: impl Into<String>, control: Option<Element<'a, Message>>) -> Element<'a, Message> {
    row![muted(title.into(), theme::LABEL).width(Length::Fill)]
        .extend(control)
        .spacing(theme::SM)
        .align_y(Alignment::Center)
        .padding([0.0, theme::XS])
        .into()
}

/// Free-form content in the same card a [`setting_group`] is drawn in.
pub(crate) fn setting_card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).padding(theme::MD).width(Length::Fill).style(theme::card).into()
}

/// An on/off switch for [`setting`], reachable by the selection ring.
pub(crate) fn switch<'a>(key: &'static str, on: bool, message: fn(bool) -> Message) -> Element<'a, Message> {
    clickable(
        key,
        iced::widget::toggler(on)
            .on_toggle(message)
            .size(theme::tap_text(20.0))
            .style(theme::switch),
    )
}

/// A checkbox, with a bigger box and a little more air beside it on a
/// console, where a thumb is what presses it.
pub(crate) fn tick<'a>(on: bool) -> iced::widget::Checkbox<'a, Message> {
    let tick = iced::widget::checkbox(on);
    match theme::console() {
        true => tick.size(20.0).spacing(theme::SM + theme::XS),
        false => tick,
    }
}

/// `content` as a place for the selection ring, pressed by clicking it.
///
/// For a control whose click is more than a message: a checkbox, a text
/// field that should take the typing. Whether it is enabled is read off it,
/// the way the pointer reads it. `key` has to be unique on the page.
pub(crate) fn clickable<'a>(key: impl Into<String>, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    focus::ring(focus::Target::Key(key.into()), focus::Press::Click, content)
}

/// A button inside a [`clickable`]-style ring, drawn on the button's edge.
///
/// The ring sends the message itself: iced's button ignores a made-up click
/// once its event is captured (2026-10-17: Cancel and Send never pressed).
pub(crate) fn tappable<'a>(
    key: impl Into<String>,
    on_press: Option<Message>,
    button: iced::widget::Button<'a, Message>,
) -> Element<'a, Message> {
    let press = match on_press.clone() {
        Some(message) => focus::Press::Send(message),
        None => focus::Press::Off,
    };
    focus::ring(focus::Target::Key(key.into()), press, button.on_press_maybe(on_press))
}

/// A separator between inline facts.
pub(crate) fn dot<'a>() -> Element<'a, Message> {
    text("·").size(theme::CAPTION).color(theme::MUTED).into()
}

/// The splash's spinner: a ring with a bright arc travelling round it.
///
/// A canvas, since iced has no spinner, driven by the 100 ms tick.
pub(crate) struct Spinner;

impl Spinner {
    /// How long one full turn takes.
    pub(crate) const PERIOD: f32 = 1.5;
    /// How thick the ring is drawn.
    pub(crate) const STROKE: f32 = 3.0;
    /// The shortest and longest the travelling arc gets, in turns.
    pub(crate) const SHORTEST: f32 = 0.08;
    pub(crate) const LONGEST: f32 = 0.42;

    pub(crate) fn phase() -> f32 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let elapsed = START.get_or_init(Instant::now).elapsed().as_secs_f32();
        (elapsed % Self::PERIOD) / Self::PERIOD
    }
}

impl canvas::Program<Message> for Spinner {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        use std::f32::consts::TAU;

        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let centre = iced::Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (bounds.width.min(bounds.height) - Self::STROKE) / 2.0;
        let stroke = |colour: iced::Color| {
            canvas::Stroke::default()
                .with_color(colour)
                .with_width(Self::STROKE)
                .with_line_cap(canvas::LineCap::Round)
        };

        // The track, so the arc reads as going round something.
        frame.stroke(
            &canvas::Path::circle(centre, radius),
            stroke(iced::Color { a: 0.12, ..theme::TEXT }),
        );

        // A sine over the turn, so the two ends of the arc move at different
        // speeds and it is never momentarily still.
        let turn = Self::phase();
        let length = Self::SHORTEST
            + (Self::LONGEST - Self::SHORTEST) * (turn * TAU).sin().mul_add(0.5, 0.5);
        let head = turn * TAU * 2.0;
        frame.stroke(
            &canvas::Path::new(|path| {
                path.arc(canvas::path::Arc {
                    center: centre,
                    radius,
                    start_angle: iced::Radians(head),
                    end_angle: iced::Radians(head + length * TAU),
                });
            }),
            stroke(theme::PLAY),
        );

        vec![frame.into_geometry()]
    }
}

/// One release's notes: its intro, then a bulleted list.
///
/// Shared by the popup and the Lightforge Updates page.
pub(crate) fn release_notes(release: &lightforge_core::releases::Release) -> Element<'static, Message> {
    // Prose, so a little more leading than a label gets: a note that wraps
    // should not read as two notes run together.
    let leading = iced::widget::text::LineHeight::Relative(1.45);
    let notes = column(release.notes.iter().map(|note| {
        row![
            text("·").size(theme::BODY).color(theme::MUTED).line_height(leading),
            text(note.clone()).size(theme::BODY).line_height(leading).width(Length::Fill),
        ]
        .spacing(theme::SM)
        .into()
    }))
    .spacing(theme::SM);
    match &release.intro {
        Some(intro) => column![
            text(intro.clone()).size(theme::BODY).line_height(leading).width(Length::Fill),
            notes,
        ]
        .spacing(theme::MD)
        .into(),
        None => notes.into(),
    }
}
