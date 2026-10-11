//! The design system: tokens first, components built from them.
//!
//! Three rules hold the four screens together:
//!
//! 1. **Nothing hard-codes a colour, size or gap.** Everything comes from the
//!    scales below, so a change lands everywhere at once instead of in one
//!    screen.
//! 2. **The action button's colour is state, not decoration**: light blue
//!    install, orange update, green play. Those four are the only saturated
//!    colours on screen, which is what makes them readable at a glance.
//! 3. **Machine text is monospaced**: versions, paths, ids. It stops a
//!    version number from being mistaken for prose, and makes columns line up
//!    without a table.
//!
//! iced gives no cascade, so "restraint" here means type, spacing and one
//! accent rather than effects.

use iced::widget::{button, container, overlay, pick_list, scrollable, text_editor, text_input};
use iced::{border, gradient, Background, Border, Color, Font, Radians, Shadow, Theme, Vector};

// ---------------------------------------------------------------- palette --

/// Page background, near black, slightly blue, so art laid behind it reads as
/// part of the page rather than a picture on a grey wall.
pub const BG: Color = rgb(0x0B, 0x0D, 0x10);
/// A panel sitting on the page.
pub const SURFACE: Color = rgb(0x14, 0x17, 0x1C);
/// A panel on a panel, list rows, inputs.
pub const RAISED: Color = rgb(0x1B, 0x20, 0x27);
/// Hairline. Always a translucent white so it works over art too.
pub const BORDER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);
/// Hairline for a panel laid over artwork, where a hairline meant for a flat
/// surface disappears. Draws the edge that [`LIFT`]'s shadow cannot, see
/// [`floating`].
pub const BORDER_OVER_ART: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.16);

pub const TEXT: Color = rgb(0xE8, 0xED, 0xF2);
/// Secondary text. Contrast-checked against [`SURFACE`], not against black.
pub const MUTED: Color = rgb(0x8A, 0x94, 0xA3);

/// Blizzard blue, the single accent, and the Battle.net hand-off colour.
/// Wowhead's own red, sampled from their mark (831 px of its 180x180, by a
/// wide margin the dominant colour). Used for the one button that goes to
/// Wowhead and nowhere else in the app: a brand colour borrowed for anything
/// but a link to that brand is a colour pretending to mean something.
pub const WOWHEAD: Color = rgb(0xC2, 0x21, 0x26);

pub const ACCENT: Color = rgb(0x00, 0xB4, 0xFF);
pub const INSTALL: Color = rgb(0x4F, 0xC3, 0xF7);

/// An update waiting. Violet rather than the orange it used to be: orange and
/// [`PLAY`]'s gold are the same hue to the eye at a glance, and Update and Play
/// are the two buttons that occupy the same spot on the panel, a player who
/// reads the colour before the word has to be able to tell them apart. Violet
/// is the one strong hue the palette had not already spent.
pub const UPDATE: Color = rgb(0xA7, 0x8B, 0xFA);

/// The launch button, and nothing else.
///
/// Taken from the wordmark: it is the midpoint of the two golds the mark is
/// drawn in (`#FABD2F` and `#D79921`), so the one button the eye goes to first
/// is wearing the app's own colour. It replaced a green that read as a traffic
/// light next to a gold mark.
pub const PLAY: Color = rgb(0xE9, 0xAB, 0x28);

/// Done, healthy, present, a status, never an action. This is the green
/// [`PLAY`] used to be, kept for the things that only report: the "installed"
/// pill, a finished setup step, the dot that says the install is shared.
pub const OK: Color = rgb(0x4C, 0xAF, 0x50);

pub const PAUSE: Color = rgb(0x78, 0x86, 0x96);
pub const ERROR: Color = rgb(0xE5, 0x73, 0x73);

/// Panel over artwork. Opaque enough to read against a bright background,
/// translucent enough that the art still shows through.
pub const PANEL: Color = Color::from_rgba(0.043, 0.055, 0.070, 0.88);

// ------------------------------------------------------------------ scales --

