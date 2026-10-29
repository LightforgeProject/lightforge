//! The images bundled into the binary, each decoded once.

use crate::*;

/// The cog on the split Play button.
///
/// Bundled: chrome, and the font has no gear glyph.
pub(crate) const GEAR_ICON: &[u8] = include_bytes!("../assets/gear.png");

/// The cog's one handle. A decoded handle mints a fresh id per call, so
/// building it in `view` made every rebuild a cache miss and a PNG decode.
pub(crate) fn gear_icon() -> image::Handle {
    static GEAR: OnceLock<image::Handle> = OnceLock::new();
    GEAR.get_or_init(|| art::decode_bundled(GEAR_ICON)).clone()
}

/// The arrow on the band's overflow button; U+2304 is missing from most fonts.
pub(crate) const CHEVRON_ICON: &[u8] = include_bytes!("../assets/chevron-down.png");

/// The chevron's one handle; see [`gear_icon`] for why it is cached.
pub(crate) fn chevron_icon() -> image::Handle {
    static CHEVRON: OnceLock<image::Handle> = OnceLock::new();
    CHEVRON.get_or_init(|| art::decode_bundled(CHEVRON_ICON)).clone()
}

/// Manage Addons. A box of parts, which is what an addon set is.
pub(crate) const PACKAGE_ICON: &[u8] = include_bytes!("../assets/package.png");

/// The package's one handle; see [`gear_icon`] for why it is cached.
pub(crate) fn package_icon() -> image::Handle {
    static PACKAGE: OnceLock<image::Handle> = OnceLock::new();
    PACKAGE.get_or_init(|| art::decode_bundled(PACKAGE_ICON)).clone()
}

/// Game Settings; the cog is Lightforge's.
pub(crate) const SLIDERS_ICON: &[u8] = include_bytes!("../assets/sliders-horizontal.png");

/// The sliders' one handle; see [`gear_icon`] for why it is cached.
pub(crate) fn sliders_icon() -> image::Handle {
    static SLIDERS: OnceLock<image::Handle> = OnceLock::new();
    SLIDERS.get_or_init(|| art::decode_bundled(SLIDERS_ICON)).clone()
}

/// Manage Subscription.
pub(crate) const CARD_ICON: &[u8] = include_bytes!("../assets/credit-card.png");

/// The card's one handle; see [`gear_icon`] for why it is cached.
pub(crate) fn card_icon() -> image::Handle {
    static CARD: OnceLock<image::Handle> = OnceLock::new();
    CARD.get_or_init(|| art::decode_bundled(CARD_ICON)).clone()
}

/// Lightforge's own wordmark, drawn art.
pub(crate) const WORDMARK: &[u8] = include_bytes!("../assets/wordmark.png");

/// The wordmark's one handle; see [`gear_icon`] for why it is cached.
pub(crate) fn wordmark_image() -> image::Handle {
    static WORDMARK_HANDLE: OnceLock<image::Handle> = OnceLock::new();
    WORDMARK_HANDLE.get_or_init(|| art::decode_bundled(WORDMARK)).clone()
}

/// Add a game: drawn like the chevron beside it; a typed `+` reads as a letter.
pub(crate) const PLUS_ICON: &[u8] = include_bytes!("../assets/plus.png");

/// The plus's one handle; see [`gear_icon`] for why it is cached.
pub(crate) fn plus_icon() -> image::Handle {
    static PLUS: OnceLock<image::Handle> = OnceLock::new();
    PLUS.get_or_init(|| art::decode_bundled(PLUS_ICON)).clone()
}

/// The rest of the bundled marks: the app menu's column, the history arrows
/// and the two social squares.
///
/// Each is [`gear_icon`]'s four lines.
macro_rules! bundled_icons {
    ($($name:ident => $file:literal),+ $(,)?) => {
        $(
            pub(crate) fn $name() -> image::Handle {
                static HANDLE: OnceLock<image::Handle> = OnceLock::new();
                HANDLE
                    .get_or_init(|| {
                        art::decode_bundled(include_bytes!(concat!("../assets/", $file)))
                    })
                    .clone()
            }
        )+
    };
}

bundled_icons! {
    back_icon => "arrow-left.png",
    forward_icon => "arrow-right.png",
    outbound_icon => "arrow-up-right.png",
    updates_icon => "file-text.png",
    bug_icon => "bug.png",
    feedback_icon => "message-square-quote.png",
    exit_icon => "power.png",
    ellipsis_icon => "ellipsis.png",
    close_icon => "close.png",
    download_icon => "download.png",
    discord_icon => "discord.png",
    x_icon => "x.png",
    kofi_icon => "kofi.png",
    paypal_icon => "paypal.png",
}

/// What Steam shows for Lightforge in its library, built by
/// `assets/steam/make.sh` from the icon and the wordmark.
pub(crate) const STEAM_ART: lightforge_core::steam_shortcut::Artwork<'static> = {
    use lightforge_core::steam_shortcut::{Artwork, Image};
    Artwork {
        capsule: Image { bytes: include_bytes!("../assets/steam/capsule.jpg"), ext: "jpg" },
        wide: Image { bytes: include_bytes!("../assets/steam/wide.jpg"), ext: "jpg" },
        hero: Image { bytes: include_bytes!("../assets/steam/hero.jpg"), ext: "jpg" },
        logo: Image { bytes: include_bytes!("../assets/steam/logo.png"), ext: "png" },
        icon: Image { bytes: include_bytes!("../assets/steam/icon.png"), ext: "png" },
    }
};