/// Spacing scale. Four values cover every gap on every screen; a fifth would
/// only invite eyeballing.
/// Height of a primary control, so the halves of a split button match exactly.
///
/// Pinned rather than left to content: the action half is sized by a line of
/// HEADING text and the menu half by an 18px icon, and letting each shrink to
/// its own content leaves the tab visibly short of the button it belongs to.
pub const CONTROL_HEIGHT: f32 = 56.0;

pub const XS: f32 = 4.0;
pub const SM: f32 = 8.0;
pub const MD: f32 = 16.0;
pub const LG: f32 = 24.0;
pub const XL: f32 = 32.0;

/// Type scale.
pub const DISPLAY: f32 = 34.0;
pub const TITLE: f32 = 22.0;
pub const HEADING: f32 = 16.0;
pub const BODY: f32 = 14.0;
pub const LABEL: f32 = 12.0;
pub const CAPTION: f32 = 11.0;

/// The box a product's wordmark is drawn in, on the dashboard under the app
/// title.
///
/// This is a *limit*, not the size anything is drawn at, [`logo_size`] does
/// the sizing, and in the present catalogue nothing reaches either edge. The
/// box is here for the mark this catalogue does not contain yet: art so
/// extreme in shape that giving it the same area as everything else would run
/// it off the page.
///
/// The width is deliberately narrower than equal area alone would ask for.
/// The two StarCraft marks are so much wider than they are tall that the same
/// area makes them span the page, StarCraft's would be 601pt across against
/// 349 for Retail, and however equal the two are by the arithmetic, the long
/// one reads as the louder. The box cuts those two back and reaches nothing
/// else; the next widest, Classic Era's, comes to 407.
///
/// The height reaches nothing at all: the tallest mark in the catalogue,
/// Retail's with its halo, is 270.
pub const LOGO_BOX_WIDTH: f32 = 450.0;
pub const LOGO_BOX_HEIGHT: f32 = 280.0;

/// The width a wordmark is centred against before it is left-aligned.
///
/// Left-aligned alone, every mark starts on the same line but the narrow ones
/// read as *further* left than the wide ones, because what the eye places is
/// the middle of the block and not its edge: Forever, the narrowest in the
/// catalogue at 263, sits its centre a good 90pt left of StarCraft's. So a
/// mark narrower than this is nudged in by half the difference, and one at
/// least this wide simply starts at the edge. Roughly the median drawn width,
/// which is what leaves most marks untouched.
pub const LOGO_ANCHOR_WIDTH: f32 = 350.0;

/// How far a mark of `width` is nudged in from the box's left edge.
pub fn logo_offset(width: f32) -> f32 {
    ((LOGO_ANCHOR_WIDTH - width) / 2.0).max(0.0)
}

/// How far in from the page's own inset the wordmark sits.
///
/// Small: the marks are trimmed to their own ink, so flush with the page edge
/// meant *touching* it, and this is the margin that stops that, not a place
/// the wordmark was moved to. The box is centred on what this leaves, so a
/// narrow mark still sits further in than a wide one starts.
pub const LOGO_INSET: f32 = XL;

/// How much of the page one product wordmark covers, in square points.
///
/// *Area*, because area is what the eye reads as size and a bounding box is
/// not. The marks are trimmed to their own ink, so their shapes have nothing
/// in common: StarCraft's is five and a half times wider than it is tall,
/// Forever's barely wider than tall. Fitted into a shared rectangle, the
/// strip spends the whole width and the square one spends only the height,
/// and the second looks half the size of the first although both "fit". Given
/// the same area instead, a wide mark is long and low, a square one is
/// shorter and deep, and the two carry the same weight on the page, which is
/// what "the same size" means for logos that are not the same shape.
///
/// The number is the largest that keeps every mark in the catalogue inside
/// [`LOGO_BOX_WIDTH`] by [`LOGO_BOX_HEIGHT`], StarCraft's being the one that
/// sets it.
const LOGO_AREA: f32 = 56_000.0;

/// The least of a logo [`logo_size`] will believe is the mark rather than its
/// glow.
///
/// Dividing by the ink fraction is dividing by a number below one, so a
/// mistaken reading, art that is genuinely almost all soft edge, would ask
/// for a wordmark several times the size of every other. A floor costs
/// nothing on the art we have, where the lowest is Retail's 0.59.
const LOGO_MIN_INK: f32 = 0.4;

/// How wide and tall to draw one wordmark, from the shape of its trimmed ink.
///
/// [`LOGO_AREA`] decides the size; `aspect` decides how that area is spent;
/// `ink` is the share of the image that is the mark rather than its halo (see
/// [`crate::art::Art::logo_ink`]), and the box is grown by the inverse of it
/// so that the *mark* ends up at the standard area with its glow spilling
/// past.
///
/// The result is then scaled down, never up, if it would leave the box, so
/// a mark of some shape nobody has published yet is made smaller rather than
/// allowed off the page.
pub fn logo_size(aspect: f32, ink: f32) -> (f32, f32) {
    let aspect = match aspect.is_finite() && aspect > 0.0 {
        true => aspect,
        false => 1.0,
    };
    let ink = match ink.is_finite() {
        true => ink.clamp(LOGO_MIN_INK, 1.0),
        false => 1.0,
    };
    let area = LOGO_AREA / ink;
    let (width, height) = ((area * aspect).sqrt(), (area / aspect).sqrt());
    let fit = (LOGO_BOX_WIDTH / width).min(LOGO_BOX_HEIGHT / height).min(1.0);
    (width * fit, height * fit)
}

/// Edge of a product tile in the header's product bar. Square, so this is
/// both dimensions; the art inside is drawn to fit.
///
/// Sized for a row of its own under the tabs rather than a seat in them: when
/// the band shared the ribbon it had to stay near the height of a button, and
/// it no longer does. Not as large as that freedom allows, though, the band
/// is a card over the artwork now, and a card is sized by what it holds.
pub const TILE: f32 = 40.0;

/// A headline's picture on the News page, and the 16:9 box it sits in.
///
/// Wowhead publishes these at whatever shape the article wanted, 802x624 is
/// typical, so the box is fixed and the picture is cropped to fill it. A row
/// of thumbnails that each kept their own aspect ratio would leave the titles
/// beside them starting at different places, which is the thing the eye
/// notices first in a list.
pub const NEWS_THUMB_WIDTH: f32 = 160.0;
pub const NEWS_THUMB_HEIGHT: f32 = NEWS_THUMB_WIDTH * 9.0 / 16.0;

// A manifest may still carry a `height` per art set. Nothing reads it any
// more, and the field is kept only so older manifests still parse.
//
// It meant "draw this art set's logo at this many pixels tall", from when
// every logo was drawn on its publisher's own canvas and a height was the
// only lever there was for making two of them look alike. It never worked,
// a height is not a size when the shapes differ, which is why every row in
// the published table carries the same number, and against [`logo_size`] it
// does active harm: capping Retail at the table's 210 undoes most of what
// measuring its mark rather than its halo just bought it.

/// Corner radius. One value for panels, one for controls.
pub const RADIUS: f32 = 10.0;
pub const RADIUS_SM: f32 = 6.0;

/// The one drop shadow, for a surface that floats above the page.
///
/// A single value rather than a scale, because depth here is binary: a panel
/// is either part of the page or laid over it. Two shadows would imply a
/// hierarchy among floating things that the four screens do not have.
///
/// Tuned for a near-black page, pure black at a low alpha, since a grey
/// shadow over [`BG`] reads as fog rather than depth, and a downward offset
/// smaller than the blur so the panel lifts without appearing to tilt.
pub const LIFT: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
    offset: Vector::new(0.0, 4.0),
    blur_radius: 20.0,
};

/// Face for versions, paths and ids.
pub const MONO: Font = Font::MONOSPACE;

/// The emphasis weight, for the words that name a place rather than describe
/// one: the page tabs and the band's label.
///
/// Spelled out rather than built from [`Font::DEFAULT`], because struct update
/// syntax is not allowed in a `const`.
pub const BOLD: Font = Font {
    family: iced::font::Family::SansSerif,
    weight: iced::font::Weight::Bold,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

// The scales are invariants, so they are checked when the crate compiles
// rather than when a test happens to run.
const _: () = assert!(XS < SM && SM < MD && MD < LG && LG < XL);
const _: () = assert!(CAPTION < LABEL && LABEL < BODY && BODY < HEADING);
const _: () = assert!(HEADING < TITLE && TITLE < DISPLAY);


const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// The base iced theme, so built-in widgets (inputs, pick lists, scrollbars)
/// share the palette instead of being restyled one by one.
pub fn base() -> Theme {
    Theme::custom(
        "Lightforge".to_owned(),
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: OK,
            // 0.14 added a warning slot. Ours is the update colour: an update
            // waiting is the only thing in Lightforge that warns.
            warning: UPDATE,
            danger: ERROR,
        },
    )
}

/// `a` laid over `b` at `amount`, resolved now rather than by the compositor.
///
/// A gradient paints the whole window on its own, so a translucent stop would
/// be asking the renderer to blend against a clear colour it has already
/// drawn. Mixing the two here keeps the result the same on both backends.
const fn mix(a: Color, b: Color, amount: f32) -> Color {
    Color {
        r: b.r + (a.r - b.r) * amount,
        g: b.g + (a.g - b.g) * amount,
        b: b.b + (a.b - b.b) * amount,
        a: 1.0,
    }
}

/// The window's own background: [`BG`] with a wash of [`ACCENT`] in the bottom
/// right corner.
///
/// Light falling on the app rather than decoration on it, which is why it is
/// the accent and not the wordmark's gold. The far corner is the one spot with
/// nothing standing in it: the wordmark and the navigation own the top, the
/// action panel the lower left, so a glow anywhere else would be lighting
/// something that already has its own weight. The whole thing is gone by a
/// third of the way back, leaving the page the flat dark the rest of the
/// palette was picked against.
///
/// Linear, not radial: iced has no radial gradient, and a diagonal fade from
/// the corner reads as the same thing at this strength.
pub fn backdrop(_theme: &Theme) -> container::Style {
    const LIT: Color = mix(ACCENT, BG, 0.09);
    container::Style {
        background: Some(Background::Gradient(
            // 315° clockwise from north: out of the bottom right corner,
            // towards the top left.
            gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_4 * 7.0))
                .add_stop(0.0, LIT)
                .add_stop(0.32, BG)
                .into(),
        )),
        ..container::Style::default()
    }
}

// -------------------------------------------------------------- containers --

/// A panel: the default home for a group of controls.
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border { color: BORDER, width: 1.0, radius: RADIUS.into() },
        ..container::Style::default()
    }
}

/// A panel that sits over artwork.
///
/// Carries [`LIFT`]. Through 0.13 it could not: the software renderer sized a
/// dirty region from the quad's bounds expanded by a single pixel, with no
/// account of a shadow's blur radius or offset, so the area under a shadow
/// never got cleared and the shadow composited over itself on every hover
/// until it went black (iced-rs/iced#2712). #2715 fixed it, and 0.14 has it.
///
/// The brighter hairline stays alongside it. The two do different jobs, the
/// hairline draws the edge, the shadow separates the panel from what is
/// behind it, and over bright artwork a shadow alone leaves the top edge
/// indistinct, which is the case [`BORDER_OVER_ART`] exists for.
pub fn floating(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: BORDER_OVER_ART,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: LIFT,
        ..container::Style::default()
    }
}

/// A dropdown's own surface, the app menu, the overflow list, an addon row's
/// menu.
///
/// Opaque, where [`floating`] is not. The translucency is right for a panel
/// that occupies the page: it keeps the artwork readable underneath and the
/// panel is large enough that the text still has contrast. A dropdown is the
/// opposite case, a small list of short words, drawn over whatever happens to
/// be behind it, read once and dismissed. At [`PANEL`]'s alpha a bright patch
/// of key art shows straight through a four-word entry.
///
/// Same hairline and lift as [`floating`], so the two still read as the same
/// material; only the background is solid.
pub fn menu(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border {
            color: BORDER_OVER_ART,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: LIFT,
        ..container::Style::default()
    }
}

/// The dimmed window behind a dialog: the page stays visible, but plainly
/// not the thing to interact with.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.72, ..BG })),
        ..container::Style::default()
    }
}

/// A dialog's own card: opaque, unlike [`floating`], since nothing behind it
/// should show through the text.
pub fn dialog(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border { color: BORDER_OVER_ART, width: 1.0, radius: RADIUS.into() },
        ..container::Style::default()
    }
}

/// The hairline closing the header off from the content below it.
///
/// Only on the screens without the game band: on the dashboard the band's own
/// card does the separating. Its own element because a [`Border`] applies to
/// all four sides and only the bottom edge wants a line here.
///
/// (There was a `ribbon` here, an edge-to-edge panel behind the whole header.
/// It went when the band moved out into a card of its own: the bar existed to
/// give the band and the title a readable ground, and the card now carries its
/// own. Worth remembering why it was there, though, the header sits directly
/// on product artwork, and loose text over a bright sky is the thing to watch.)
pub fn ribbon_edge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER_OVER_ART)),
        ..container::Style::default()
    }
}

/// A hairline between sections, and between Settings' list and its page.
pub fn divider(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER)),
        ..container::Style::default()
    }
}

/// One row in a list. Quiet enough to repeat forty times.
pub fn row(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        border: Border { color: BORDER, width: 1.0, radius: RADIUS_SM.into() },
        ..container::Style::default()
    }
}

/// The shape of a scrollbar: narrow, and embedded rather than floating.
///
/// iced's default is 10pt wide and drawn *over* the content, which on a list
/// whose rows end in a button puts the scroller on top of the button. Giving
/// it [`SM`] of spacing makes it take layout space of its own, so it sits
/// beside the rows and the list below is simply that much narrower.
pub fn scrollbar_shape() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new().width(6.0).scroller_width(6.0).spacing(SM)
}

/// A scrollbar: no rail, and a scroller that is a hairline made solid.
///
/// Unstyled, iced draws a light grey rail that belongs to a light theme. The
/// rail is dropped entirely, the gutter is already empty space, and the
/// scroller is [`BORDER`]'s white at an alpha that reads over both a panel and
/// artwork, brightening under the pointer the way the buttons do.
pub fn scrollbar(_theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let hovered = matches!(
        status,
        scrollable::Status::Hovered { is_vertical_scrollbar_hovered: true, .. }
            | scrollable::Status::Dragged { is_vertical_scrollbar_dragged: true, .. }
    );
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(Color {
                a: match hovered {
                    true => 0.34,
                    false => 0.18,
                },
                ..TEXT
            }),
            border: Border { radius: 999.0.into(), ..Border::default() },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        ..scrollable::default(_theme, status)
    }
}

/// A pill carrying one word of status.
pub fn badge(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(Color { a: 0.16, ..color })),
        text_color: Some(color),
        border: Border { radius: 999.0.into(), ..Border::default() },
        ..container::Style::default()
    }
}

/// A state worn at a button's size: same height, same corners, no press.
///
/// [`badge`] is for a mark inside a line of text, a provider name beside an
/// addon's title. This is for a state that stands where a button would and has
/// to hold that space: "installed" next to Install, a build recommendation
/// beside the picker that sets it. A pill in that position reads as a stray
/// label rather than as the answer to the button next to it.
///
/// Deliberately not a disabled button: iced greys a button out by status, and
/// grey is the one thing this must not be, the colour is the message.
pub fn tag(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(Color { a: 0.18, ..color })),
        text_color: Some(color),
        border: Border {
            color: Color { a: 0.45, ..color },
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
        ..container::Style::default()
    }
}

/// A solid round mark, for a status a glyph can't carry: the UI font has no
/// check mark, and renders U+2713 as a tofu box.
pub fn dot(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border { radius: 999.0.into(), ..Border::default() },
        ..container::Style::default()
    }
}

// ----------------------------------------------------------------- buttons --

/// Solid button in one of the state colours. The primary action, at most once
/// per screen.
pub fn filled(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| button::Style {
        border: Border { radius: RADIUS_SM.into(), ..Border::default() },
        ..filled_base(color, status)
    }
}

/// The two halves of a split button: a wide primary action and a narrow menu
/// tab welded to its right, the way Battle.net's Play control is built.
///
/// Rounded on their outer edges only, so the pair reads as one control rather
/// than as two buttons that happen to touch. The tab is shaded a little darker
/// than the action so the divide is visible without a border between them,
/// a border would read as a seam, which is exactly what a split button is
/// trying not to be.
pub fn filled_left(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| button::Style {
        border: Border {
            radius: border::radius(RADIUS_SM).top_right(0).bottom_right(0),
            ..Border::default()
        },
        ..filled_base(color, status)
    }
}

pub fn filled_right(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let tab = lighten(color, -0.10);
        button::Style {
            border: Border {
                radius: border::radius(RADIUS_SM).top_left(0).bottom_left(0),
                ..Border::default()
            },
            ..filled_base(tab, status)
        }
    }
}

/// Fill and text for a solid button, without committing to a corner radius.
fn filled_base(color: Color, status: button::Status) -> button::Style {
    let base = match status {
        button::Status::Hovered => lighten(color, 0.08),
        button::Status::Pressed => lighten(color, -0.06),
        button::Status::Disabled => Color { a: 0.35, ..color },
        button::Status::Active => color,
    };
    button::Style {
        background: Some(Background::Color(base)),
        text_color: on(color),
        ..button::Style::default()
    }
}

/// Outlined button, secondary actions that still deserve a border.
pub fn outlined(_theme: &Theme, status: button::Status) -> button::Style {
    let (fill, border) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (Color { a: 0.10, ..TEXT }, Color { a: 0.24, ..TEXT })
        }
        button::Status::Disabled => (Color::TRANSPARENT, Color { a: 0.05, ..TEXT }),
        button::Status::Active => (Color::TRANSPARENT, BORDER),
    };
    button::Style {
        background: Some(Background::Color(fill)),
        text_color: match status {
            button::Status::Disabled => MUTED,
            _ => TEXT,
        },
        border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
        ..button::Style::default()
    }
}

/// Borderless button, navigation and low-stakes actions.
pub fn ghost(_theme: &Theme, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Hovered | button::Status::Pressed => 0.10,
        _ => 0.0,
    };
    button::Style {
        background: Some(Background::Color(Color { a: alpha, ..TEXT })),
        text_color: match status {
            button::Status::Disabled => MUTED,
            _ => TEXT,
        },
        border: Border { radius: RADIUS_SM.into(), ..Border::default() },
        ..button::Style::default()
    }
}

/// A whole card that is also a link, the News page's headlines.
///
/// [`card`] as a button rather than a container: the card *is* the click
/// target, so the hover has to happen on the card itself. A row-sized hover
/// highlight inside a static card would say the row is pressable and leave
/// the rest of the card looking like it is not.
///
/// Translucent, and wearing [`BORDER_OVER_ART`], because these sit over the
/// page's backdrop, the same pair [`floating`] uses and for the same reason.
/// A hundred opaque cards would cover the picture entirely and leave it
/// showing only in the margins, which is a background nobody can see.
pub fn card_button(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        // [`RAISED`] on hover, the existing "a panel on a panel" step, so a
        // hovered card lifts by the same amount as every other surface in the
        // app rather than by a brightness invented here. Both carry PANEL's
        // alpha, so hovering changes the colour and not the transparency.
        background: Some(Background::Color(match hovered {
            true => Color { a: PANEL.a, ..RAISED },
            false => Color { a: PANEL.a, ..SURFACE },
        })),
        text_color: TEXT,
        border: Border {
            color: match hovered {
                true => Color { a: 0.24, ..TEXT },
                false => BORDER_OVER_ART,
            },
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

/// The selected tab or nav entry: filled quietly, never with the accent, so it
/// does not compete with the primary action.
pub fn selected(_theme: &Theme, status: button::Status) -> button::Style {
    // Only a lit tab that is also a way back (Play, from a game's own pages)
    // can be hovered; the current tab has no action and reports `Disabled`.
    let a = match status {
        button::Status::Hovered | button::Status::Pressed => 0.22,
        _ => 0.14,
    };
    button::Style {
        background: Some(Background::Color(Color { a, ..ACCENT })),
        text_color: ACCENT,
        border: Border { radius: RADIUS_SM.into(), ..Border::default() },
        ..button::Style::default()
    }
}

/// The branch picker, and the list it drops down.
///
/// iced's default is a light control, which was tolerable when the picker sat
/// on the page and is not once it sits inside the header band. Both halves are
/// styled: the closed control and its overlay menu are separate types, and
/// styling only one leaves a pale list hanging off a dark button.
pub fn picker(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let hovered =
        matches!(status, pick_list::Status::Hovered | pick_list::Status::Opened { .. });
    pick_list::Style {
        background: Background::Color(match hovered {
            true => RAISED,
            false => SURFACE,
        }),
        text_color: TEXT,
        placeholder_color: MUTED,
        handle_color: MUTED,
        border: Border {
            color: match hovered {
                true => BORDER_OVER_ART,
                false => BORDER,
            },
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
    }
}

/// The open list behind [`picker`].
pub fn picker_menu(_theme: &Theme) -> overlay::menu::Style {
    overlay::menu::Style {
        background: Background::Color(SURFACE),
        text_color: TEXT,
        border: Border { color: BORDER, width: 1.0, radius: RADIUS_SM.into() },
        selected_background: Background::Color(Color { a: 0.18, ..ACCENT }),
        selected_text_color: TEXT,
        // 0.14 fixed the software renderer's shadow damage (iced-rs/iced#2715),
        // so a shadow here is safe again, but the menu never had one, and
        // adding one is a design decision rather than part of this port.
        shadow: iced::Shadow::default(),
    }
}

// ------------------------------------------------------------------ inputs --

pub fn field(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border = match status {
        text_input::Status::Focused { .. } => ACCENT,
        text_input::Status::Hovered => Color { a: 0.18, ..TEXT },
        _ => BORDER,
    };
    text_input::Style {
        background: Background::Color(RAISED),
        border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
        icon: MUTED,
        placeholder: MUTED,
        value: TEXT,
        selection: Color { a: 0.30, ..ACCENT },
    }
}

/// The multi-line twin of [`field`], for prose rather than a value.
///
/// Same surface, same border states, a report's description should look like
/// the field above it and simply be taller.
pub fn editor(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let border = match status {
        text_editor::Status::Focused { .. } => ACCENT,
        text_editor::Status::Hovered => Color { a: 0.18, ..TEXT },
        _ => BORDER,
    };
    text_editor::Style {
        background: Background::Color(RAISED),
        border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
        placeholder: MUTED,
        value: TEXT,
        selection: Color { a: 0.30, ..ACCENT },
    }
}

// ------------------------------------------------------------------ helpers --

/// Black or white text, whichever the background can carry.
fn on(background: Color) -> Color {
    let luma = 0.299 * background.r + 0.587 * background.g + 0.114 * background.b;
    match luma > 0.6 {
        true => rgb(0x0B, 0x0D, 0x10),
        false => Color::WHITE,
    }
}

fn lighten(color: Color, amount: f32) -> Color {
    Color {
        r: (color.r + amount).clamp(0.0, 1.0),
        g: (color.g + amount).clamp(0.0, 1.0),
        b: (color.b + amount).clamp(0.0, 1.0),
        a: color.a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_on_a_state_colour_stays_readable() {
        // Install, Play and Update are all light enough to need dark text;
        // green OK is the one that needs white. Getting this backwards makes
        // the one button that matters unreadable.
        assert!(on(INSTALL).r < 0.5, "light backgrounds need dark text");
        assert!(on(PLAY).r < 0.5, "the wordmark's gold needs dark text");
        assert!(on(UPDATE).r < 0.5, "violet is light enough for dark text");
        assert_eq!(on(OK), Color::WHITE);
    }

    /// The point of the whole exercise: marks of wildly different shape come
    /// out covering the same amount of page.
    #[test]
    fn marks_of_any_shape_are_given_the_same_area() {
        // Mists' trimmed ink against Forever's: 2.0:1 and 1.2:1, the widest
        // and the narrowest the box does not cut back.
        let (mw, mh) = logo_size(704.0 / 353.0, 1.0);
        let (fw, fh) = logo_size(538.0 / 440.0, 1.0);
        let (wide, square) = (mw * mh, fw * fh);
        assert!((wide - square).abs() / square < 0.01, "{wide} vs {square}");
        for (w, h) in [(mw, mh), (fw, fh)] {
            assert!(w <= LOGO_BOX_WIDTH && h <= LOGO_BOX_HEIGHT, "{w}x{h}");
        }
    }

    /// Equal area is the rule; the box is the exception to it, and StarCraft
    /// is the one mark that meets the exception; see [`LOGO_BOX_WIDTH`].
    #[test]
    fn the_one_mark_too_wide_for_the_page_is_cut_back() {
        let (sw, sh) = logo_size(704.0 / 127.0, 1.0);
        assert_eq!(sw, LOGO_BOX_WIDTH);
        assert!(sw * sh < LOGO_AREA * 0.8, "{}", sw * sh);
        // The next widest is not touched: cutting a second mark back would
        // make this a rule about wide marks rather than about this one.
        let (w2, h2) = logo_size(704.0 / 202.0, 1.0);
        assert!(w2 < LOGO_BOX_WIDTH);
        assert!((w2 * h2 - LOGO_AREA).abs() / LOGO_AREA < 0.01);
    }

    /// Shape nobody has published: made smaller, never pushed off the page.
    #[test]
    fn an_extreme_shape_is_clamped_into_the_box() {
        let (w, h) = logo_size(20.0, 1.0);
        assert!(w <= LOGO_BOX_WIDTH && h <= LOGO_BOX_HEIGHT, "{w}x{h}");
        assert!(w * h < LOGO_AREA, "a clamped mark is smaller, not larger");
    }

    /// An aspect that is not a number at all is a square, not a panic.
    #[test]
    fn a_nonsense_aspect_is_treated_as_square() {
        for nonsense in [f32::NAN, 0.0, -3.0, f32::INFINITY] {
            let (w, h) = logo_size(nonsense, 1.0);
            assert!((w - h).abs() < 0.5, "{nonsense}: {w}x{h}");
        }
    }

    /// Retail's mark sits in a halo a third as wide again as itself. Measured
    /// by its outer bounds it comes out visibly smaller than the rest; measured
    /// by the mark, it matches them.
    #[test]
    fn a_mark_inside_a_halo_is_sized_by_the_mark() {
        // Retail: 570x440 trimmed, 431x346 of it solid.
        let ink = (431.0 * 346.0) / (570.0 * 440.0);
        let (haloed, _) = logo_size(570.0 / 440.0, ink);
        let (plain, _) = logo_size(570.0 / 440.0, 1.0);
        assert!(haloed > plain * 1.2, "{haloed} vs {plain}");

        // And what is left once the halo is discounted is the standard area.
        let (w, h) = logo_size(570.0 / 440.0, ink);
        let mark = (w * h) * ink;
        assert!((mark - LOGO_AREA).abs() / LOGO_AREA < 0.01, "{mark}");
    }

    /// A narrow mark is nudged in; a wide one is left where it is.
    #[test]
    fn only_the_narrow_marks_are_nudged_off_the_edge() {
        assert_eq!(logo_offset(LOGO_BOX_WIDTH), 0.0);
        assert_eq!(logo_offset(LOGO_ANCHOR_WIDTH), 0.0);
        // Forever, the narrowest, by half what it is short of the anchor.
        assert_eq!(logo_offset(263.0), (LOGO_ANCHOR_WIDTH - 263.0) / 2.0);
    }

    /// A reading of almost-all-halo would ask for a wordmark several times
    /// everyone else's. The floor is what stops that.
    #[test]
    fn an_impossible_ink_reading_is_floored() {
        let (w, h) = logo_size(2.0, 0.01);
        let (floored, _) = logo_size(2.0, LOGO_MIN_INK);
        assert_eq!(w, floored);
        assert!(w <= LOGO_BOX_WIDTH && h <= LOGO_BOX_HEIGHT);
    }
}

