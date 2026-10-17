//! Lightforge GUI, the dashboard.
//!
//! One product at a time: its branding, one primary action whose label and
//! color *are* the state machine in [`lightforge_core::state`], and the
//! secondary paths around it.
//!
//! Two things here are deliberate and easy to get wrong:
//!
//! - **Progress arrives through a `watch` channel, sampled on a timer.** The
//!   downloader calls its callback once per stored file; forwarding each one as
//!   a message would drown the loop and make transfer speed a function of UI
//!   responsiveness. `watch` keeps only the newest value, so a slow window
//!   costs nothing.
//! - **Pausing is cancelling.** The downloader's cancel token stops the loop
//!   between chunks and the store keeps what it has, so resuming is a fresh
//!   run that plans against what is already on disk. That is also exactly what
//!   pre-flight does before launching Battle.net, which is why the same guard
//!   serves both.

mod addonwork;
mod art;
mod focus;
mod pad;
mod popover;
mod provision;
mod report;
mod tasks;
mod theme;
mod trace;
mod wizard;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use iced::widget::{
    button, canvas, column, container, image, mouse_area, progress_bar, responsive,
    rich_text, row, scrollable, span, stack, text, text_editor, text_input, tooltip, Space,
};
use iced::{Alignment, ContentFit, Element, Length, Subscription, Task};

use lightforge_core::error::chain;
use lightforge_core::addons::manage;
use lightforge_core::addons::richtext;
use lightforge_core::addons::Channel;
use lightforge_core::assets::{Manifest, Pipeline, Store};
use lightforge_core::config::{Config, SteamOsUse};
use lightforge_core::debug::DebugInfo;
use lightforge_core::library;
use lightforge_core::net::Http;
use lightforge_core::news;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::product::{Branch, Catalog, Kind, Product, ProductCode, ProductLine};
use lightforge_core::state::{self, ActionState, Local};
use lightforge_core::proton::{self, ProtonPin};
use lightforge_core::setup::Status as SetupStatus;
use lightforge_core::storage;
use lightforge_core::tact::install::Offerings;
use lightforge_core::transfers::{self, Saved, Standing};
use lightforge_core::tact::download::Cancel;
use lightforge_core::tweaks::{GraphicsApi, HostTools, Preset, Tweaks};

use art::Art;
use tasks::{Refreshed, SharedDownloads, TransferState};

/// Where a player manages their subscription, per the spec.
const ACCOUNT_URL: &str = "https://account.battle.net/games";

/// The cog on the split Play button.
///
/// Bundled rather than themed: it is Lightforge's own chrome, not a product's
/// branding, so it must render before any asset pipeline has answered, and
/// the UI font has no gear glyph to fall back on.
const GEAR_ICON: &[u8] = include_bytes!("../assets/gear.png");

/// The cog's one handle. A decoded handle mints a fresh id per call, so
/// building it in `view` made every rebuild a cache miss and a PNG decode.
fn gear_icon() -> image::Handle {
    static GEAR: OnceLock<image::Handle> = OnceLock::new();
    GEAR.get_or_init(|| art::decode_bundled(GEAR_ICON)).clone()
}

/// The arrow on the band's overflow button. Bundled for the same reasons as
/// the cog: it is Lightforge's chrome, and the UI font has no dependable
/// chevron, U+2304 is missing from most of them and renders as a tofu box.
const CHEVRON_ICON: &[u8] = include_bytes!("../assets/chevron-down.png");

/// The chevron's one handle; see [`gear_icon`] for why it is cached.
fn chevron_icon() -> image::Handle {
    static CHEVRON: OnceLock<image::Handle> = OnceLock::new();
    CHEVRON.get_or_init(|| art::decode_bundled(CHEVRON_ICON)).clone()
}

/// Manage Addons. A box of parts, which is what an addon set is.
const PACKAGE_ICON: &[u8] = include_bytes!("../assets/package.png");

/// The package's one handle; see [`gear_icon`] for why it is cached.
fn package_icon() -> image::Handle {
    static PACKAGE: OnceLock<image::Handle> = OnceLock::new();
    PACKAGE.get_or_init(|| art::decode_bundled(PACKAGE_ICON)).clone()
}

/// Game Optimizations. Sliders rather than the cog: the cog is already the
/// split button's own mark for Lightforge's settings, and these are the
/// game's.
const SLIDERS_ICON: &[u8] = include_bytes!("../assets/sliders-horizontal.png");

/// The sliders' one handle; see [`gear_icon`] for why it is cached.
fn sliders_icon() -> image::Handle {
    static SLIDERS: OnceLock<image::Handle> = OnceLock::new();
    SLIDERS.get_or_init(|| art::decode_bundled(SLIDERS_ICON)).clone()
}

/// Manage Subscription. The bill, not the browser: every one of the three
/// links goes somewhere, so an outbound arrow said nothing about which one
/// this is, and game time is a card on file.
const CARD_ICON: &[u8] = include_bytes!("../assets/credit-card.png");

/// The card's one handle; see [`gear_icon`] for why it is cached.
fn card_icon() -> image::Handle {
    static CARD: OnceLock<image::Handle> = OnceLock::new();
    CARD.get_or_init(|| art::decode_bundled(CARD_ICON)).clone()
}

/// Lightforge's own wordmark, which stands where the app's name used to be
/// set in type. It is drawn art, the letterforms are outlines, not the UI
/// font, so it is bundled as a picture rather than composed at run time.
const WORDMARK: &[u8] = include_bytes!("../assets/wordmark.png");

/// The wordmark's one handle; see [`gear_icon`] for why it is cached.
fn wordmark_image() -> image::Handle {
    static WORDMARK_HANDLE: OnceLock<image::Handle> = OnceLock::new();
    WORDMARK_HANDLE.get_or_init(|| art::decode_bundled(WORDMARK)).clone()
}

/// Add a game. A typed `+` does exist in every font, but it is a *character*:
/// it sits on the text baseline at the weight of the UI font, which reads as
/// a letter next to the drawn chevron beside it. Both marks are strokes of the
/// same width so the pair looks like one control.
const PLUS_ICON: &[u8] = include_bytes!("../assets/plus.png");

/// The plus's one handle; see [`gear_icon`] for why it is cached.
fn plus_icon() -> image::Handle {
    static PLUS: OnceLock<image::Handle> = OnceLock::new();
    PLUS.get_or_init(|| art::decode_bundled(PLUS_ICON)).clone()
}

/// The rest of the bundled marks: the app menu's column, the history arrows
/// and the two social squares.
///
/// Spelled as a macro because each one is the same four lines as [`gear_icon`]
/// above, bytes in, one cached handle out, and ten copies of it would bury
/// the three that have something of their own to say.
macro_rules! bundled_icons {
    ($($name:ident => $file:literal),+ $(,)?) => {
        $(
            fn $name() -> image::Handle {
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

/// How far the News backdrop is darkened over the body of the page.
///
/// Higher than the dashboard's mid-page 0.35 because far more text sits over
/// it: a hundred headline cards rather than a handful of controls. At 0.70
/// the picture still reads as a picture between the cards while the muted
/// summary text keeps the contrast it has on a plain page.
const NEWS_SCRIM: f32 = 0.70;

/// How far it is darkened across the header and the page title.
///
/// The navigation row has no backdrop of its own, and this artwork is pale
/// sky across its top third, the one combination that leaves white text
/// sitting on white. Held, not ramped, so the whole row is on even cover.
const NEWS_SCRIM_TOP: f32 = 0.88;

/// The News page's backdrop.
///
/// Bundled rather than fetched from the asset manifest, unlike every product
/// background: that manifest is keyed by product, and this is chrome for one
/// page that belongs to no game. Kept at [`art::BACKGROUND_MAX_WIDTH`]'s 2560,
/// the same ceiling product art gets, which lands it at about half a megabyte;
/// see `assets/README.md` for how it was prepared.
const NEWS_BACKGROUND: &[u8] = include_bytes!("../assets/news-background.jpg");

/// What Steam shows for Lightforge in its library, built by
/// `assets/steam/make.sh` from the icon and the wordmark.
const STEAM_ART: lightforge_core::steam_shortcut::Artwork<'static> = {
    use lightforge_core::steam_shortcut::{Artwork, Image};
    Artwork {
        capsule: Image { bytes: include_bytes!("../assets/steam/capsule.jpg"), ext: "jpg" },
        wide: Image { bytes: include_bytes!("../assets/steam/wide.jpg"), ext: "jpg" },
        hero: Image { bytes: include_bytes!("../assets/steam/hero.jpg"), ext: "jpg" },
        logo: Image { bytes: include_bytes!("../assets/steam/logo.png"), ext: "png" },
        icon: Image { bytes: include_bytes!("../assets/steam/icon.png"), ext: "png" },
    }
};

/// The desktop entry's basename, the Wayland `app_id` and the AppStream
/// component id, which are required to be the same string, see
/// `packaging/README.md`.
const APP_ID: &str = "pro.tuxcore.Lightforge";

/// Opening size, and the floor for resizing, deliberately the same number.
///
/// The header sets the width: the title, the product picker (whose longest
/// entry is a user-supplied display name) and four navigation buttons, none of
/// which wrap. The dashboard sets the rest. Its action panel is a fixed 240pt
/// column of controls beside a column that fills, and a `Row` does not wrap
/// either, so once the window is narrower than both together, the fixed
/// column is simply not drawn. That is not a squeeze, it is PLAY disappearing
/// entirely, which is why the floor is the opening size rather than something
/// smaller that still technically renders.
///
/// The number itself is a display decision rather than a layout one: the
/// dashboard stops clipping somewhere around 1000pt, and the rest is giving
/// the key art enough room to be art. On a 2560x1440 screen this is half the
/// width and a little over half the height.
///
/// Because the floor is the same value, this is also the smallest screen
/// Lightforge is usable on, a 1366x768 laptop cannot fit 820pt of client
/// plus a title bar, and would get a window taller than its display.
///
/// A Wayland compositor is free to ignore all of it, a tiling one will hand
/// the window whatever its slot is, and below this width the dashboard will
/// lose its right-hand column. Nothing here can prevent that.
const WINDOW: iced::Size = iced::Size::new(1280.0, 820.0);
const WINDOW_MIN: iced::Size = WINDOW;

/// The splash card, the only thing painted while a start's artwork is
/// fetched. Small enough to read as a loading screen rather than as a
/// launcher that has come up broken, which is the complaint it answers: a
/// cold start used to show the full window with an empty card for a second or
/// two per game, while that game's background and wordmark came down.
///
/// The *window* is the launcher's size throughout; see [`main`]. Only this
/// card is drawn, on a transparent background, so what anyone sees is a small
/// panel floating in the middle of the screen.
const SPLASH: iced::Size = iced::Size::new(460.0, 280.0);

/// How long the splash card is kept on screen at the very least.
///
/// A warm cache answers in milliseconds, and a card that appears and vanishes
/// inside two frames reads as a glitch rather than as a start. Short enough
/// that "nothing to fetch" still feels instant, a start is never *slower*
/// than it was, only less ugly.
const SPLASH_FLOOR: Duration = Duration::from_millis(350);

/// A ceiling on the fetch, so a machine with no network, or one behind a
/// captive portal that never answers, still reaches its launcher. Whatever
/// landed inside the budget is kept; the rest is fetched on demand as it
/// always was.
const SPLASH_BUDGET: Duration = Duration::from_secs(12);

// We used to force the software renderer here. Through iced 0.13, `wgpu`
// lagged badly when resizing a window on native Wayland: the client kept up
// (a resize was serviced in well under a millisecond) but the buffer reached
// the compositor late, so the window crawled behind the pointer,
// iced-rs/iced#2750. Retested on 0.14 (KDE, native Wayland, radv on a Navi 48)
// and it is gone: wgpu is now the smoother of the two, and the software
// renderer is the one that stutters under a drag.
//
// Nothing replaces the pin, because iced's own fallback is better than ours
// was: with `ICED_BACKEND` unset it tries wgpu and drops to tiny-skia on any
// initialisation error (`iced_renderer`'s `fallback::Compositor`), so a
// machine with no Vulkan, no GPU or a broken driver still gets a window.
// `ICED_BACKEND=tiny-skia` remains as a manual escape hatch for hardware that
// initialises fine but renders poorly, which no fallback can detect.

/// Keep large, short-lived allocations out of the heap.
///
/// Decoding pictures is this app's one big allocator load: a news thumbnail
/// is 320px wide but its source is often 2560 or 3840, and the decode happens
/// at full size before the resize. Those buffers are freed correctly, but
/// glibc raises its own mmap threshold as it watches large blocks come and go
/// (up to 32 MiB), after which such allocations come off the heap instead,
/// and freed heap pages stay resident. Across 48 threads, each with its own
/// arena, that strands hundreds of megabytes of free space.
///
/// Measured on the News page with 97 cached images, before and after:
///
/// ```text
/// as shipped                        444 MB RSS
/// MALLOC_ARENA_MAX=2                276 MB
/// + trim and mmap thresholds        149 MB   (the Play page costs 148)
/// ```
///
/// Pinning the mmap threshold also disables glibc's dynamic adjustment of it,
/// which is the mechanism that caused the drift. The trim threshold decides
/// how much free space at the top of the heap is kept rather than returned.
///
/// glibc only: these are GNU extensions, absent on musl, where the allocator
/// behaves differently and would need measuring again rather than assuming.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    // 128 KiB: large enough that ordinary small allocations are untouched,
    // small enough that every image buffer is mapped and so handed straight
    // back on free.
    const LARGE: libc::c_int = 128 * 1024;
    // SAFETY: `mallopt` takes two ints and touches only the allocator's own
    // parameters. Called before any thread is spawned.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, LARGE);
        libc::mallopt(libc::M_TRIM_THRESHOLD, LARGE);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

fn main() -> iced::Result {
    // The one flag, answered before anything opens a window. Packaging checks
    // that a release ships the build it says it does by asking both binaries,
    // and a bug report is easier with the version than with a screenshot.
    // Same shape as clap's answer for `lightforge`.
    if std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V") {
        println!("lightforge-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    tune_allocator();
    trace::begin();
    // 0.14 takes the boot function where 0.13 took the title, and runs it
    // from `run` rather than `run_with`.
    iced::application(
        || {
            let (app, task) = Lightforge::new();
            trace::mark("app constructed, startup tasks queued");
            (app, task)
        },
        Lightforge::update,
        Lightforge::view,
    )
    .title("Lightforge")
    .subscription(Lightforge::subscription)
    // Annotated: without the type the closure is inferred for one
    // lifetime, and 0.14's `theme` wants it for any.
    .theme(|_state: &Lightforge| theme::base())
    // What the surface is cleared to, which is the other half of the splash
    // being a small window. `transparent: true` only gives the surface an
    // alpha channel, iced still clears it to the theme's background, so
    // without this the "transparent" window is an opaque rectangle with a
    // card in the middle of it. Measured: it dimmed everything behind it.
    .style(|state: &Lightforge, theme| iced::theme::Style {
        background_color: match state.splash.is_some() {
            true => iced::Color::TRANSPARENT,
            false => theme.palette().background,
        },
        text_color: theme.palette().text,
    })
    // The window is the launcher's size from the first frame and is never
    // resized. While the artwork is fetched it paints nothing but the
    // [`SPLASH`] card on a transparent background, so what anyone sees is a
    // small panel in the middle of the screen, and the frame is off for the
    // same reason, because a loading screen with a title bar is a window, not
    // a splash.
    //
    // **The window must not be resized to do this**, which an earlier version
    // did and three bugs came out of. A Wayland client cannot place itself,
    // winit's `set_outer_position` is literally `// Not possible on Wayland.`,
    // so the compositor centres the window once, at the size it is mapped
    // at, and a client-side resize afterwards grows it from its top-left
    // corner and leaves it off-centre for good. Mapping at the final size is
    // the only way the window is ever centred. It also costs nothing: no
    // stretched first frames, no `xdg_toplevel` protocol error from a
    // `min_size` beside `resizable: false`, and no maximize flash.
    .window(iced::window::Settings {
        size: WINDOW,
        min_size: Some(WINDOW_MIN),
        decorations: false,
        transparent: true,
        position: iced::window::Position::Centered,
        // The Wayland `app_id` / X11 `WM_CLASS`, and the one thing that ties
        // a running window to the installed desktop entry: a compositor
        // matches `pro.tuxcore.Lightforge.desktop` by this string and by
        // nothing else. Left at winit's default the taskbar shows a generic
        // placeholder instead of the launcher's icon.
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: APP_ID.to_owned(),
            ..Default::default()
        },
        ..iced::window::Settings::default()
    })
    // Closing is handled by the app, so a transfer can checkpoint first.
    // Set after `window`, which replaces the settings wholesale.
    .exit_on_close_request(false)
    .run()
}

/// The release-notes popup, while it is up.
#[derive(Debug, Clone)]
struct PatchNotes {
    release: lightforge_core::releases::Release,
    /// Whether the checkbox is ticked. Written to the config on dismissal
    /// rather than on each click, so a tick someone changes their mind about
    /// before closing the card costs nothing.
    mute: bool,
}

/// What the splash window is waiting on.
#[derive(Debug, Clone)]
struct Splash {
    /// How many products the prefetch was given, for the line under the mark.
    products: usize,
    /// When the window opened, so [`SPLASH_FLOOR`] can be honoured.
    opened: Instant,
    /// Whether the cache has been read and the splash knows if it waits for
    /// the network. Decided once, on the first cache-only pass.
    decided: bool,
}

#[derive(Debug, Clone)]
enum Message {
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
    /// The product codes Battle.net's own config uses.
    KnownCodes(Vec<String>),
    /// Tick or untick "never show this again" on the release-notes popup.
    MutePatchNotes(bool),
    /// The same preference from Settings › General, read the other way round.
    ShowReleaseNotes(bool),
    /// Show or hide the headlines beside the key art.
    NewsOnPlay(bool),
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
    /// The window gained or lost the desktop's focus. A gamepad is read
    /// whatever window is in front, so this is what keeps it from steering
    /// the launcher behind a running game.
    WindowFocused(bool),
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
    PreflightDone(Launch, Result<(), String>),
    /// Diagnostic only: subscribed to solely under `LIGHTFORGE_TRACE=frames`.
    ResizeTraced(iced::Size),
    /// The optimistic, cache-only paint that precedes [`Message::Themed`].
    CachedTheme(ProductCode, Box<tasks::Themed>),
    Primary,
    Tick,
    Downloaded(ProductCode, Result<Complete, String>),
    CloseRequested(iced::window::Id),
    Launched(Result<String, String>),
    /// A hand-off ran to the point where there is something to say: the
    /// game is up, the client needs a person, or nothing started.
    HandoffStarted(Result<tasks::Handoff, String>),
    BattleNet,
    ManageSubscription,
    OpenFiles,
    CopyDebug,
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
    /// Open Settings on a particular tab, for a link that means a specific
    /// page rather than "settings somewhere".
    ShowSettingsTab(SettingsTab),
    /// A switch on a Game Optimizations panel was flipped.
    Tweak(Scope, Tweak, bool),
    /// A starting point was chosen, setting the frames-versus-scenery switches.
    TweakPreset(Preset),
    /// "Custom": clear the managed switches and leave them to the user.
    TweakCustom,
    /// Show or hide the environment-variable editor.
    TweakEnvToggle(Scope),
    /// The graphics API the client should ask for.
    TweakApi(ApiChoice),
    TweakFpsCap(String),
    /// Frame cap for when the window is not focused.
    TweakFpsCapBackground(String),
    /// Which monitor the Wayland backend should treat as primary.
    TweakWaylandMonitor(Scope, String),
    /// Key and value fields of the environment escape hatch.
    TweakEnvKey(String),
    TweakEnvValue(String),
    TweakEnvAdd(Scope),
    TweakEnvRemove(Scope, String),
    /// Throw away this product's shader cache, e.g. after a Proton swap.
    ClearShaderCache,
    /// Opt in or out of one optional download, by manifest tag name.
    OptionalContent(String, bool),
    /// The cog beside the primary button.
    ToggleGear,
    /// The menu under the Lightforge wordmark.
    ToggleAppMenu,
    /// Back and forward through the pages visited this session.
    GoBack,
    GoForward,
    /// Open one of the project's own links in the browser.
    OpenLink(&'static str),
    /// A headline's own link. Owned, unlike [`Message::OpenLink`]'s static
    /// chrome URLs: these come off the wire.
    OpenArticle(String),
    NewsLoaded(Option<Box<news::Headlines>>),
    NewsImages(Vec<(String, image::Handle)>),
    NewsBackground(image::Handle),
    RefreshNews,
    /// Quit.
    Exit,
    /// A click that no widget consumed, close whatever is open, the way
    /// every other context menu on the desktop behaves.
    Dismiss,
    /// A click *inside* an open panel. Does nothing, and that is the point:
    /// it stops the click reaching [`Message::Dismiss`] underneath.
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
    /// Put the 32-bit library command on the clipboard. Lightforge never runs
    /// it: installing system packages needs root, and the distributions where
    /// help is most needed cannot be served by a command at all.
    CopyRuntimeCommand(String),
    /// Re-read the machine after the user has installed the libraries, so the
    /// row clears without restarting the app.
    RecheckRuntime,
    SetupFinished(Result<Box<SetupStatus>, String>),
    PinDefaultProton(ProtonChoice),
    /// The build Battle.net and Agent launch under.
    PinBattleNetProton(ProtonChoice),
    /// One product's own pin. Carries the code, because the Proton page
    /// lists every installed game rather than whichever one the Play page
    /// happens to be showing.
    PinProductProton(ProductCode, ProtonChoice),
    /// The download cap field changed.
    DownloadLimitChanged(String),
    /// The pause-while-playing switch was flipped.
    PauseWhilePlaying(bool),
    /// Whether the launcher gets out of the way once a game is up.
    MinimizeOnLaunch(bool),
    /// Whether a Battle.net hand-off puts the client in the tray.
    BattleNetMinimized(bool),
    /// Battle.net's own limit, as read from its config.
    BattleNetLimitRead(Option<u64>),
    /// The Battle.net limit field changed.
    BattleNetLimitChanged(String),
    /// Writing that limit finished.
    BattleNetLimitWritten(Result<(), String>),
    CheckProton,
    ProtonChecked(Result<provision::ProtonReleases, String>),
    /// A version was chosen in the "install another build" picker.
    ChooseProtonVersion(ProtonChoice),
    /// Install the chosen version.
    InstallProton,
    /// That build finished, or did not.
    ProtonInstalled(Result<(), String>),
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
    Uninstalled(ProductCode, Result<u64, String>),
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
    AddonsFinished(Result<Box<lightforge_core::addons::Report>, String>),
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
    BackupsListed(Result<Vec<lightforge_core::backup::Backup>, String>),
    BackupNow,
    AskRestore(PathBuf),
    CancelRestore,
    ConfirmRestore,
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
    AutoUpdateFinished(Result<Option<Box<lightforge_core::addons::Report>>, String>),
    SettingsTab(SettingsTab),
    SearchChanged(String),
    RunSearch,
    SearchDone(Box<addonwork::SearchOutcome>),
    InstallAddon(String, String),
    CheckAddonUpdates,
    AddonUpdatesChecked(ProductCode, Result<Vec<lightforge_core::addons::manage::UpdateStatus>, String>),
}

/// What INSTALL opens before anything downloads: where it goes, what it
/// costs, and whether to include high-resolution textures.
#[derive(Debug, Clone)]
struct InstallDialog {
    code: ProductCode,
    label: String,
    stage: DialogStage,
    high_res: bool,
    /// Every drive this game could go on, main first.
    targets: Vec<InstallTarget>,
    /// Index into `targets`.
    chosen: usize,
}

impl InstallDialog {
    fn target(&self) -> Option<&InstallTarget> {
        self.targets.get(self.chosen)
    }
}

/// One drive offered by the install dialog.
#[derive(Debug, Clone)]
struct InstallTarget {
    id: String,
    label: String,
    /// Where the game would actually land.
    install: PathBuf,
    /// Space on that drive, or `None` when it could not be read.
    disk: Option<storage::Disk>,
    state: library::State,
    /// A WoW install is already there, so this version joins it and shares
    /// its data store rather than starting a second one.
    shares: bool,
}

#[derive(Debug, Clone)]
enum DialogStage {
    /// Reading the build's download manifest.
    Calculating,
    Ready(Box<Offerings>),
    Failed(String),
}

/// Which of the two report dialogs is open.
/// Where a press that moves the ring came from; see [`Message::Nav`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavFrom {
    Keys,
    Pad,
}

/// A press from the other source this soon after one from the first is the
/// same press arriving twice. Longer than both a key's repeat and the pad's,
/// so a held direction stays with whichever source started it.
const NAV_DOUBLED: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportKind {
    /// Something is broken, so the diagnostic bundle goes with it.
    Bug,
    /// A note about the app. Nothing is wrong with the machine, so nothing
    /// about the machine is sent.
    Feedback,
}

/// How far a report has got.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReportStage {
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
const REPORT_COOLDOWN: Duration = Duration::from_secs(300);

/// The bug report / feedback dialog.
///
/// Not `Clone` or `Debug`, unlike its neighbours: a [`text_editor::Content`]
/// is neither, because it owns a laid-out buffer rather than a string.
struct ReportDialog {
    kind: ReportKind,
    /// What the user is typing. A multi-line editor rather than a field,
    /// "what were you doing" is a paragraph, and a one-line box that scrolls
    /// sideways invites a one-line answer.
    note: iced::widget::text_editor::Content,
    /// Picked before Send is offered; `None` until then.
    category: Option<report::Category>,
    /// An address or a Discord name, so there is a way to answer. Required
    /// on a bug report, since one nobody can be asked about usually stalls at
    /// the first question; optional on feedback. The only personal thing
    /// either report carries.
    contact: String,
    /// Facts worth filtering issues by, taken off the same bundle the card
    /// shows. Empty for feedback, which carries no diagnostics.
    tags: Vec<(String, String)>,
    /// A screenshot dropped on the dialog, if there is one.
    shot: Option<art::Shot>,
    /// The debug bundle, rendered when the dialog opened. That is where the
    /// four things [`DebugInfo::gather`] needs are in scope. Empty for
    /// feedback, which sends none of it. Already scrubbed of the account
    /// name: every rendered bundle is.
    base: String,
    /// `base` plus what the category attaches: what the card shows and what
    /// is sent, rebuilt whenever the category changes.
    debug: String,
    /// The recent status lines shown in `debug`, scrubbed, as they were when
    /// the dialog opened. What is sent, so the report carries exactly what
    /// the card showed and nothing logged while it was being written. Empty
    /// for feedback, like the bundle.
    history: Vec<String>,
    stage: ReportStage,
}

impl ReportDialog {
    /// Everything Send needs from what was typed and picked. The stage is the
    /// caller's to check: this is about the content, not the moment.
    fn can_send(&self) -> bool {
        let contact = match self.kind {
            ReportKind::Bug => !self.contact.trim().is_empty(),
            ReportKind::Feedback => true,
        };
        self.category.is_some() && contact && !self.note.text().trim().is_empty()
    }
}

/// The confirmation before a game is deleted.
#[derive(Debug, Clone)]
struct UninstallDialog {
    code: ProductCode,
    label: String,
    /// `None` while the plan is being worked out.
    plan: Option<Result<tasks::UninstallPreview, String>>,
    /// Deleting: the buttons are held until it answers.
    working: bool,
    /// Bytes the compaction has copied so far, written by the task.
    copied: Arc<std::sync::atomic::AtomicU64>,
}

/// One library's unused game data on Settings > Storage.
enum UnusedData {
    Checking,
    Ready(tasks::Unused),
    /// Freeing it: bytes copied so far, written by the task, and the total.
    Freeing { copied: Arc<std::sync::atomic::AtomicU64>, total: u64 },
    /// What the last check or free had to say: how much it freed, or why not.
    Said(String),
}

/// Below this the row is not shown: a button that frees a few megabytes is
/// noise on the page.
const UNUSED_WORTH_SHOWING: u64 = 100 * 1024 * 1024;

/// The confirmation before a download is given up.
///
/// Two different questions wearing one dialog. Giving up a *first install*
/// leaves a partial store nothing can read and nothing names, so it offers to
/// delete it and says how much that is. Giving up an *update* must not delete
/// anything: the game on disk is playable right now, and what has come down is
/// reused by the next attempt rather than fetched twice.
#[derive(Debug, Clone)]
struct CancelDialog {
    code: ProductCode,
    label: String,
    /// The version already installed, when this is an update. `None` makes it
    /// a first install, which is the only case where files are removed.
    installed: Option<String>,
}

/// The confirmation before a launch that would open the shared prefix with an
/// older Proton than the one that last built it.
///
/// Shown rather than refused. The pin is a choice the user made, an older
/// build is sometimes the only one a title runs under, and Lightforge is not
/// in a position to know that this particular pair breaks. What it can do is
/// say what is about to happen to the prefix every other game shares.
#[derive(Debug, Clone)]
struct DowngradeDialog {
    /// Held until it is answered, then started unchanged.
    what: Launch,
    /// The build the launch would run under.
    pick: String,
    /// The build that last created or upgraded the prefix.
    built_by: String,
    /// The game being started, for the sentence.
    label: String,
}

/// Which section of Settings is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SettingsTab {
    General,
    Setup,
    Proton,
    BattleNet,
    Downloads,
    Addons,
    Storage,
    About,
}

impl SettingsTab {
    const ALL: [Self; 8] = [
        Self::General,
        Self::Setup,
        Self::Proton,
        Self::BattleNet,
        Self::Downloads,
        Self::Addons,
        Self::Storage,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Setup => "Setup",
            Self::Proton => "Proton",
            Self::BattleNet => "Battle.net",
            Self::Downloads => "Downloads",
            Self::Addons => "Addons",
            Self::Storage => "Storage",
            Self::About => "About",
        }
    }
}

/// Where a product stands on [`proton::SHOP_BUILD`].
///
/// Three states rather than a boolean: a hand-off title has no WoW shop to
/// care about, and saying nothing is different from saying no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShopBuild {
    /// Not a WoW product. Nothing to recommend.
    NotWow,
    /// Already on it.
    Running,
    /// A WoW product pinned somewhere else, or following a default that is.
    Elsewhere,
}

/// Which half of the addon screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddonTab {
    /// What is installed, and what has updates.
    Mine,
    /// Search every provider and install.
    Browse,
    /// Bring a collection over from WoWUp in one paste.
    Import,
    /// Hand this collection to another machine, or to WoWUp.
    Export,
}

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Screen {
    Dashboard,
    /// Live news, once there is something to show. A placeholder for now: the
    /// tab is in the ribbon so the shape of the app is settled, and what fills
    /// it, Wowhead, sales, patch notes, is still being decided.
    News,
    /// Lightforge's own release notes. Placeholder, like [`Screen::News`], and
    /// separate from it: what Lightforge changed is not what Blizzard did.
    Updates,
    /// Setup lives here too, as the first card.
    Settings,
    Addons,
    /// Per-product performance settings.
    Tweaks,
}

/// One switch on the Game Optimizations panel.
///
/// A single message with a discriminant rather than a variant each: the panel
/// is a list of independent booleans, and fifteen near-identical messages would
/// only obscure that.
/// Which row of the Game Optimizations grid a height is for.
///
/// The two rows hold different amounts, so they are measured apart, see
/// [`Lightforge::card_height`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardRow {
    /// Performance and Looks.
    Top,
    /// Input and feel, and Advanced.
    Bottom,
}

/// Which profile a Game Optimizations control edits.
///
/// The same switches appear twice, on a product's own page, and in
/// Settings › Battle.net for the shared profile that Battle.net and every
/// game it launches run under. Routed by an explicit scope rather than by
/// whichever screen happens to be open: the second is the kind of implicit
/// rule that works until a panel becomes reachable from somewhere new.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The product the launcher is showing.
    Product,
    /// `Config::battle_net_tweaks`: shared, because the mechanism cannot be
    /// per-game. See that field for why.
    BattleNet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tweak {
    GamePerformance,
    GameMode,
    MangoHud,
    SkipAddonProfiling,
    ReduceRaidEffects,
    FasterZoneLoads,
    RawMouse,
    NoScreenEffects,
    MaxDrawDistance,
    NoClothPhysics,
    SoftwareCursor,
    ClipCursor,
    NoMouseAcceleration,
    ShowCursorInRelativeMode,
    Wayland,
    Hdr,
    BigShaderCache,
}

/// A graphics API in a picker, or the "leave the client's own choice alone"
/// entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiChoice(Option<GraphicsApi>);

impl std::fmt::Display for ApiChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(api) => f.write_str(api.label()),
            None => f.write_str("Whatever the client picks"),
        }
    }
}

/// A Proton build in a picker, or the "follow the default" entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtonChoice(Option<String>, Unpinned);

/// What a picker's "no pin of my own" entry falls through to, which is not the
/// same on every row, and the entry has to say which, or it reads as a build
/// the user never chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unpinned {
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
            (Some(version), _) => f.write_str(version),
            (None, Unpinned::Newest) => f.write_str("Newest installed"),
            (None, Unpinned::Default) => f.write_str("Follow the default"),
            (None, Unpinned::BattleNet) => f.write_str("Follow Battle.net"),
        }
    }
}

/// The parts of a finished run the UI actually shows. `install::Summary` is not
/// `Clone`, and a message has to be.
#[derive(Debug, Clone)]
struct Complete {
    complete: bool,
    cancelled: bool,
    files: usize,
    version: Option<String>,
}

/// The caption under a pre-installed game's button, from Battle.net's own
/// `Available October 19, 2026`. Said as the servers, not the game: the game
/// starts fine before then, it is the login that is refused. A string that
/// does not have that shape is shown as Blizzard wrote it.
fn servers_open(release_date: &str) -> String {
    let Some(date) = release_date.strip_prefix("Available ") else {
        return release_date.to_owned();
    };
    // The year is noise for a date days away.
    let date = match date.rsplit_once(", ") {
        Some((day, year)) if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) => day,
        _ => date,
    };
    format!("Servers will be available on {date}")
}

/// A product in the picker. `ProductCode` has no `Display`, and the label the
/// user reads is not the code.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    code: ProductCode,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// One entry in the branch dropdown: a build of the selected line.
///
/// Carries whether the product is configured so the list can offer branches
/// the user has never set up, picking one is how you add it, which beats
/// hand-editing `config.toml` to try a PTR.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BranchChoice {
    code: ProductCode,
    branch: Branch,
    /// The catalog's name for this product, used when adding it to config.
    label: String,
    /// Battle.net's name for this version where it has one of its own,
    /// `Stress Test` rather than `Live`; see `Families::version_name`.
    name: Option<String>,
    configured: bool,
}

impl std::fmt::Display for BranchChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.name.as_deref().unwrap_or(self.branch.label());
        match self.configured {
            true => f.write_str(name),
            // Said plainly in the list rather than greyed out: iced's
            // pick_list cannot style one entry differently, and a silently
            // different-looking row explains nothing anyway.
            false => write!(f, "{name} — add"),
        }
    }
}

/// Where one product's transfer has got to.
///
/// Keyed to a product rather than held app-wide, which is the whole point:
/// the old single `Option<Transfer>` meant every product you switched to
/// inherited whichever one happened to be running.
enum Stage {
    /// Accepted, waiting for the running one to finish.
    Queued,
    Running {
        rx: tokio::sync::watch::Receiver<TransferState>,
        cancel: Cancel,
    },
    /// Stopped with work outstanding. Keeps the progress it reached so the bar
    /// does not snap back to zero on resume.
    Paused(state::Progress),
}

/// One product's place in the download queue.
struct Download {
    code: ProductCode,
    label: String,
    stage: Stage,
    /// Newest sample off the channel, kept after a pause so the manager can
    /// still say how far it got.
    sample: TransferState,
    rate: Rate,
    job: Job,
}

/// What one transfer set out to fetch, carried across its pauses.
///
/// The bar measures the job, not the product: an update that changes 10 GB of
/// a 90 GB install reads "0 of 10 GB", not "80 of 90 GB". A resumed run plans
/// only what is still missing, so the job's size is fixed by the *first* plan
/// and each later run is placed within it.
#[derive(Debug, Clone, Copy, Default)]
struct Job {
    /// Bytes the first run planned. `None` until one has planned.
    total: Option<u64>,
    /// Bytes done when the current run started, shown while a resumed run
    /// is still planning, so the bar holds rather than dropping to zero.
    reached: u64,
}

impl Download {
    fn progress(&self) -> state::Progress {
        job_progress(self.job, &self.sample, self.rate.per_second)
    }

    /// This entry as a resumed job: the size its first run planned, and how
    /// far it got before the pause.
    fn resumed(self) -> Job {
        let reached = match self.stage {
            Stage::Paused(p) => p.downloaded_bytes,
            _ => 0,
        };
        Job { reached, ..self.job }
    }

    fn transfer(&self) -> state::Transfer {
        match &self.stage {
            Stage::Queued => state::Transfer::Queued,
            Stage::Running { .. } => state::Transfer::Running(self.progress()),
            Stage::Paused(progress) => state::Transfer::Paused(*progress),
        }
    }
}

/// Which half of the detail view is on show.
///
/// Changelog first, deliberately: somebody who opens this from an installed
/// addon is asking "what would change if I updated", and somebody browsing
/// gets the description on the other tab one click away. The default is set
/// at the call site for that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Changelog,
    About,
}

/// One addon's project page, while it is open.
struct AddonDetail {
    provider: String,
    id: String,
    /// What to call it before the project answers. The search row and the
    /// ledger both already know the name, and a card headed "…" while a
    /// request is out reads like a failure.
    name: String,
    /// `None` while the catalogue is still answering.
    loaded: Option<addonwork::Detail>,
    problem: Option<String>,
    tab: DetailTab,
}

/// The Game Settings popup, for the game that was selected when it opened.
#[derive(Debug, Clone)]
struct GameSettings {
    code: ProductCode,
    label: String,
    /// `None` until the first listing answers.
    backups: Option<Vec<lightforge_core::backup::Backup>>,
    /// A backup or restore is running; every button waits for it.
    working: bool,
    /// The backup whose Restore was pressed, waiting for its confirmation.
    confirm: Option<PathBuf>,
    /// The last outcome: a sentence, or what went wrong.
    note: Option<Result<String, String>>,
}

/// One addon's builds, while the user chooses between them.
#[derive(Debug, Clone)]
struct VersionPicker {
    provider: String,
    id: String,
    name: String,
    /// The build installed now, so the list can say which one that is.
    installed: Option<String>,
    /// `None` while the catalogue is still answering.
    builds: Option<Vec<lightforge_core::addons::catalog::FileInfo>>,
    problem: Option<String>,
}

/// What the hand-off panel's one button does, which depends on how much is
/// known about the product.
///
/// Three states rather than a pair of booleans at the call site: the label,
/// the colour and the message have to agree, and they only do that reliably
/// when one value decides all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandoffAct {
    /// Installed, and the client knows which tile it is: start it.
    Play,
    /// Known tile, nothing installed: land on it so Install can be pressed.
    Install,
    /// No game family, so nothing can be named. Opens the client and stops.
    OpenClient,
}

impl HandoffAct {
    fn label(self) -> &'static str {
        match self {
            Self::Play => "Play via Battle.net",
            Self::Install => "Install via Battle.net",
            Self::OpenClient => "Open Battle.net",
        }
    }

    /// Blue means ready to play, and only that.
    ///
    /// Installing is a filled button too. It is still the thing to press on
    /// this panel, but a neutral one, so the blue is never spent on a game
    /// that cannot be played yet. `None` draws outlined: opening the client
    /// is not an act on this game at all, so it does not dress as one.
    fn colour(self) -> Option<iced::Color> {
        match self {
            Self::Play => Some(theme::ACCENT),
            Self::Install => Some(theme::PAUSE),
            Self::OpenClient => None,
        }
    }

    /// What the button will really do, for the hints mode.
    fn hint(self) -> &'static str {
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

    fn message(self) -> Message {
        match self {
            Self::Play => Message::BattleNet,
            Self::Install => Message::InstallViaBattleNet,
            Self::OpenClient => Message::OpenBattleNet,
        }
    }
}

/// What a pre-flight pass is clearing the way for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    /// Straight to the game executable.
    Game,
    /// Hand off to Battle.net, which starts the game itself.
    ///
    /// Not for the shop's sake. That was the old assumption, disproven on
    /// 2026-10-02: the shop is the game's own CEF and works from a direct
    /// launch. This is for a game only Battle.net can install and update.
    BattleNet,
    /// Open the Battle.net client itself, for everything outside the WoW
    /// family. A pass-through, not a managed install; see `SPEC.md`.
    Client,
    /// Put the client on a game's tile so its Install button can be pressed.
    ///
    /// The same hand-off as [`Launch::BattleNet`] and deliberately a separate
    /// intent: an install needs a visible window and a person, where a launch
    /// needs neither and waits for a game that an install will not produce
    /// for an hour.
    Install,
}

struct Lightforge {
    layout: Layout,
    dirs: Option<AppDirs>,
    config: Config,
    /// One client, one connection pool, for every request the update loop
    /// starts. `None` only if reqwest failed to build one at startup.
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    /// The branding manifest, read once and themed from thereafter.
    manifest: Option<Arc<Manifest>>,
    /// Every configured product's bar icon, for those that have one.
    icons: HashMap<ProductCode, tasks::Mark>,
    /// The branch last used in each line, so returning to a tile returns to
    /// the build you were on rather than snapping back to live.
    last_branch: HashMap<ProductLine, ProductCode>,
    /// Which products exist and how they group, built in, then whatever the
    /// manifest states over it.
    catalog: Catalog,
    choices: Vec<Choice>,
    selected: Choice,

    local: Local,
    remote: Option<lightforge_core::tact::version::Version>,
    branding: Option<Art>,

    /// Every transfer this session knows about, in queue order. At most one
    /// is [`Stage::Running`].
    queue: Vec<Download>,
    /// Whether the download manager panel is open.
    manager_open: bool,
    /// Whether the catalog panel is open.
    browse_open: bool,
    /// The band's overflow menu, only ever reachable when something is in it.
    strip_open: bool,
    /// The tile whose right-click menu is open.
    tile_menu: Option<ProductLine>,
    /// The tile under the cursor, if any.
    tile_hover: Option<ProductLine>,
    /// The tile held down, while it is.
    tile_drag: Option<TileDrag>,
    /// The control the keyboard has selected, and the ring is on. `None`
    /// until a key is pressed, and again once the mouse moves.
    focus: Option<focus::Target>,
    /// The control that opened the menu the ring is in, to land on again
    /// when the menu closes.
    focus_return: Option<focus::Target>,
    /// Where the ring was when the mouse moved and took it away, for the
    /// next key to bring it back to. Steam's keyboard moves the pointer
    /// when it closes, and the ring then started over at the logo.
    focus_resume: Option<focus::Target>,
    /// Where the last control pressed by Enter was, and on which page, in
    /// case pressing it made it go away: Keep after Remove, Install inside a
    /// dialog.
    focus_pressed_at: Option<(iced::Rectangle, Screen)>,
    /// Waiting for what a key press opened or closed to be drawn, so the ring
    /// can follow it. Counts down the frames it is willing to wait.
    focus_entering: u8,
    /// A region the ring moves into once it is drawn: the page beside the
    /// Settings list, after Enter on one of its tabs.
    focus_into: Option<&'static str>,
    /// Whether the window has the desktop's focus; see
    /// [`Message::WindowFocused`].
    window_focused: bool,
    /// Which source moved the ring last, and when, so a pad that Steam is
    /// also turning into keys moves it once and not twice.
    nav_last: Option<(NavFrom, Instant)>,
    /// The picker whose list is open, by its key; see [`Lightforge::picker`].
    picker_open: Option<String>,
    /// Running on SteamOS, which is what puts the Deck-or-desktop question
    /// in setup. Read once: the OS does not change under a running app.
    steamos: bool,
    /// What the last "Add Lightforge to Steam" did, said under its button.
    steam_shortcut: Option<Result<String, String>>,
    /// Whether the menu under the wordmark is open.
    app_menu_open: bool,
    /// Pages left behind, oldest first, and pages stepped back out of.
    ///
    /// Two stacks rather than an index into one list, because that is exactly
    /// what back and forward are: going back moves the current page onto
    /// `forward`, going forward moves it onto `visited`, and visiting a new
    /// page from anywhere clears `forward`: the branch you did not take.
    visited: Vec<Screen>,
    forward: Vec<Screen>,
    /// Whether the explanatory prose under the controls is drawn.
    ///
    /// Off for the people who use Lightforge: the interface is meant to be
    /// readable without a paragraph under every switch, and a screen of grey
    /// sentences is what it looks like when it is not. On for whoever is
    /// working on it; see [`HINTS_CHORD`].
    hints: bool,
    /// Battle.net's own mark, if the prefix has one, see
    /// [`Layout::battle_net_icon`]. Decoded once, here, and held as the one
    /// handle: iced has no decoder of its own, and a handle rebuilt in `view`
    /// would be a fresh texture on every frame.
    bnet_icon: Option<image::Handle>,
    /// The headlines, once the News page has asked for them.
    ///
    /// Fetched when that page is first opened rather than at startup: a
    /// launcher's job on the first frame is to show the Play button, and a
    /// feed nobody has looked at yet is not worth a request in front of it.
    news: Option<news::Headlines>,
    /// Whether a fetch is in flight, so the page says "loading" once and the
    /// tab cannot start a second one by being clicked twice.
    news_pending: bool,
    /// Set once the first attempt has finished, so an empty result reads as
    /// "nothing came back" rather than as "not asked yet".
    news_asked: bool,
    /// Headline pictures by their URL, decoded once.
    ///
    /// Keyed by URL rather than by index because the list is re-fetched: a
    /// picture whose article is still in the feed keeps the handle it already
    /// had, and a handle that does not change is a texture iced does not
    /// re-upload.
    news_images: HashMap<String, image::Handle>,
    /// The page's backdrop, decoded once off the UI thread. Drawn when it
    /// arrives; the page is perfectly readable in the instant before.
    news_bg: Option<image::Handle>,

    /// Whether the cog menu beside the primary button is open.
    gear_open: bool,
    /// Live progress of a store scan, and whether one is running.
    scan_rx: Option<tokio::sync::watch::Receiver<lightforge_core::tact::verify::Progress>>,
    scan_sample: lightforge_core::tact::verify::Progress,
    scanning: bool,
    /// Set while a deep scan runs; tripping it stops the scan. The download
    /// queue is held for as long as it is set.
    deep_cancel: Option<Cancel>,
    /// Damage a deep scan found and could not drop yet, because something
    /// was running in the prefix. Repaired from the cog menu.
    owed_repair: Option<(ProductCode, Box<lightforge_core::tact::verify::Report>)>,
    /// The product a store scan, deep or not, is checking, so its result
    /// lands on that product rather than whichever tile is selected by then.
    scan_for: Option<ProductCode>,
    /// A repair of damaged entries is rewriting the store's indices.
    repairing: Option<ProductCode>,
    /// Versions Battle.net reports installed, read when the catalog opens.
    installed_versions: HashMap<String, String>,
    /// Product codes Battle.net itself uses, folded into the catalog.
    known_codes: Vec<String>,
    /// Transfers paused for a play session, to be put back afterwards.
    resume_after_play: Vec<ProductCode>,
    /// Why `config.toml` could not be loaded. While set, the app shows only
    /// that and writes nothing; see [`Lightforge::new`].
    config_error: Option<String>,
    /// The queue as last written to disk, so an unchanged one is not
    /// rewritten on every message.
    saved_queue: transfers::Queue,
    /// Set once the window is closing: the pause that closing performs must
    /// not be what the next start restores.
    queue_frozen: bool,
    /// A cancelled transfer whose task has not reported back yet.
    ///
    /// Pausing flips the UI at once, but the run keeps going for a moment,
    /// it finishes the requests already in flight and checkpoints the store.
    /// Starting anything before it lands would put two writers on the same
    /// CASC store, which is the corruption pre-flight exists to prevent.
    winding_down: Option<ProductCode>,

    downloads: SharedDownloads,
    busy: bool,
    status: String,
    error: Option<String>,
    /// The last few status lines, oldest first, for a bug report to carry.
    history: std::collections::VecDeque<String>,
    /// What was last reported and when, so the same text cannot go out twice
    /// in a row by accident.
    last_report: Option<(String, Instant)>,
    /// Set when the window has asked to close and a transfer is still coming
    /// to rest; the window closes once it has.
    closing: Option<iced::window::Id>,

    screen: Screen,
    setup: Option<SetupStatus>,
    /// The first run, while it is happening. `None` the rest of the time, and
    /// on every start after the one that finished or dismissed it.
    wizard: Option<wizard::Wizard>,
    /// The release notes for the version that has just started, while they
    /// are on screen. `None` once dismissed, or when this start is not the
    /// first one after an update; see [`Lightforge::patch_notes_due`].
    patch_notes: Option<PatchNotes>,
    /// The small window the app opens in. `None` once it has grown into the
    /// launcher, which happens exactly once per run.
    splash: Option<Splash>,
    /// Live setup progress, sampled off the same tick as a download.
    setup_rx: Option<tokio::sync::watch::Receiver<provision::Progress>>,
    setup_sample: provision::Progress,
    setup_running: bool,
    /// Editable copy of the download cap, in MB/s. Empty means unlimited.
    limit_input: String,
    /// Editable copy of Battle.net's *own* cap, in MB/s. Empty means none.
    bnet_limit_input: String,
    proton_notice: Option<String>,
    /// Stable releases that could be installed, newest first.
    proton_available: Vec<String>,
    /// The one picked in the install picker, if any.
    proton_choice: Option<String>,
    /// Measured size of each installed build, keyed by its directory.
    proton_sizes: HashMap<PathBuf, u64>,
    /// The last Storage measurement; `None` until the section first asks.
    storage: Option<storage::Report>,
    storage_measuring: bool,
    /// What clearing the cache last freed, for the line under its row.
    cache_notice: Option<String>,
    /// Editable library root, applied by "Use this location".
    location_input: String,
    install_dialog: Option<InstallDialog>,
    uninstall_dialog: Option<UninstallDialog>,
    cancel_dialog: Option<CancelDialog>,
    /// A newer Lightforge on GitHub, found at start-up. Shown once per
    /// release and never installed; see `core::update`.
    update_notice: Option<lightforge_core::update::Available>,
    /// A cancelled transfer whose files go once the run has wound down.
    ///
    /// Deferred rather than done on the click: the downloader may still have
    /// the store open, and deleting a store out from under its own writer is
    /// not something to race. The `Downloaded` message is the run saying it
    /// has stopped, which is when this fires.
    delete_after_cancel: Vec<ProductCode>,
    /// A given-up download's files being removed right now.
    discarding: Option<ProductCode>,
    /// The product being launched, from pre-flight until it exits. Captured
    /// on the click, so the launch, and what follows it, cannot move to a
    /// tile selected in the meantime.
    launching: Option<ProductCode>,
    /// A launch whose pre-flight is done, held until the download it paused
    /// has finished winding down: the client and a still-checkpointing
    /// downloader would share one store.
    launch_when_quiet: Option<Launch>,
    report_dialog: Option<ReportDialog>,
    /// Filled in once, shortly after startup. `None` until then, and a report
    /// sent in that window simply says so rather than blocking on it.
    system: Option<iced::system::Information>,
    downgrade_dialog: Option<DowngradeDialog>,
    /// Builds the user has already agreed to launch under this session.
    ///
    /// Per build rather than once for all: agreeing to one downgrade is not
    /// agreeing to the next one. Not persisted, a new run of the launcher is
    /// a fair moment to ask again, and the prefix may have moved since.
    downgrade_ok: HashSet<String>,
    /// Partitions that could become a library, as last scanned. Refreshed
    /// when the Storage section opens rather than polled, a drive appearing
    /// while the page is shut is not news.
    partitions: Vec<library::Partition>,
    /// What clearing one drive last freed, by library id.
    drive_notice: Option<(String, String)>,
    /// Each library's unused game data, as last checked, by library id.
    unused: HashMap<String, UnusedData>,
    /// Whether nothing is installed yet, as last checked.
    ///
    /// Cached beside `drive_states` and for the same reason: it is two
    /// `read_dir` calls, and the view asks it on every frame.
    library_empty: bool,
    /// Each library's state as last checked, by id.
    ///
    /// Cached rather than asked in the view: `action()` runs on every frame,
    /// and a marker file read per frame would put a filesystem call in the
    /// render path. Refreshed when drives are rescanned and when a product is
    /// reloaded, which is also when a drive going away would matter.
    drive_states: HashMap<String, library::State>,

    /// Optional performance tools this machine has, detected once at startup.
    /// A toggle for a binary that is not installed is a launch failure waiting
    /// to happen, so the panel hides what is missing.
    tools: HostTools,
    /// Editable copies of the selected product's numeric tweaks, and the two
    /// fields of the environment escape hatch.
    fps_input: String,
    fps_bk_input: String,
    monitor_input: String,
    bnet_monitor_input: String,
    env_key_input: String,
    env_value_input: String,

    addons: addonwork::Listing,
    token_input: String,
    /// Narrows the installed list. Held here rather than derived, because it
    /// is the one piece of the addon page the user types into and expects to
    /// survive a refresh of the list underneath it.
    addon_filter: String,
    /// The addon whose Remove has been clicked once. Removing deletes files,
    /// so the second click is the one that means it.
    addon_remove_armed: Option<(String, String)>,
    /// The addon whose per-addon menu is showing. One at a time: the panel is
    /// an overlay, and two of them would draw over each other.
    addon_menu_open: Option<(String, String)>,
    /// Whether the next import reproduces the token's exact versions. Off by
    /// default: most tokens are someone saying which addons they use, not
    /// which builds of them.
    import_pinned: bool,
    /// The version picker, when it is open: which addon, and the builds the
    /// catalogue offers. `None` in the list means the fetch is still out.
    version_picker: Option<VersionPicker>,
    game_settings: Option<GameSettings>,
    addon_detail: Option<AddonDetail>,
    /// Whether the environment-variable editor is expanded, per panel.
    ///
    /// Two flags rather than one because they want different defaults. On a
    /// product's page the card is folded away. It is the escape hatch, below
    /// four cards of switches. In Settings › Battle.net it is open from the
    /// start: a hand-off game has no client settings, so environment
    /// variables are not a last resort there but most of what the page is
    /// for, and hiding them behind a Show button reads as "nothing here".
    env_open: bool,
    bnet_env_open: bool,
    /// Game families, as Blizzard's own catalogue names them, read out of the
    /// client's cache at startup. Empty on a prefix whose client has never
    /// run, which falls the hand-off back to the compiled-in families.
    families: lightforge_core::families::Families,
    /// Products whose unattended pass has already run this session. Addons
    /// are per product, so switching to Classic is the first chance to bring
    /// Classic's up to date, but only the first chance: clicking back and
    /// forth along the band must not re-ask CurseForge each time.
    addon_auto_done: Vec<ProductCode>,
    /// An unattended addon pass is in flight. Separate from `addons_busy`,
    /// which disables the buttons a person pressed: this one nobody asked
    /// for, so it must not grey the page out.
    addon_auto_running: bool,
    /// What the unattended pass did, said once on the Addons page. `None`
    /// when it has not run or had nothing to do, "0 addons updated" is noise.
    addon_auto_note: Option<String>,
    /// A launch waiting for that pass to finish. The game reads AddOns once,
    /// at login, and extracting into that directory while it does is how a
    /// half-written addon gets loaded.
    launch_after_addons: Option<Launch>,
    /// Which tab the standing report belongs to. An import's failures are read
    /// next to the import that caused them; an update's belong with the
    /// addons. Same type, two meanings, so it is remembered rather than
    /// guessed at from the report's contents.
    addon_report_tab: AddonTab,
    /// Counts keystrokes in the search box. A debounced search carries the
    /// number it was scheduled at and does nothing if another key has landed
    /// since, which is what stops a six-letter word becoming six searches.
    search_seq: u64,
    /// The token the Export button built, held so it can be read and copied.
    export_token: String,
    addon_rx: Option<tokio::sync::watch::Receiver<addonwork::Progress>>,
    addon_sample: addonwork::Progress,
    addons_busy: bool,
    addon_report: Option<lightforge_core::addons::Report>,
    addon_tab: AddonTab,
    settings_tab: SettingsTab,
    search_input: String,
    search: addonwork::SearchOutcome,
    searching: bool,
    updates: Vec<lightforge_core::addons::manage::UpdateStatus>,
}

impl Lightforge {
    fn new() -> (Self, Task<Message>) {
        let dirs = AppDirs::discover().ok();
        // A missing file is a first run and gets defaults. A file that exists
        // but will not load is someone's settings with a problem in them: the
        // app stops on it rather than quietly running on defaults, which
        // point at the default library, and then saving those over it.
        let loaded = dirs.as_ref().map(|d| Config::load_or_default(&d.config_file()));
        let config_error = match (&loaded, dirs.as_ref()) {
            (Some(Err(e)), Some(dirs)) => Some(broken_config(e, &dirs.config_file())),
            _ => None,
        };
        let mut config = match loaded {
            Some(Ok(config)) => config,
            _ => Config::with_default_products(),
        };
        // A config written before multi-drive storage has no libraries in it.
        // Seeded here rather than lazily, because every path below this line
        // resolves through one. Deliberately *not* saved: starting the app is
        // not a reason to write to someone's config, and the seed does not
        // need writing to be stable; see `Config::ensure_libraries`. It goes
        // to disk with the first change the user makes.
        let _ = config.ensure_libraries();
        let config = config;
        // Honours `blizzard_root`, the same way the CLI does.
        let layout = config.layout().unwrap_or_else(|_| Layout::with_root("."));
        let bnet_icon = layout.battle_net_icon().and_then(|path| art::try_mark(&path));

        let choices: Vec<Choice> = config
            .products
            .iter()
            .map(|p| Choice { code: p.code.clone(), label: p.display_name.clone() })
            .collect();
        let selected = choices.first().cloned().unwrap_or(Choice {
            code: ProductCode::Retail,
            label: "World of Warcraft".to_owned(),
        });

        // Without XDG directories there is no cache to theme into, and the
        // dashboard renders its own plain look rather than failing to start.
        let pipeline = dirs
            .as_ref()
            .map(|dirs| Pipeline::new(config.asset_origin(), Store::new(dirs.asset_cache_dir())));


        let limit_input = config.download_limit_mbps.map(|v| v.to_string()).unwrap_or_default();
        let app = Self {
            layout,
            dirs,
            config,
            http: Http::new().ok(),
            pipeline,
            manifest: None,
            icons: HashMap::new(),
            last_branch: HashMap::new(),
            catalog: Catalog::builtin(),
            choices,
            selected: selected.clone(),
            local: Local::default(),
            remote: None,
            branding: None,
            queue: Vec::new(),
            manager_open: false,
            browse_open: false,
            strip_open: false,
            tile_menu: None,
            tile_hover: None,
            tile_drag: None,
            focus: None,
            focus_return: None,
            focus_resume: None,
            focus_pressed_at: None,
            focus_entering: 0,
            picker_open: None,
            focus_into: None,
            window_focused: true,
            nav_last: None,
            steamos: lightforge_core::runtime::OsRelease::read().is_steamos(),
            steam_shortcut: None,
            app_menu_open: false,
            visited: Vec::new(),
            forward: Vec::new(),
            hints: false,
            bnet_icon,
            news: None,
            news_pending: false,
            news_asked: false,
            news_images: HashMap::new(),
            news_bg: None,
            gear_open: false,
            scan_rx: None,
            scan_sample: lightforge_core::tact::verify::Progress::default(),
            scanning: false,
            deep_cancel: None,
            owed_repair: None,
            scan_for: None,
            repairing: None,
            installed_versions: HashMap::new(),
            known_codes: Vec::new(),
            resume_after_play: Vec::new(),
            saved_queue: transfers::Queue::default(),
            config_error,
            queue_frozen: false,
            winding_down: None,
            downloads: SharedDownloads::default(),
            busy: false,
            status: "checking…".to_owned(),
            error: None,
            closing: None,
            screen: Screen::Dashboard,
            setup: None,
            wizard: None,
            patch_notes: None,
            splash: None,
            setup_rx: None,
            setup_sample: provision::Progress::default(),
            setup_running: false,
            limit_input,
            bnet_limit_input: String::new(),
            proton_notice: None,
            proton_available: Vec::new(),
            proton_choice: None,
            proton_sizes: HashMap::new(),
            storage: None,
            storage_measuring: false,
            cache_notice: None,
            location_input: String::new(),
            install_dialog: None,
            uninstall_dialog: None,
            cancel_dialog: None,
            update_notice: None,
            delete_after_cancel: Vec::new(),
            discarding: None,
            launching: None,
            launch_when_quiet: None,
            report_dialog: None,
            system: None,
            history: std::collections::VecDeque::new(),
            last_report: None,
            downgrade_dialog: None,
            downgrade_ok: HashSet::new(),
            partitions: Vec::new(),
            drive_notice: None,
            unused: HashMap::new(),
            library_empty: false,
            drive_states: HashMap::new(),
            tools: HostTools::detect(),
            fps_input: String::new(),
            fps_bk_input: String::new(),
            monitor_input: String::new(),
            bnet_monitor_input: String::new(),
            env_key_input: String::new(),
            env_value_input: String::new(),
            addons: addonwork::Listing::default(),
            token_input: String::new(),
            addon_filter: String::new(),
            addon_remove_armed: None,
            addon_menu_open: None,
            version_picker: None,
            game_settings: None,
            addon_detail: None,
            env_open: false,
            bnet_env_open: true,
            families: lightforge_core::families::Families::default(),
            import_pinned: false,
            addon_auto_done: Vec::new(),
            addon_auto_running: false,
            addon_auto_note: None,
            launch_after_addons: None,
            addon_report_tab: AddonTab::Mine,
            search_seq: 0,
            export_token: String::new(),
            addon_rx: None,
            addon_sample: addonwork::Progress::default(),
            addons_busy: false,
            addon_report: None,
            addon_tab: AddonTab::Mine,
            settings_tab: SettingsTab::Setup,
            search_input: String::new(),
            search: addonwork::SearchOutcome::default(),
            searching: false,
            updates: Vec::new(),
        };
        let mut app = app;
        // Nothing starts on a config that did not load: no probes against a
        // library it may not describe, and above all no restored download.
        if app.config_error.is_some() {
            // The frame still goes on: the broken-config screen is prose
            // someone has to read, close and act on, and a window with no
            // way to close it is not the thing to hand them.
            return (
                app,
                iced::window::oldest().and_then(iced::window::toggle_decorations),
            );
        }
        app.load_tweak_inputs();
        // Before anything resolves a path: a drive that came back at a
        // different mount point is found again here, so a game on it reads as
        // installed rather than as missing. Cheap, one pass over
        // `/proc/self/mounts` and one marker file per absent library.
        app.refresh_drives();
        app.restore_queue();

        // What is provisioned decides which screen opens: a machine that has
        // never run the CLI cannot install or launch anything, and a dashboard
        // whose only working button is an error is worse than saying so.
        let probe = Task::perform(
            provision::status(app.layout.clone(), app.config.clone()),
            |status| Message::SetupStatus(status.map(Box::new)),
        );
        let installed = Task::perform(
            tasks::installed_versions(app.layout.clone()),
            Message::InstalledVersions,
        );
        let codes = Task::perform(
            tasks::known_codes(app.layout.clone(), app.config.prefix_user.clone()),
            Message::KnownCodes,
        );
        let bnet_limit = Task::perform(
            tasks::battle_net_limit(app.layout.clone(), app.config.prefix_user.clone()),
            Message::BattleNetLimitRead,
        );
        // What was downloading when the app closed carries on, as it does in
        // Battle.net.
        let resume = app.pump();
        // Addons that update themselves do it now, so they are current by
        // the time anyone presses Play. Quiet, and nothing waits for it
        // except a launch; see `preflight`.
        let addons = Task::done(Message::AutoUpdateAddons);
        // Which tile each product belongs to, for the games Battle.net runs.
        let families = Task::perform(
            tasks::game_families(app.layout.clone(), app.config.prefix_user.clone(), app.config.region),
            |families| Message::FamiliesRead(Box::new(families)),
        );
        // The first run, exactly once: the flag is written when the wizard is
        // finished or dismissed, so a machine that has seen it never sees it
        // again by itself. Nothing is written here, starting the app is not a
        // reason to touch someone's config.
        if !app.config.setup_complete {
            app.wizard = Some(wizard::Wizard::default());
        }
        // Every tracked product's art into the cache before the launcher is
        // worth looking at, so the first visit to a panel is not two seconds
        // of empty card. The splash *is* this fetch, made visible, and the
        // wizard's first slide reads its count off the same run rather than
        // starting a second one.
        let artwork = app.open_splash();
        let task = Task::batch([
            app.reload(selected.code),
            addons,
            families,
            probe,
            installed,
            codes,
            bnet_limit,
            resume,
            artwork,
            iced::system::information().map(|info| Message::SystemInfo(Box::new(info))),
        ]);
        (app, task)
    }

    /// Open the splash and start the fetch it is waiting on.
    fn open_splash(&mut self) -> Task<Message> {
        let codes: Vec<ProductCode> = self.choices.iter().map(|c| c.code.clone()).collect();
        self.splash = Some(Splash { products: codes.len(), opened: Instant::now(), decided: false });
        // Nothing is fetched yet: the cache-only pass `reload` starts beside
        // this decides whether anything needs to be. See `splash_decide`.
        Task::batch([
            // The window is the launcher's size but only the card is painted,
            // so the rest of it is a transparent rectangle that would still
            // swallow every click landing on whatever is behind it. On
            // Wayland this sets a real empty input region, not a hint.
            iced::window::oldest().and_then(iced::window::enable_mouse_passthrough),
            // Beside the artwork rather than before it: this is the one moment
            // a start is already waiting on the network, and nothing on screen
            // depends on the answer, so it never holds the splash open.
            Task::perform(
                tasks::check_for_update(self.http.clone(), self.config.check_for_updates),
                Message::UpdateChecked,
            ),
        ])
    }

    /// The splash's one decision, made when the selected game's cache-only
    /// pass lands: close, or wait for the network.
    ///
    /// The splash exists so nobody sees a grey launcher, so it closes early
    /// only when the cache already holds everything the first screen draws,
    /// decoded and in state by now. The revalidation `reload` started runs
    /// on behind the launcher and swaps in whatever changed, exactly as on a
    /// product switch. Anything missing, or a first run, whose wizard counts
    /// off the prefetch, waits for the full fetch as it always has.
    fn splash_decide(&mut self, warm: bool) -> Task<Message> {
        let Some(splash) = self.splash.as_mut().filter(|s| !s.decided) else {
            return Task::none();
        };
        splash.decided = true;
        if warm && self.wizard.is_none() {
            trace::mark("splash: cache is warm, not waiting on the network");
            return self.close_splash_after_floor();
        }
        Task::perform(
            tasks::prefetch_within(
                SPLASH_BUDGET,
                self.http.clone(),
                self.pipeline.clone(),
                self.choices.iter().map(|c| c.code.clone()).collect(),
            ),
            Message::SplashDone,
        )
    }

    /// Close the splash once it has been up for [`SPLASH_FLOOR`].
    fn close_splash_after_floor(&mut self) -> Task<Message> {
        let waited = self.splash.as_ref().map(|s| s.opened.elapsed()).unwrap_or_default();
        match SPLASH_FLOOR.checked_sub(waited) {
            Some(left) if !left.is_zero() => {
                Task::perform(tasks::after(left), |()| Message::FinishSplash)
            }
            // Both ways out go through `finish_splash` and nothing else. An
            // earlier version did the work in two places, and the two copies
            // drifted: the one taken whenever the fetch outlasts the floor,
            // which is every start with anything to fetch, missed a step the
            // other had.
            _ => self.finish_splash(),
        }
    }

    /// Put the release notes up, if this start is the first one on this
    /// version.
    ///
    /// Four conditions, and each one is a case that should not see a popup:
    /// setup must be finished (a first run has the wizard to get through, and
    /// notes for a version nothing has been done with yet say nothing); the
    /// checkbox must not have been ticked; the notes must not already have
    /// been seen for this version; and the shipped file must actually describe
    /// this version, which a development build between releases does not.
    fn open_patch_notes(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        if !self.config.setup_complete
            || self.config.patch_notes_muted
            || self.config.patch_notes_seen.as_deref() == Some(version)
        {
            return;
        }
        if let Some(release) = lightforge_core::releases::notes_for(version) {
            self.patch_notes = Some(PatchNotes { release, mute: false });
        }
    }

    /// Turn the splash window into the launcher's own.
    ///
    /// Put the splash away and show the launcher.
    ///
    /// The one way out of the splash, reached whether the fetch outlasted
    /// [`SPLASH_FLOOR`] or the floor outlasted the fetch. There is deliberately
    /// no second copy of this: when there were two, they drifted.
    ///
    /// Nothing is resized or moved, the window has been the launcher's size
    /// and in the middle of the screen since it was mapped (see [`main`]).
    /// All that changes is that the frame goes on, the window starts taking
    /// clicks again, and `view` stops drawing the card.
    fn finish_splash(&mut self) -> Task<Message> {
        self.splash = None;
        trace::mark("splash done, showing the launcher");
        self.open_patch_notes();
        let console = self.console();
        let frame = iced::window::oldest().and_then(move |id| {
            // iced reads the frame's live state and inverts it, so given a
            // window mapped without one, this is "put it on". A console's
            // window goes fullscreen instead, where a frame has no place.
            let frame = match console {
                true => Task::none(),
                false => iced::window::toggle_decorations(id),
            };
            frame.chain(iced::window::disable_mouse_passthrough(id))
        });
        let frame = match console {
            true => frame.chain(self.window_mode()),
            false => frame,
        };
        // Nobody ever *navigates* to the opening screen, so `arrived` has
        // never run for it, and the news rail on it needs the headlines.
        // Through the same function the navigation uses rather than a second
        // call to `fetch_news`, so the dashboard has one definition of what
        // it wants on the way in.
        Task::batch([frame, self.arrived()])
    }

    /// Re-read the disk, the version service, and the theme for one product.
    fn reload(&self, code: ProductCode) -> Task<Message> {
        Task::batch([
            Task::perform(
                tasks::refresh(
                    self.http.clone(),
                    self.layout.clone(),
                    code.clone(),
                    self.config.region,
                    self.kind_of(&code) == Kind::Managed,
                ),
                {
                    let code = code.clone();
                    move |r| Message::Refreshed(code.clone(), Box::new(r))
                },
            ),
            // Fires first and usually lands first: it reads the cache and
            // nothing else, so the window opens themed rather than snapping
            // to art once the revalidation below comes back.
            Task::perform(
                tasks::cached_theme(self.pipeline.clone(), self.product_codes(), code.clone()),
                {
                    let code = code.clone();
                    move |t| Message::CachedTheme(code.clone(), Box::new(t))
                },
            ),
            Task::perform(
                tasks::theme(
                    self.http.clone(),
                    self.pipeline.clone(),
                    self.manifest.clone(),
                    self.product_codes(),
                    code.clone(),
                ),
                {
                    let code = code.clone();
                    move |t| Message::Themed(code.clone(), Box::new(t))
                },
            ),
            Task::perform(addonwork::list(self.layout.clone(), code), |listing| {
                Message::AddonsListed(Box::new(listing))
            }),
        ])
    }

    /// A running scan's bar and caption, for under the status line.
    ///
    /// A scan otherwise says nothing for a minute or more, which reads as a
    /// hung launcher. A deep scan is measured in bytes: its files range from
    /// a few bytes to hundreds of megabytes, so a file count stalls and leaps.
    fn scan_progress<'a>(
        &self,
        left: iced::widget::Column<'a, Message>,
    ) -> iced::widget::Column<'a, Message> {
        let p = &self.scan_sample;
        if !self.scanning || p.total == 0 {
            return left;
        }
        let (done, what) = match p.bytes_total {
            0 => (
                p.checked as f32 / p.total as f32,
                format!("{} of {} files checked", p.checked, p.total),
            ),
            total => (
                p.bytes as f32 / total as f32,
                format!("{} checked", bytes_of(p.bytes, total)),
            ),
        };
        let caption = match p.damaged {
            0 => what,
            n => format!("{what} — {n} damaged so far"),
        };
        left.push(progress_bar(0.0..=1.0, done).girth(6)).push(muted(caption, theme::CAPTION))
    }

    /// Every configured product, in the order the bar draws them.
    /// Every product whose mark the UI may draw.
    ///
    /// Not just the configured ones: the browse panel lists the whole catalog
    /// and wants each row to wear the same mark its tile would, so a game you
    /// have not added yet is still recognisable at a glance. Marks are small,
    /// content-addressed and cached immutably, so the extra ones cost one
    /// fetch each, once, and nothing thereafter.
    fn product_codes(&self) -> Vec<ProductCode> {
        let mut codes: Vec<ProductCode> = self.choices.iter().map(|c| c.code.clone()).collect();
        for entry in self.catalog.entries() {
            if !codes.contains(&entry.code) {
                codes.push(entry.code.clone());
            }
        }
        codes
    }

    /// Clear the way for a launch, then let [`Message::PreflightDone`] start it.
    /// Get out of the way, where the user asked for that.
    ///
    /// The window id has to be asked for rather than kept: this is a
    /// single-window application, so there is exactly one, but iced hands it
    /// out at runtime and the only other place that needs it, the close
    /// request, is given it by the event.
    ///
    /// Nothing un-minimizes when the game exits. A launcher that raises itself
    /// over whatever the user moved on to is the behaviour this kind of
    /// setting is turned on to avoid; the window is in the taskbar.
    fn minimize_if_asked(&self) -> Task<Message> {
        match self.config.minimize_on_launch {
            true => iced::window::latest().and_then(|id| iced::window::minimize(id, true)),
            false => Task::none(),
        }
    }

    fn preflight(&mut self, what: Launch) -> Task<Message> {
        // Before anything is told to start: the prefix is shared by every
        // product and one Battle.net login, and Proton only ever migrates it
        // forward. Running an older build against it is allowed, somebody
        // pinned it on purpose, but not silently.
        if let Some(dialog) = self.downgrade_ahead(what) {
            self.downgrade_dialog = Some(dialog);
            return Task::none();
        }
        // Checked again here, not just on the button: pre-flight relinks the
        // prefix, and relinking a library that is not there is what would
        // leave an empty install behind for the client to offer a repair on.
        // A Battle.net hand-off is held too, the game it would start is on
        // the same drive.
        if !matches!(what, Launch::Client) {
            if let Some(drive) = self.offline_drive(&self.selected.code) {
                self.error = Some(format!("{drive} is not mounted"));
                return Task::none();
            }
        }

        // Scan, repair, compaction and uninstall rewrite the store the client
        // is about to open. Downloads are not refused here: pre-flight pauses
        // them, and the launch waits for them to come to rest.
        if let Some(what) = self.maintenance() {
            self.error = Some(format!("wait for {what} to finish first"));
            return Task::none();
        }

        self.busy = true;
        self.error = None;
        self.status = "pre-flight…".to_owned();
        self.launching = Some(self.selected.code.clone());

        // An unattended addon pass is extracting into the directory the
        // client is about to read. Hold the launch until it is done rather
        // than racing it. It is seconds, and the alternative is a half
        // written addon loaded at login. Opening the Battle.net client reads
        // nothing of ours, so it does not wait.
        if self.addon_auto_running && !matches!(what, Launch::Client) {
            self.status = "finishing addon updates…".to_owned();
            self.launch_after_addons = Some(what);
            return Task::none();
        }

        // Pre-flight pauses the launched product's own download by itself,
        // that one is a correctness guard, since the client and the
        // downloader would share one CASC store. The rest of the queue shares
        // that store too, so it is paused here when asked, and resumed when
        // the game exits.
        if self.config.pause_downloads_while_playing {
            let others: Vec<ProductCode> = self
                .queue
                .iter()
                .filter(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued))
                .map(|d| d.code.clone())
                .collect();
            for code in others {
                self.pause(&code);
                self.resume_after_play.push(code);
            }
        }
        Task::perform(
            tasks::preflight(
                self.layout.clone(),
                self.selected.code.clone(),
                self.downloads.clone(),
                // Cleared under whichever build is about to run in the prefix.
                Self::runs_for(what),
            ),
            move |result| Message::PreflightDone(what, result),
        )
    }

    /// Start what a finished pre-flight cleared the way for.
    fn start_launch(&mut self, what: Launch) -> Task<Message> {
        let code = self.launching.clone().unwrap_or_else(|| self.selected.code.clone());

        // The long wait starts here, so say what it actually is. The
        // old code left "pre-flight…" on screen for the whole session
        // and the wind-down after it, which reads as a stuck launcher
        // rather than a running game.
        match what {
            Launch::Game => {
                // Play unlocks when the *game* exits, not when the
                // prefix is quiet, the wind-down behind it is watched
                // and reaped without holding the button. See
                // `tasks::run_game`.
                self.status = "running — Play unlocks when the game closes".to_owned();
                Task::batch([
                    Task::perform(
                        tasks::run_game(self.layout.clone(), code.clone()),
                        Message::Launched,
                    ),
                    self.minimize_if_asked(),
                ])
            }
            Launch::BattleNet => {
                // Two steps, because Battle.net gives nothing back:
                // the client is told to launch, then the prefix is
                // watched until the game shows up. Until it does,
                // this is still a launch in progress.
                self.status = "handing off to Battle.net…".to_owned();
                Task::perform(
                    tasks::launch_via_battle_net(
                        self.layout.clone(),
                        code.clone(),
                        self.config.prefix_user.clone(),
                        self.family_of(&code),
                        self.config.battle_net_minimized,
                    ),
                    Message::HandoffStarted,
                )
            }
            Launch::Install => {
                self.status = "opening Battle.net…".to_owned();
                Task::perform(
                    tasks::install_via_battle_net(
                        self.layout.clone(),
                        code.clone(),
                        self.config.prefix_user.clone(),
                        self.install_uid_of(&code),
                        self.label_of(&code),
                    ),
                    Message::Launched,
                )
            }
            Launch::Client => {
                self.status = "opening Battle.net…".to_owned();
                Task::perform(
                    tasks::open_battle_net(
                        self.layout.clone(),
                        code.clone(),
                        self.config.prefix_user.clone(),
                    ),
                    Message::Launched,
                )
            }
        }
    }

    /// The warning this launch owes the user, or `None` to go ahead.
    ///
    /// `None` covers every ordinary case: a prefix that does not exist yet, the
    /// build that made it, a newer one, a pair of tags with no ordering between
    /// them, and a build already agreed to this session.
    fn downgrade_ahead(&self, what: Launch) -> Option<DowngradeDialog> {
        use lightforge_core::proton::PrefixFit;

        let runs = Self::runs_for(what);
        let code = self.selected.code.clone();
        let (pick, fit) = tasks::prefix_fit(&self.layout, &code, runs);
        let (pick, built_by) = match (pick, fit) {
            (Some(pick), PrefixFit::Older { built_by }) => (pick, built_by),
            _ => return None,
        };
        if self.downgrade_ok.contains(&pick) {
            return None;
        }
        Some(DowngradeDialog {
            what,
            pick,
            built_by,
            label: self.label_of(&code),
        })
    }

    /// Which workload an intent runs as, the game itself, or the client
    /// acting for it.
    fn runs_for(what: Launch) -> tasks::Runs {
        match what {
            Launch::Game => tasks::Runs::Game,
            Launch::BattleNet | Launch::Client | Launch::Install => tasks::Runs::BattleNet,
        }
    }

    /// Which Battle.net tile a product belongs to, or `None` when neither
    /// the compiled-in families nor the client's catalogue name one.
    ///
    /// `None` is the honest answer rather than a guess: the client resolves
    /// an unknown family to the first one it knows, which is Warcraft III,
    /// so a wrong uid opens somebody else's game.
    fn family_of(&self, code: &ProductCode) -> Option<String> {
        lightforge_core::battlenet::resolve_game_uid(code, &self.families)
    }

    /// What Battle.net's install flow calls this product.
    ///
    /// The catalogue's install key where there is one, the product code
    /// otherwise. They are the same for most titles, and differ where the
    /// CDN and the client disagree (`hsb` against `hs_beta`).
    fn install_uid_of(&self, code: &ProductCode) -> String {
        self.families
            .install_uid(code.as_str())
            .unwrap_or_else(|| code.as_str())
            .to_owned()
    }

    /// Whether Lightforge installs this product or hands it to Battle.net.
    ///
    /// The catalog's answer where it has one, the code's shape otherwise, so a
    /// product added before the manifest was read is still classified.
    fn kind_of(&self, code: &ProductCode) -> Kind {
        self.catalog.get(code).map(|e| e.kind).unwrap_or_else(|| Kind::of(code))
    }

    /// The action panel for a game Battle.net installs and starts.
    ///
    ///
    /// Deliberately bare: no Install, no Update, no Play, not even greyed out.
    /// Offering those disabled would say Lightforge could manage this game and
    /// merely will not.
    /// What a hand-off game's button can honestly be, in order of how much
    /// is known. Without a game family the client cannot be told which tile
    /// to open, and a guess lands on somebody else's game, so that case
    /// promises nothing beyond opening the client.
    fn handoff_act(&self) -> HandoffAct {
        let family = self.family_of(&self.selected.code);
        let installed = self.installed_versions.contains_key(self.selected.code.as_str());
        match (family.is_some(), installed) {
            (false, _) => HandoffAct::OpenClient,
            (true, true) => HandoffAct::Play,
            (true, false) => HandoffAct::Install,
        }
    }

    fn handoff_panel(&self) -> Element<'_, Message> {
        let mut left = column![].spacing(theme::SM).width(Length::Fill);
        left = left.push(match &self.error {
            Some(e) => text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR),
            None => text(self.status.clone()).size(theme::LABEL).color(theme::MUTED),
        });

        left = self.scan_progress(left);
        match self.installed_versions.get(self.selected.code.as_str()) {
            Some(version) => {
                left = left.push(
                    row![muted("installed".to_owned(), theme::LABEL), mono(version.clone())]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center),
                );
            }
            // Said out loud, because the button changing word for word is
            // easy to miss on a screen that is mostly key art.
            None => left = left.push(muted("not installed".to_owned(), theme::LABEL)),
        }

        let act = self.handoff_act();
        left = left.extend(self.hint(act.hint()).map(Element::from));
        // The managed panel has a row of links and this one had none, so a
        // hand-off game had no route to any optimization page. It is phrased
        // for the shared profile on purpose, "its games", not "this game",
        // because one environment covers every game the client starts.
        left = left.push(self.ring(
            focus::Target::HandOffSettings,
            Some(Message::ShowSettingsTab(SettingsTab::BattleNet)),
            button(
                row![
                    image(sliders_icon())
                        .width(Length::Fixed(LINK_GLYPH))
                        .height(Length::Fixed(LINK_GLYPH)),
                    text("How Battle.net and its games launch").size(theme::LABEL),
                ]
                .spacing(theme::XS)
                .align_y(Alignment::Center),
            )
            .on_press(Message::ShowSettingsTab(SettingsTab::BattleNet))
            .style(theme::ghost)
            .padding(theme::tap([theme::XS, theme::SM])),
        ));
        let open = button(
            container(text(act.label()).size(theme::HEADING)).center_x(Length::Fill),
        )
        // Dressed as the Play control on a game Lightforge runs itself,
        // same 240 by CONTROL_HEIGHT, same heading, same filled face, because
        // it is the same act. The hand-off drives Battle.net end to end now,
        // so a quiet outlined button would be understating what it does.
        //
        // Blue rather than PLAY's gold: it starts through Battle.net, and
        // the colour is the one thing that can say so without a second line
        // of text. When there is no game family to ask for, the button only
        // opens the client, not a launch, so it does not dress as one.
        .style(move |base: &iced::Theme, status| match act.colour() {
            Some(colour) => theme::filled(colour)(base, status),
            None => theme::outlined(base, status),
        })
        .padding([theme::MD, theme::LG])
        .width(Length::Fixed(240.0))
        .height(Length::Fixed(theme::CONTROL_HEIGHT));

        let press = act.message();

        container(
            row![
                left,
                self.ring(
                    focus::Target::Primary,
                    (!self.busy).then(|| press.clone()),
                    match self.busy {
                        true => open,
                        false => open.on_press(press),
                    },
                ),
            ]
            .spacing(theme::XL)
            .align_y(Alignment::Center),
        )
        .padding(theme::LG)
        .style(theme::floating)
        .into()
    }

    /// The button state, which is core's decision and not the GUI's.
    ///
    /// The one thing decided here is the drive being away. That is a fact
    /// about the filesystem rather than about a build, and it outranks every
    /// other state: a product whose library is not mounted has nothing to
    /// launch, nothing worth downloading, and must not be repaired, the
    /// files are intact somewhere else.
    fn action(&self) -> ActionState {
        if let Some(drive) = self.offline_drive(&self.selected.code) {
            return ActionState::Offline { drive };
        }
        // Only this product's own transfer. Anything else is another game's
        // business, however loudly it is downloading.
        let transfer = self.download_for(&self.selected.code).map(Download::transfer);
        state::decide(&self.local, self.remote.as_ref(), transfer)
    }

    /// The name of this product's drive, when it is one that has gone away.
    fn offline_drive(&self, code: &ProductCode) -> Option<String> {
        let library = self.config.library_of(code)?;
        match self.drive_states.get(&library.id) {
            Some(library::State::Missing) => Some(library.label.clone()),
            _ => None,
        }
    }

    /// Re-read every library's marker, and whether anything is installed yet.
    /// One small file per drive, plus two directory reads.
    fn check_drives(&mut self) {
        self.drive_states = self
            .config
            .libraries
            .iter()
            .map(|library| (library.id.clone(), library.state()))
            .collect();
        self.library_empty = storage::library_is_empty(&self.layout);
        // Seeded here rather than when Storage is first measured: the field
        // lives on Setup now, which is the page a first run opens on, before
        // anything has been measured.
        if self.location_input.is_empty() {
            self.location_input = self.layout.root().display().to_string();
        }
    }

    /// Progress against the whole product, not against this run.
    ///
    /// A resumed run plans only what is missing, so run-relative numbers read
    /// as "starting over" every time a download is paused and picked up again.
    /// Refresh only when the finished product is the one on screen.
    fn reload_if_showing(&mut self, code: &ProductCode) -> Task<Message> {
        match code == &self.selected.code {
            true => self.reload(code.clone()),
            false => Task::none(),
        }
    }

    fn download_for(&self, code: &ProductCode) -> Option<&Download> {
        self.queue.iter().find(|d| &d.code == code)
    }

    /// The one transfer actually moving bytes, if any.
    /// What, other than a download, is writing to a game store right now.
    ///
    /// Every WoW flavour in a library shares one CASC store, and nothing in
    /// core stops two of Lightforge's own writers from opening it at once,
    /// core only checks the prefix for processes. So the line is held here:
    /// each of these is a writer, and none starts while another runs.
    fn maintenance(&self) -> Option<&'static str> {
        if self.scanning {
            Some("the scan")
        } else if self.repairing.is_some() {
            Some("the repair")
        } else if self.unused.values().any(|u| matches!(u, UnusedData::Freeing { .. })) {
            Some("freeing unused game data")
        } else if self.uninstall_dialog.as_ref().is_some_and(|d| d.working) {
            Some("the uninstall")
        } else if self.discarding.is_some() {
            Some("removing the cancelled download")
        } else {
            None
        }
    }

    /// [`Self::maintenance`], or a download still writing.
    fn store_writer(&self) -> Option<&'static str> {
        match self.running().is_some() || self.winding_down.is_some() {
            true => Some("the download"),
            false => self.maintenance(),
        }
    }

    /// Whether addon work may start: not while the user's own runs, nor
    /// while the unattended pass is writing the same folder and ledger.
    fn addons_locked(&self) -> bool {
        self.addons_busy || self.addon_auto_running
    }

    /// Whether a launch has to hold: a paused download is still winding
    /// down, or the launched product's own run has not stopped yet.
    fn launch_must_wait(&self) -> bool {
        self.winding_down.is_some()
            || self.queue.iter().any(|d| {
                Some(&d.code) == self.launching.as_ref() && matches!(d.stage, Stage::Running { .. })
            })
    }

    fn running(&self) -> Option<&Download> {
        self.queue.iter().find(|d| matches!(d.stage, Stage::Running { .. }))
    }

    /// Put the selected product in the queue, starting it if nothing is running.
    fn enqueue(&mut self) -> Task<Message> {
        self.enqueue_for(self.selected.code.clone())
    }

    /// Put `code` in the queue, starting it if nothing is running.
    fn enqueue_for(&mut self, code: ProductCode) -> Task<Message> {
        let label = self.label_of(&code);
        // The panel above should never offer this, but the download path is
        // the one that writes to disk, so it checks for itself.
        if self.kind_of(&code) == Kind::HandOff {
            self.error = Some(format!("{label} is installed through Battle.net"));
            return Task::none();
        }
        // The panel holds this product when its drive is away, but the queue
        // is also restored from disk and resumed without anyone pressing
        // anything, so the check that actually protects the files is here.
        if let Some(drive) = self.offline_drive(&code) {
            self.error = Some(format!("{drive} is not mounted"));
            return Task::none();
        }
        self.error = None;
        let showing = code == self.selected.code;

        // Already in the queue: a queued entry stays put, a running one is
        // left running, and a paused one is being resumed, as the same job,
        // so its bar continues. Taking a running entry out would orphan its
        // run, and the next one would start beside it on the same store.
        let mut job = Job::default();
        if let Some(existing) = self.queue.iter().position(|d| d.code == code) {
            if matches!(self.queue[existing].stage, Stage::Queued | Stage::Running { .. }) {
                return Task::none();
            }
            job = self.queue.remove(existing).resumed();
        }

        self.queue.push(Download {
            code,
            label,
            stage: Stage::Queued,
            sample: TransferState { stage: "queued".to_owned(), ..TransferState::default() },
            rate: Rate::default(),
            job,
        });
        let started = self.pump();
        // Not started: waiting on another download, or on this one's own
        // paused run to finish winding down.
        let waiting = self.queue.iter().any(|d| {
            d.code == self.selected.code && matches!(d.stage, Stage::Queued)
        });
        if waiting && showing {
            self.status = "queued…".to_owned();
        }
        started
    }

    /// Do whatever was waiting for the store to be free: close the window,
    /// remove a cancelled download's files, or start the first queued
    /// transfer, in that order. Called whenever a writer finishes.
    ///
    /// One at a time on purpose: two downloads would split one connection and
    /// write into the same CASC store, making both slower and both ETAs
    /// fiction.
    fn pump(&mut self) -> Task<Message> {
        // A launch waiting for the downloads pre-flight paused to come to
        // rest goes first. Not on the whole line being free: with
        // pause-while-playing off, another product's download is meant to
        // keep running beside the game.
        if self.launch_when_quiet.is_some() && !self.launch_must_wait() {
            let what = self.launch_when_quiet.take().expect("checked above");
            return self.start_launch(what);
        }
        // A deep scan reads the store a transfer would write, and maintenance
        // rewrites it. Neither refuses the download, they hold it: the queue
        // picks up once they are done.
        if self.store_writer().is_some() || self.deep_cancel.is_some() {
            return Task::none();
        }
        if let Some(id) = self.closing.take() {
            return iced::window::close(id);
        }
        if !self.delete_after_cancel.is_empty() {
            let code = self.delete_after_cancel.remove(0);
            return self.remove_cancelled(code);
        }
        if self.launch_when_quiet.is_some() {
            return Task::none();
        }
        let Some(next) = self.queue.iter().position(|d| matches!(d.stage, Stage::Queued)) else {
            return Task::none();
        };

        let cancel = Cancel::new();
        let (tx, rx) = tokio::sync::watch::channel(TransferState::default());
        let code = self.queue[next].code.clone();

        self.downloads.register(code.clone(), cancel.clone());
        self.queue[next].stage = Stage::Running { rx, cancel: cancel.clone() };
        // Whatever the line said before, "paused", most often, is over.
        if code == self.selected.code {
            self.status = "downloading…".to_owned();
        }
        self.queue[next].rate = Rate::default();
        self.queue[next].sample =
            TransferState { stage: "preparing".to_owned(), ..TransferState::default() };

        Task::perform(
            tasks::download(
                self.layout.clone(),
                code.clone(),
                self.config.region,
                self.config.locale.clone(),
                self.config.download_limit_bytes(),
                self.opted_optional(&code),
                cancel,
                tx,
            ),
            move |result| {
                let mapped = result.map(|summary| Complete {
                    complete: summary.complete,
                    cancelled: summary.outcome.cancelled,
                    files: summary.outcome.files_stored,
                    version: summary.version,
                });
                Message::Downloaded(code.clone(), mapped)
            },
        )
    }

    /// Stop one product's transfer, keeping what it reached.
    fn pause(&mut self, code: &ProductCode) {
        let Some(entry) = self.queue.iter_mut().find(|d| &d.code == code) else {
            return;
        };
        match &entry.stage {
            Stage::Running { cancel, .. } => {
                cancel.cancel();
                // Shown as paused immediately, the click is answered now,
                // not when the run finishes winding down.
                entry.stage = Stage::Paused(job_progress(entry.job, &entry.sample, 0));
                self.winding_down = Some(code.clone());
                self.status = "pausing…".to_owned();
            }
            // Never started, so there is nothing to wind down.
            Stage::Queued => entry.stage = Stage::Paused(state::Progress::default()),
            Stage::Paused(_) => {}
        }
    }

    /// Stop a transfer and take it out of the queue, keeping what it reached.
    ///
    /// The bytes stay in the store either way: this is the half of giving up
    /// that is always safe, and deleting them is a separate decision the
    /// dialog makes.
    fn drop_transfer(&mut self, code: &ProductCode) {
        self.pause(code);
        self.queue.retain(|d| &d.code != code);
        self.downloads.finished(code);
        if self.queue.is_empty() {
            self.manager_open = false;
        }
    }

    /// Delete what a given-up first install wrote.
    ///
    /// Not an uninstall: a WoW install holds one store for every flavour in
    /// it, so dropping the folder is only right when this was the only thing
    /// in there. Otherwise the entries this build wants and no remaining
    /// product wants are the ones to go, which is what
    /// [`tasks::discard_download`] works out.
    fn remove_cancelled(&mut self, code: ProductCode) -> Task<Message> {
        self.discarding = Some(code.clone());
        Task::perform(
            tasks::discard_download(
                self.http.clone(),
                self.layout.clone(),
                code.clone(),
                self.config.region,
            ),
            move |result| Message::CancelCleaned(code.clone(), result),
        )
    }

    /// Every message, with the status line watched on the way out.
    ///
    /// Lightforge only traces to stdout, so the status line is the closest
    /// thing to a log it has, and whatever it said in the minute before a
    /// bug report is usually the whole story. Recording it here rather than at
    /// the hundred-odd places that assign it means nothing can be forgotten.
    fn update(&mut self, message: Message) -> Task<Message> {
        let before = (self.status.clone(), self.error.clone());
        let task = self.step(message);

        if self.status != before.0 && !self.status.is_empty() {
            self.remember(self.status.clone());
        }
        if self.error != before.1 {
            if let Some(error) = self.error.clone() {
                self.remember(format!("error: {error}"));
            }
        }
        task
    }

    /// Keep the last [`HISTORY`] things the launcher said.
    fn remember(&mut self, line: String) {
        if self.history.back() == Some(&line) {
            return;
        }
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(line);
    }

    fn step(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.persist_queue();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LineSelected(line) => {
                // Picking from the overflow menu closes it: the tile is in the
                // band now, and the menu is answering a question already asked.
                self.close_panels();
                // The band is on News too, where choosing a game is a request
                // to go and look at it, leaving you on a page that says
                // nothing about the game you just picked would read as a dead
                // click.
                self.go_to(Screen::Dashboard);
                self.select_line(&line)
            }

            Message::BranchSelected(pick) => {
                // An unconfigured branch is offered, not hidden, picking it
                // is how it gets added, which beats hand-editing config.toml
                // to try a PTR for an afternoon.
                if !pick.configured {
                    let label = pick.label.clone();
                    self.config
                        .products
                        .push(Product::new(pick.code.clone(), label.clone()));
                    self.save_config();
                    self.choices.push(Choice { code: pick.code.clone(), label });
                }
                let Some(choice) = self.choices.iter().find(|c| c.code == pick.code).cloned() else {
                    return Task::none();
                };
                self.select(choice)
            }

            Message::Refreshed(code, refreshed) => {
                if code != self.selected.code {
                    return Task::none();
                }
                // A drive can go away between two refreshes, and this is the
                // moment the panel is being rebuilt anyway.
                self.check_drives();
                self.local = refreshed.local.clone();
                self.remote = refreshed.remote.clone();
                self.status = match &refreshed.offline {
                    // Rule 2 of the resolver: offline never blocks a launch, so
                    // it is a note, not an error.
                    Some(_) => "version service unreachable — offline".to_owned(),
                    None => refreshed
                        .remote
                        .as_ref()
                        .map(|r| format!("latest {}", r.versions_name))
                        .unwrap_or_default(),
                };
                Task::none()
            }

            Message::ResizeTraced(size) => {
                trace::mark(format_args!("resize {}x{}", size.width, size.height));
                Task::none()
            }

            Message::CachedTheme(code, themed) => {
                trace::mark(format_args!(
                    "cache-first theme for {}: {}, {} icon(s)",
                    code.as_str(),
                    match &themed.branding {
                        Some(_) => "hit",
                        None => "MISS — nothing usable on disk",
                    },
                    themed.icons.len(),
                ));
                // Strictly a gap-filler: the revalidated answer wins whenever
                // it happens to arrive first, so none of this overwrites it.
                if self.manifest.is_none() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                if self.icons.is_empty() {
                    self.icons = themed.icons;
                }
                let selected = code == self.selected.code;
                if selected && self.branding.is_none() {
                    self.branding = themed.branding;
                }
                match selected {
                    true => self.splash_decide(themed.warm),
                    false => Task::none(),
                }
            }

            Message::Themed(code, themed) => {
                trace::mark(format_args!(
                    "revalidated theme for {}: manifest {}, branding {}",
                    code.as_str(),
                    if themed.manifest.is_some() { "ok" } else { "failed" },
                    if themed.branding.is_some() { "ok" } else { "failed" },
                ));
                // The manifest describes every product, so it is kept even
                // when a switch has already moved on from the one it themed.
                if themed.manifest.is_some() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                // Same reasoning as the branding swap below. A revalidation
                // that answered 304 produces the same files decoded again;
                // assigning them hands the renderer brand-new texture ids for
                // pixels already on screen, and the whole band blinks after
                // every switch.
                if !themed.icons.is_empty() && !tasks::same_marks(&self.icons, &themed.icons) {
                    self.icons = themed.icons;
                }
                if code == self.selected.code {
                    // Only when it is actually different art. A revalidation
                    // that answered 304 resolves to the same files, already
                    // decoded and on screen, but assigning it hands iced new
                    // image handles, which it has to upload again, and the
                    // page blinks a second or two after a switch for no
                    // reason. [`Branding`] carries each asset's path and
                    // whether its bytes moved this run, so comparing it
                    // answers exactly that question.
                    let same = self.branding.as_ref().map(|art| &art.branding)
                        == themed.branding.as_ref().map(|art| &art.branding);
                    trace::mark(match same {
                        true => "branding unchanged, keeping the decoded art",
                        false => "branding changed, swapping the art",
                    });
                    if !same {
                        self.branding = themed.branding;
                    }
                }
                Task::none()
            }

            Message::Primary => {
                // Pressed on a game only being looked at: it joins the band.
                self.track_selected();
                match self.action() {
                    // A first install asks first; an update or a retry already
                    // has its answers.
                    ActionState::Install => self.open_install(),
                    ActionState::Update { .. } | ActionState::Error { .. } => self.enqueue(),
                    ActionState::Downloading(_) => {
                        // Pause: trip the token. The loop stops between chunks and
                        // the store keeps everything already written.
                        self.pause(&self.selected.code.clone());
                        Task::none()
                    }
                    ActionState::Paused(_) => self.enqueue(),
                    // Leaving the queue is pausing: nothing has started, so there
                    // is nothing to stop, but the intent is the same.
                    ActionState::Queued => {
                        self.pause(&self.selected.code.clone());
                        Task::none()
                    }
                    ActionState::Play => self.preflight(Launch::Game),
                    ActionState::Unknown => Task::none(),
                    // Nothing to download and nothing to repair: look for the
                    // drive again, and reload the product if it came back.
                    ActionState::Offline { .. } => {
                        self.refresh_drives();
                        match self.offline_drive(&self.selected.code) {
                            Some(drive) => {
                                self.status = format!("{drive} is still not mounted");
                                Task::none()
                            }
                            None => {
                                self.error = None;
                                self.reload(self.selected.code.clone())
                            }
                        }
                    }
                }
            }

            Message::Tick => {
                if let Some(rx) = &self.setup_rx {
                    self.setup_sample = rx.borrow().clone();
                }
                if let Some(rx) = &self.addon_rx {
                    self.addon_sample = rx.borrow().clone();
                }
                if let Some(rx) = &self.scan_rx {
                    self.scan_sample = *rx.borrow();
                }
                if let Some(entry) = self
                    .queue
                    .iter_mut()
                    .find(|d| matches!(d.stage, Stage::Running { .. }))
                {
                    if let Stage::Running { rx, .. } = &entry.stage {
                        entry.sample = rx.borrow().clone();
                    }
                    match (entry.job.total, entry.sample.remaining) {
                        (None, remaining) => entry.job.total = remaining,
                        // Restored from before a restart, and a new build has
                        // come out since: the saved size no longer describes
                        // this job, so it starts over from what is left.
                        (Some(total), Some(remaining)) if remaining > total => {
                            entry.job = Job { total: Some(remaining), reached: 0 };
                        }
                        _ => {}
                    }
                    // Rate is measured on what this run has stored; the
                    // baseline is a constant offset and would only mute the
                    // first sample.
                    entry.rate.sample(entry.sample.progress.bytes_stored);

                    // What this build offers is only knowable once its
                    // manifest has been read, so a run is the one chance to
                    // learn it. Cached per product so the settings panel has
                    // something real to list.
                    let (code, offered) =
                        (entry.code.clone(), entry.sample.optional.clone());
                    if !offered.is_empty() {
                        self.remember_optional(&code, offered);
                    }
                }
                Task::none()
            }

            Message::CloseRequested(id) => {
                // A deep scan only reads; there is nothing to wind down.
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                }
                // Closing mid-transfer must not orphan what was fetched: trip
                // the token, let the run checkpoint its store, then close.
                if let Some(code) = self.running().map(|d| d.code.clone()) {
                    // Saved as running a message ago; keep it that way.
                    self.queue_frozen = true;
                    self.pause(&code);
                }
                // Anything else writing to a store, a pause still winding
                // down, a repair, a compaction, is let finish too: `pump`
                // closes the window once the last of them reports back.
                match self.store_writer() {
                    Some(what) => {
                        self.closing = Some(id);
                        self.status = format!("finishing {what} safely…");
                        Task::none()
                    }
                    None => iced::window::close(id),
                }
            }

            Message::Downloaded(code, result) => {
                // A run a launch paused stops as cancelled, and is still owed
                // its restart when the game exits: `finished` would forget
                // that, which is why `Launched` never found anything owed.
                let paused_by_launch = matches!(&result, Ok(done) if done.cancelled)
                    && self.downloads.pending().contains(&code);
                if !paused_by_launch {
                    self.downloads.finished(&code);
                }
                if self.winding_down.as_ref() == Some(&code) {
                    self.winding_down = None;
                }
                // The run this was waiting on has stopped, so the store is
                // closed and its files can go, which `pump` does before it
                // starts anything else. Before everything below, which is all
                // about a queue entry that no longer exists.
                if self.delete_after_cancel.contains(&code) {
                    return self.pump();
                }

                // Status and errors describe the selected product. A transfer
                // that finished while the user was looking elsewhere updates
                // the queue and says nothing.
                let showing = code == self.selected.code;
                let entry = self.queue.iter().position(|d| d.code == code);
                let note = |app: &mut Self, text: String| {
                    if showing {
                        app.status = text;
                    }
                };

                // Resumed while this run was still winding down: the entry is
                // already a fresh, queued one. Marking it paused here, as
                // this handler once did, swallowed the resume, and whether it
                // happened came down to how fast the old run stopped.
                let resumed = entry.is_some_and(|at| matches!(self.queue[at].stage, Stage::Queued));
                let complete = matches!(&result, Ok(done) if done.complete);
                if resumed && !complete {
                    if let (Err(e), true) = (&result, showing) {
                        self.error = Some(e.clone());
                    }
                    return self.pump();
                }

                match result {
                    Ok(done) if done.complete => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
                        }
                        note(
                            self,
                            match done.version {
                                Some(v) => format!("installed {v}"),
                                None => format!("stored {} files", done.files),
                            },
                        );
                        Task::batch([self.pump(), self.reload_if_showing(&code)])
                    }
                    Ok(done) if done.cancelled => {
                        // Paused, so it keeps its place with what it reached.
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        note(self, "paused".to_owned());
                        self.pump()
                    }
                    Ok(_) => {
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        note(self, "stopped — run again to continue".to_owned());
                        Task::batch([self.pump(), self.reload_if_showing(&code)])
                    }
                    Err(e) => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
                        }
                        if showing {
                            self.error = Some(e);
                        }
                        self.pump()
                    }
                }
            }

            Message::Launched(result) => {
                self.busy = false;
                let launched = self.launching.take().unwrap_or_else(|| self.selected.code.clone());
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                // A launch may have paused transfers, its own product through
                // pre-flight's guard, and the rest of the queue through the
                // setting. Both are owed a restart, and resuming means
                // planning afresh.
                for code in std::mem::take(&mut self.resume_after_play) {
                    if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                        self.queue[at].stage = Stage::Queued;
                        self.queue[at].sample.stage = "queued".to_owned();
                    }
                }
                let owed = self.downloads.pending().contains(&launched);
                if owed {
                    if let Some(at) = self.queue.iter().position(|d| d.code == launched) {
                        self.queue[at].stage = Stage::Queued;
                    }
                }
                // A Battle.net session may have filled a cache that was
                // empty when the app started, the first non-WoW install in
                // a prefix is exactly when its catalogue first appears. It is
                // 25 ms of reading, so it costs less than remembering not to.
                Task::batch([
                    self.pump(),
                    self.reload(self.selected.code.clone()),
                    Task::perform(
                        tasks::game_families(
                            self.layout.clone(),
                            self.config.prefix_user.clone(),
                            self.config.region,
                        ),
                        |families| Message::FamiliesRead(Box::new(families)),
                    ),
                    // And what it has installed: an install handed over to
                    // Battle.net finishes in Battle.net, so Agent's registry
                    // is the only thing that knows it happened.
                    Task::perform(
                        tasks::installed_versions(self.layout.clone()),
                        Message::InstalledVersions,
                    ),
                ])
            }

            Message::HandoffStarted(result) => {
                match result {
                    // Found it. `busy` stays set and the queue stays paused
                    // for the length of the session, exactly as a direct
                    // launch does, the whole point of watching.
                    Ok(tasks::Handoff::Playing(game)) => {
                        self.status = format!(
                            "{} is running — this unlocks when it exits",
                            game.name,
                        );
                        // Not when the hand-off started: the client's window
                        // and its sign-in are things the user is meant to see.
                        Task::batch([
                            Task::perform(
                                tasks::await_game_exit(self.layout.clone(), game),
                                Message::Launched,
                            ),
                            self.minimize_if_asked(),
                        ])
                    }
                    // The client is up and waiting on a person, two-factor,
                    // an outage, an update of its own. Said plainly, with the
                    // client left running so it can be finished there.
                    // Closed while we were waiting for it to sign in. Not a
                    // failure to report as one, just nothing to hand off to.
                    Ok(tasks::Handoff::ClientClosed) => self.update(Message::Launched(Ok(
                        "Battle.net was closed before the game could start".to_owned(),
                    ))),
                    Ok(tasks::Handoff::NeedsAttention { minimized }) => {
                        // Where it is matters as much as that it needs
                        // something: a client started minimized is in the
                        // tray, and "its window" is not on screen to look at.
                        self.update(Message::Launched(Ok(match minimized {
                            true => "Battle.net needs attention — open it from the tray, \
                                     finish there and press Play"
                                .to_owned(),
                            false => "Battle.net needs attention — finish in its window and \
                                      press Play there"
                                .to_owned(),
                        })))
                    }
                    // Signed in, told to launch, and nothing came up.
                    Ok(tasks::Handoff::NothingStarted) => self.update(Message::Launched(Ok(
                        "Battle.net is open — no game started".to_owned(),
                    ))),
                    Err(e) => self.update(Message::Launched(Err(e))),
                }
            }

            Message::BattleNet => {
                self.track_selected();
                self.preflight(Launch::BattleNet)
            }

            Message::InstallViaBattleNet => {
                self.track_selected();
                self.preflight(Launch::Install)
            }

            Message::OpenBattleNet => {
                self.track_selected();
                self.preflight(Launch::Client)
            }

            Message::ToggleManager => {
                self.manager_open = !self.manager_open;
                if self.manager_open {
                    self.browse_open = false;
                    self.strip_open = false;
                }
                Task::none()
            }

            Message::ToggleHints => {
                self.hints = !self.hints;
                Task::none()
            }

            Message::ToggleStrip => {
                self.strip_open = !self.strip_open;
                if self.strip_open {
                    self.browse_open = false;
                    self.manager_open = false;
                }
                Task::none()
            }

            Message::OpenProduct(code) => {
                self.close_panels();
                self.go_to(Screen::Dashboard);
                let choice = self.choices.iter().find(|c| c.code == code).cloned().unwrap_or_else(|| {
                    let label = self
                        .catalog
                        .get(&code)
                        .map(|e| e.label.clone())
                        .unwrap_or_else(|| code.as_str().to_owned());
                    Choice { code, label }
                });
                self.select(choice)
            }

            Message::TileMenu(line) => {
                let open = self.tile_menu.as_ref() != Some(&line);
                self.close_panels();
                self.tile_menu = open.then_some(line);
                Task::none()
            }

            Message::TileEntered(line) => {
                self.tile_hover = Some(line);
                Task::none()
            }

            Message::TileLeft(line) => {
                // Only if it is still the one recorded: entering the next tile
                // can arrive before leaving the last.
                if self.tile_hover.as_ref() == Some(&line) {
                    self.tile_hover = None;
                }
                Task::none()
            }

            Message::TileGrabbed(line) => {
                self.tile_drag = Some(TileDrag { line, origin: None, last: 0.0, moving: false });
                Task::none()
            }

            Message::TileDragged(x) => {
                let Some(drag) = &mut self.tile_drag else {
                    return Task::none();
                };
                let Some(origin) = drag.origin else {
                    drag.origin = Some(x);
                    drag.last = x;
                    return Task::none();
                };
                drag.moving |= (x - origin).abs() > DRAG_SLOP;
                let heading = x - drag.last;
                drag.last = x;
                if !drag.moving {
                    return Task::none();
                }

                // Moved over another tile: take its place, the way a browser
                // tab does. Only in the direction of travel, or two tiles of
                // different widths trade places back and forth under a
                // cursor that has come to rest on the seam.
                let held = drag.line.clone();
                let Some(over) = self.tile_hover.clone().filter(|line| line != &held) else {
                    return Task::none();
                };
                let lines = self.lines();
                let (Some(from), Some(to)) = (
                    lines.iter().position(|l| l == &held),
                    lines.iter().position(|l| l == &over),
                ) else {
                    return Task::none();
                };
                if (to > from && heading > 0.0) || (to < from && heading < 0.0) {
                    self.move_line(from, to);
                }
                Task::none()
            }

            Message::TileDropped => {
                let Some(drag) = self.tile_drag.take() else {
                    return Task::none();
                };
                match drag.moving {
                    // Saved once, at the end: the order changes on every
                    // tile crossed, and the file needs only the last one.
                    true => {
                        self.save_config();
                        Task::none()
                    }
                    // A click is a press and a release on the same tile.
                    false if self.tile_hover.as_ref() == Some(&drag.line)
                        && drag.line != self.catalog.line_of(&self.selected.code) =>
                    {
                        self.update(Message::LineSelected(drag.line))
                    }
                    false => Task::none(),
                }
            }

            Message::ToggleBrowse => {
                self.browse_open = !self.browse_open;
                self.strip_open = false;
                if !self.browse_open {
                    return Task::none();
                }
                self.manager_open = false;
                // Read on open, so the list says what is already on the
                // machine without anything running in the background.
                Task::perform(
                    tasks::installed_versions(self.layout.clone()),
                    Message::InstalledVersions,
                )
            }

            Message::InstalledVersions(found) => {
                self.installed_versions = found;
                Task::none()
            }

            Message::KnownCodes(codes) => {
                self.known_codes = codes;
                self.refresh_catalog();
                Task::none()
            }

            Message::RemoveProduct(code) => {
                // A tile is a *line*, and its PTR and beta are branches behind
                // its version picker rather than games of their own, so
                // Remove takes the line with it. Dropping the head alone left
                // the tile on screen wearing whichever branch survived, which
                // is what someone removing the game has just said they do not
                // want.
                let line = self.catalog.line_of(&code);
                let going: Vec<ProductCode> = self
                    .choices
                    .iter()
                    .filter(|c| self.catalog.line_of(&c.code) == line)
                    .map(|c| c.code.clone())
                    .collect();

                // The header cannot be empty: with nothing tracked there is no
                // product to show and no way back to this panel.
                if going.len() >= self.choices.len() {
                    self.error =
                        Some("that is the only game left — add another first".to_owned());
                    return Task::none();
                }

                for code in &going {
                    // A transfer for a product nobody is tracking has nowhere
                    // to report to, so it stops here rather than running on
                    // unseen.
                    self.pause(code);
                    self.queue.retain(|d| &d.code != code);
                    self.downloads.finished(code);
                }

                self.config.products.retain(|p| !going.contains(&p.code));
                self.save_config();
                self.choices.retain(|c| !going.contains(&c.code));
                self.tile_menu = None;
                self.last_branch.remove(&line);

                match going.contains(&self.selected.code) {
                    // Standing on one of the ones just removed: move to
                    // whatever the header now starts with.
                    true => {
                        let next = self.choices[0].clone();
                        self.select(next)
                    }
                    false => Task::none(),
                }
            }

            Message::MutePatchNotes(mute) => {
                if let Some(notes) = self.patch_notes.as_mut() {
                    notes.mute = mute;
                }
                Task::none()
            }

            Message::NewsOnPlay(show) => {
                self.config.news_on_play = show;
                self.save_config();
                Task::none()
            }

            Message::ShowReleaseNotes(show) => {
                self.config.patch_notes_muted = !show;
                self.save_config();
                Task::none()
            }

            Message::DismissPatchNotes => {
                if let Some(notes) = self.patch_notes.take() {
                    // Recorded on the way out, so a start that was killed
                    // before anyone read them shows them again.
                    self.config.patch_notes_seen = Some(notes.release.version.clone());
                    self.config.patch_notes_muted = notes.mute;
                    self.save_config();
                }
                Task::none()
            }

            Message::UpdateChecked(found) => {
                let Some(available) = found else { return Task::none() };
                // Once per release. Someone who has read "beta.3 is out" and
                // decided to stay on beta.2 should not be told again on every
                // start until they give in.
                if self.config.update_seen.as_deref() == Some(available.latest.as_str()) {
                    return Task::none();
                }
                self.update_notice = Some(available);
                Task::none()
            }

            Message::DismissUpdate => {
                if let Some(available) = self.update_notice.take() {
                    self.config.update_seen = Some(available.latest);
                    self.save_config();
                }
                Task::none()
            }

            Message::OpenReleases => {
                let url = self
                    .update_notice
                    .as_ref()
                    .map(|a| a.url.clone())
                    .unwrap_or_else(|| lightforge_core::update::RELEASES_PAGE.to_owned());
                if let Err(e) = open::that_detached(&url) {
                    self.error = Some(format!("{url}: {e}"));
                }
                self.update(Message::DismissUpdate)
            }

            Message::CheckLauncherUpdates(on) => {
                self.config.check_for_updates = on;
                self.save_config();
                Task::none()
            }

            Message::AddToSteam => {
                self.add_to_steam();
                Task::none()
            }

            Message::ControllerLook(on) => {
                self.config.steamos_use = Some(match on {
                    true => SteamOsUse::Console,
                    false => SteamOsUse::Desktop,
                });
                self.save_config();
                self.window_mode()
            }

            Message::SplashDone(count) => {
                // The wizard's own count comes off this run: it is the same
                // fetch, and starting a second one on the first slide would
                // revalidate everything the splash just revalidated.
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.prefetched = Some(count);
                }
                match self.splash.is_some() {
                    true => self.close_splash_after_floor(),
                    false => Task::none(),
                }
            }

            Message::FinishSplash => self.finish_splash(),

            Message::AddProduct(code) => {
                if self.choices.iter().any(|c| c.code == code) {
                    return Task::none();
                }
                let label = self
                    .catalog
                    .get(&code)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());

                self.config.products.push(Product::new(code.clone(), label.clone()));
                self.save_config();
                self.choices.push(Choice { code, label });
                Task::none()
            }

            Message::PauseDownload(code) => {
                self.pause(&code);
                Task::none()
            }

            Message::ResumeDownload(code) => {
                // Its game is running and has the store open: it goes back in
                // the queue when the game exits, as pre-flight arranged.
                if self.busy && self.launching.as_ref() == Some(&code) {
                    if !self.resume_after_play.contains(&code) {
                        self.resume_after_play.push(code);
                    }
                    self.status = "resumes when the game closes".to_owned();
                    return Task::none();
                }
                // Back to the end of the queue rather than jumping the line:
                // whatever is running was started first.
                if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                    self.queue[at].stage = Stage::Queued;
                    self.queue[at].sample.stage = "queued".to_owned();
                }
                self.pump()
            }

            Message::CancelDownload(code) => {
                self.drop_transfer(&code);
                self.pump()
            }

            Message::AskCancelDownload(code) => {
                self.gear_open = false;
                if !self.queue.iter().any(|d| d.code == code) {
                    return Task::none();
                }
                let label = self
                    .queue
                    .iter()
                    .find(|d| d.code == code)
                    .map(|d| d.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());
                // What is on disk for *this* product, which is the thing that
                // decides whether giving up may delete anything. Read from the
                // panel only when the panel is showing this product; otherwise
                // the disk is asked directly.
                let installed = match code == self.selected.code {
                    true => self.local.version.clone(),
                    false => state::Local::inspect(&self.layout, &code).version,
                };
                self.cancel_dialog = Some(CancelDialog { code, label, installed });
                Task::none()
            }

            Message::CancelDownloadDismiss => {
                self.cancel_dialog = None;
                Task::none()
            }

            Message::CancelCleaned(code, result) => {
                self.discarding = None;
                let label = self
                    .catalog
                    .get(&code)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| code.short_name().to_owned());
                match result {
                    Ok(freed) => {
                        if code == self.selected.code {
                            self.status =
                                format!("cancelled {label} — freed {}", size_of(freed));
                        }
                    }
                    // The transfer is still cancelled; only the clean-up
                    // failed, and saying so is better than a status line
                    // claiming space came back.
                    Err(e) => {
                        self.error = Some(format!(
                            "{label} was cancelled, but its files could not be \
                             removed: {e}"
                        ));
                    }
                }
                Task::batch([
                    Task::perform(
                        tasks::installed_versions(self.layout.clone()),
                        Message::InstalledVersions,
                    ),
                    self.reload_if_showing(&code),
                    self.pump(),
                ])
            }

            Message::CancelDownloadConfirm { delete } => {
                let Some(dialog) = self.cancel_dialog.take() else {
                    return Task::none();
                };
                let code = dialog.code;
                // Finished while the dialog was open: there is no download
                // left to give up, and its files are now an install.
                if !self.queue.iter().any(|d| d.code == code) {
                    self.status = format!("{} had already finished; nothing was cancelled", dialog.label);
                    return Task::none();
                }
                self.drop_transfer(&code);
                self.status = match delete {
                    true => "cancelled — removing the files…".to_owned(),
                    false => "cancelled".to_owned(),
                };
                if !delete {
                    return self.pump();
                }
                // Removed once nothing is writing to the store any more: at
                // once for a queued entry, after the wind-down for a running
                // or just-paused one, after another product's run for a
                // store they share. See `pump`.
                self.delete_after_cancel.push(code);
                self.pump()
            }

            Message::PreflightDone(what, result) => {
                if let Err(e) = result {
                    self.busy = false;
                    self.launching = None;
                    self.error = Some(e);
                    return self.pump();
                }
                // Pre-flight trips the paused downloads' tokens and returns;
                // the runs themselves are still flushing what was in flight
                // and checkpointing their indices. The client opening the
                // store under them is the overlap this whole gate exists for.
                if self.launch_must_wait() {
                    self.launch_when_quiet = Some(what);
                    self.status = "pre-flight… waiting for the download to pause".to_owned();
                    return Task::none();
                }
                self.start_launch(what)
            }

            Message::ManageSubscription => {
                if let Err(e) = tasks::open_url(ACCOUNT_URL) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::OpenFiles => {
                let dir = self.layout.product_dir(&self.selected.code);
                let dir = if dir.is_dir() { dir } else { self.layout.games_root() };
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::Show(screen) => {
                // Asking for the page you are on is not a history step, or
                // back would walk through a page repeated however many times
                // its tab was clicked.
                self.go_to(screen);
                // Including when the page did not change: the menu entry was
                // still a click, and it has had its answer.
                self.close_panels();
                self.error = None;
                self.arrived()
            }

            Message::GoBack => {
                if let Some(screen) = self.visited.pop() {
                    self.forward.push(self.screen);
                    self.screen = screen;
                    self.error = None;
                }
                self.arrived()
            }

            Message::GoForward => {
                if let Some(screen) = self.forward.pop() {
                    self.visited.push(self.screen);
                    self.screen = screen;
                    self.error = None;
                }
                self.arrived()
            }

            Message::ToggleAppMenu => {
                self.app_menu_open = !self.app_menu_open;
                Task::none()
            }

            Message::OpenLink(url) => {
                if let Err(e) = tasks::open_url(url) {
                    self.error = Some(e);
                }
                self.close_panels();
                Task::none()
            }

            Message::OpenArticle(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::RefreshNews => self.fetch_news(true),

            Message::NewsLoaded(headlines) => {
                self.news_pending = false;
                self.news_asked = true;
                // A failed refresh keeps what is already on screen: headlines
                // from ten minutes ago are worth more than an empty page.
                if let Some(headlines) = headlines {
                    self.news = Some(*headlines);
                }
                self.fetch_news_images()
            }

            Message::NewsBackground(handle) => {
                self.news_bg = Some(handle);
                Task::none()
            }

            Message::NewsImages(images) => {
                // Only pictures the fetch found are in here, and only ones
                // not already held, so this adds, never replaces. Replacing
                // would hand the view a new handle for an identical picture
                // and re-upload every texture on the page.
                self.news_images.extend(images);
                Task::none()
            }

            Message::Exit => iced::exit(),

            Message::CopyRuntimeCommand(command) => iced::clipboard::write(command),

            Message::RecheckRuntime => Task::perform(
                provision::status(self.layout.clone(), self.config.clone()),
                |status| Message::SetupStatus(status.map(Box::new)),
            ),

            Message::SetupStatus(status) => {
                let status = status.map(|boxed| *boxed);
                let measure = match &status {
                    Some(status) => Task::perform(
                        provision::measure(
                            status.installed.iter().map(|b| b.path.clone()).collect(),
                        ),
                        Message::ProtonSizes,
                    ),
                    None => Task::none(),
                };
                // Open on settings, whose first card is setup, when the machine
                // cannot launch anything yet, but never yank the user out of
                // a screen they chose.
                if let Some(status) = &status {
                    // Not while the first run is up: it has a setup step of its
                    // own, and moving the screen underneath it would be where
                    // the user lands the moment they dismiss it.
                    if !status.is_ready()
                        && self.screen == Screen::Dashboard
                        && !self.setup_running
                        && self.wizard.is_none()
                    {
                        self.screen = Screen::Settings;
                        self.settings_tab = SettingsTab::Setup;
                    }
                }
                // Installing Battle.net is what puts its mark in the prefix,
                // and setup is the only thing that installs it, so this is the
                // one moment the answer can have changed.
                self.bnet_icon =
                    self.layout.battle_net_icon().and_then(|path| art::try_mark(&path));
                self.setup = status;
                measure
            }

            Message::Wizard(msg) => wizard::update(self, msg),

            Message::InstallBattleNet => {
                self.config.battle_net_declined = false;
                self.save_config();
                // Straight into the same run the button below starts: the two
                // steps before it are already done, so this is the one step.
                Task::done(Message::RunSetup)
            }

            Message::RunSetup => {
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;

                Task::perform(
                    provision::run(
                        self.layout.clone(),
                        self.config.clone(),
                        self.config.default_proton.version().map(str::to_owned),
                        tx,
                    ),
                    |result| Message::SetupFinished(result.map(Box::new)),
                )
            }

            Message::SetupFinished(result) => {
                self.setup_running = false;
                self.setup_rx = None;
                match result {
                    Ok(status) => {
                        let ready = status.is_ready();
                        self.setup = Some(*status);
                        if ready && self.wizard.is_none() {
                            self.screen = Screen::Dashboard;
                        }
                        // Provisioning changes what the disk holds, so the
                        // action button has to be re-resolved.
                        self.reload(self.selected.code.clone())
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::PinBattleNetProton(choice) => {
                self.config.battle_net_proton = ProtonPin::from(choice.0);
                self.save_config();
                Task::none()
            }

            Message::PinDefaultProton(choice) => {
                self.config.default_proton = ProtonPin::from(choice.0);
                self.save_config();
                Task::none()
            }

            Message::PinProductProton(code, choice) => {
                if let Some(product) = self.config.product_mut(&code) {
                    product.profile.proton_ge_version = choice.0;
                }
                self.save_config();
                Task::none()
            }

            Message::DownloadLimitChanged(value) => {
                // Digits only, so the field cannot hold something that is not
                // a speed. Clearing it is how you say "unlimited".
                self.limit_input = value.chars().filter(char::is_ascii_digit).collect();
                let trimmed = self.limit_input.trim();
                self.config.download_limit_mbps = match trimmed.parse::<u64>() {
                    Ok(0) | Err(_) => None,
                    Ok(mbps) => Some(mbps),
                };
                self.save_config();
                Task::none()
            }

            Message::BattleNetLimitRead(mbps) => {
                self.bnet_limit_input = mbps.map(|v| v.to_string()).unwrap_or_default();
                Task::none()
            }

            Message::BattleNetLimitChanged(value) => {
                self.bnet_limit_input = value.chars().filter(char::is_ascii_digit).collect();
                let mbps = self.bnet_limit_input.trim().parse::<u64>().ok().filter(|v| *v > 0);
                Task::perform(
                    tasks::set_battle_net_limit(
                        self.layout.clone(),
                        self.config.prefix_user.clone(),
                        mbps,
                    ),
                    Message::BattleNetLimitWritten,
                )
            }

            Message::BattleNetLimitWritten(result) => {
                if let Err(e) = result {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::AddonChannelDefault(choice) => {
                self.config.addon_channel = choice.0;
                self.save_config();
                // Nothing is fetched here: the new default applies at the next
                // check or update, which is also when it could matter.
                Task::none()
            }

            Message::AddonChannelFor(provider, id, choice) => {
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_channel(&options, &provider, &id, choice.0) {
                    self.error = Some(chain(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonMenu(which) => {
                self.addon_menu_open = which;
                Task::none()
            }

            Message::AddonIgnore(provider, id, ignored) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_ignored(&options, &provider, &id, ignored) {
                    self.error = Some(chain(&e));
                }
                // Re-list rather than patch the row in place: the ledger on
                // disk is the truth the rest of the page is drawn from.
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonAutoUpdate(provider, id, choice) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_auto_update(&options, &provider, &id, choice) {
                    self.error = Some(chain(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::FamiliesRead(families) => {
                self.families = *families;
                Task::none()
            }

            Message::ImportPinned(on) => {
                self.import_pinned = on;
                Task::none()
            }

            Message::AdoptAddons => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.addons_busy = true;
                self.error = None;
                self.addon_auto_note = Some("looking up what is installed…".to_owned());
                Task::perform(
                    addonwork::adopt(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                    ),
                    |result| Message::AdoptFinished(Box::new(result)),
                )
            }

            Message::AdoptFinished(result) => {
                self.addons_busy = false;
                match *result {
                    Ok(adopted) => {
                        self.addon_auto_note = Some(match adopted.len() {
                            0 => "nothing new recognised — every addon here is \
                                  either already managed or not on CurseForge"
                                .to_owned(),
                            1 => format!("{} is now managed", adopted[0].name),
                            n => format!("{n} addons are now managed"),
                        });
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::PickVersion(provider, id, name, installed) => {
                self.addon_menu_open = None;
                self.version_picker = Some(VersionPicker {
                    provider: provider.clone(),
                    id: id.clone(),
                    name,
                    installed,
                    builds: None,
                    problem: None,
                });
                Task::perform(
                    addonwork::versions(
                        self.http.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                    ),
                    |result| Message::VersionsListed(Box::new(result)),
                )
            }

            Message::VersionsListed(result) => {
                // Dropped on the floor if the picker was closed while the
                // request was out: an answer to a question nobody is asking
                // any more.
                if let Some(picker) = self.version_picker.as_mut() {
                    match *result {
                        Ok(builds) => picker.builds = Some(builds),
                        Err(e) => {
                            picker.builds = Some(Vec::new());
                            picker.problem = Some(e);
                        }
                    }
                }
                Task::none()
            }

            Message::CancelVersionPicker => {
                self.version_picker = None;
                Task::none()
            }

            Message::OpenGameSettings => {
                self.close_panels();
                self.game_settings = Some(GameSettings {
                    code: self.selected.code.clone(),
                    label: self.selected.label.clone(),
                    backups: None,
                    working: false,
                    confirm: None,
                    note: None,
                });
                self.list_backups()
            }

            Message::CloseGameSettings => {
                self.game_settings = None;
                Task::none()
            }

            Message::BackupsListed(result) => {
                // Dropped if the popup closed while the listing was out.
                if let Some(settings) = self.game_settings.as_mut() {
                    match result {
                        Ok(backups) => settings.backups = Some(backups),
                        Err(e) => {
                            settings.backups = Some(Vec::new());
                            settings.note = Some(Err(e));
                        }
                    }
                }
                Task::none()
            }

            Message::BackupNow => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                settings.working = true;
                settings.confirm = None;
                Task::perform(
                    tasks::create_backup(self.layout.clone(), settings.code.clone()),
                    Message::BackupFinished,
                )
            }

            Message::AskRestore(archive) => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.confirm = Some(archive);
                }
                Task::none()
            }

            Message::CancelRestore => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.confirm = None;
                }
                Task::none()
            }

            Message::ConfirmRestore => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                let Some(archive) = settings.confirm.take() else {
                    return Task::none();
                };
                settings.working = true;
                Task::perform(
                    tasks::restore_backup(self.layout.clone(), settings.code.clone(), archive),
                    Message::BackupFinished,
                )
            }

            Message::BackupFinished(result) => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                settings.working = false;
                settings.note = Some(result);
                // Either outcome can have changed the list: a restore adds
                // the backup of what it replaced.
                self.list_backups()
            }

            Message::ShowAddonDetail(provider, id, name) => {
                self.addon_menu_open = None;
                // Changelog first for something already installed, About for
                // something being browsed: the question is different in each
                // case, and the tab that answers it should not need a click.
                let installed = self.addon_is_installed(&provider, &id);
                self.addon_detail = Some(AddonDetail {
                    provider: provider.clone(),
                    id: id.clone(),
                    name,
                    loaded: None,
                    problem: None,
                    tab: match installed {
                        true => DetailTab::Changelog,
                        false => DetailTab::About,
                    },
                });
                Task::perform(
                    addonwork::detail(self.http.clone(), self.selected.code.clone(), provider, id),
                    |result| Message::AddonDetailLoaded(Box::new(result)),
                )
            }

            Message::AddonDetailLoaded(result) => {
                // Dropped if the card was closed while the request was out,
                // the same as the version picker: an answer to a question
                // nobody is asking any more.
                if let Some(detail) = self.addon_detail.as_mut() {
                    match *result {
                        Ok(loaded) => detail.loaded = Some(loaded),
                        Err(e) => detail.problem = Some(e),
                    }
                }
                Task::none()
            }

            Message::AddonDetailTab(tab) => {
                if let Some(detail) = self.addon_detail.as_mut() {
                    detail.tab = tab;
                }
                Task::none()
            }

            Message::CloseAddonDetail => {
                self.addon_detail = None;
                Task::none()
            }

            Message::InstallVersion(provider, id, file_id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.version_picker = None;
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::install_version(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        file_id,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::AddonPin(provider, id, file_id) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_pinned(&options, &provider, &id, file_id) {
                    self.error = Some(chain(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonAutoUpdateDefault(on) => {
                self.config.addon_auto_update = on;
                self.save_config();
                // Nothing is fetched here: the new default applies at the next
                // pass, which is also the first time it could matter.
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AutoUpdateAddons => {
                // Never against a running game, which writes SavedVariables
                // beside the addons and loads them, and never on top of work
                // the user started: both write the same directory. A game
                // that was already running when Lightforge started is not a
                // launch of ours, so the prefix itself is asked as well.
                if self.addon_auto_running
                    || self.addons_busy
                    || self.launching.is_some()
                    || self.addon_auto_done.contains(&self.selected.code)
                {
                    return Task::none();
                }
                self.addon_auto_done.push(self.selected.code.clone());
                self.addon_auto_running = true;
                let (tx, _rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                Task::perform(
                    addonwork::auto_update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                        self.config.addon_auto_update,
                        tx,
                    ),
                    |result| Message::AutoUpdateFinished(result.map(|r| r.map(Box::new))),
                )
            }

            Message::AutoUpdateFinished(result) => {
                self.addon_auto_running = false;
                let mut tasks = Vec::new();

                match result {
                    // Nothing to do, so nothing to say.
                    Ok(None) => {}
                    Ok(Some(report)) => {
                        if !report.installed.is_empty() {
                            self.addon_auto_note = Some(match report.installed.len() {
                                1 => format!("{} updated automatically", report.installed[0]),
                                n => format!("{n} addons updated automatically"),
                            });
                            tasks.push(Task::perform(
                                addonwork::list(
                                    self.layout.clone(),
                                    self.selected.code.clone(),
                                ),
                                |listing| Message::AddonsListed(Box::new(listing)),
                            ));
                        }
                    }
                    // Said quietly and only here: a launcher that cannot
                    // reach CurseForge is not a launcher that cannot play.
                    Err(e) => self.addon_auto_note = Some(format!("addon check failed: {e}")),
                }

                // Whoever was waiting on this can go now.
                if let Some(what) = self.launch_after_addons.take() {
                    tasks.push(self.preflight(what));
                }
                Task::batch(tasks)
            }

            Message::PauseWhilePlaying(on) => {
                self.config.pause_downloads_while_playing = on;
                self.save_config();
                Task::none()
            }

            Message::MinimizeOnLaunch(on) => {
                self.config.minimize_on_launch = on;
                self.save_config();
                Task::none()
            }

            Message::BattleNetMinimized(on) => {
                self.config.battle_net_minimized = on;
                self.save_config();
                Task::none()
            }

            Message::CheckProton => {
                // Runs with no build installed too: the release list is what
                // fills the install picker, and needing a build before you can
                // find one to install is the wrong way round.
                let installed = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.proton.as_ref())
                    .map(|p| p.version.clone());
                self.proton_notice = Some("checking…".to_owned());
                Task::perform(
                    provision::check_proton(self.http.clone(), installed),
                    Message::ProtonChecked,
                )
            }

            Message::ProtonChecked(result) => {
                match result {
                    Ok(releases) => {
                        self.proton_notice = Some(match &releases.update {
                            // Notify-only by policy: nothing updates itself.
                            Some(version) => format!("{version} is available"),
                            None => match self.setup.as_ref().and_then(|s| s.proton.as_ref()) {
                                Some(_) => "up to date".to_owned(),
                                None => "nothing installed yet".to_owned(),
                            },
                        });
                        self.proton_available = releases.available;
                    }
                    Err(e) => self.proton_notice = Some(format!("check failed: {e}")),
                }
                Task::none()
            }

            Message::ChooseProtonVersion(choice) => {
                self.proton_choice = choice.0;
                Task::none()
            }

            Message::InstallProton => {
                let Some(version) = self.proton_choice.clone() else {
                    return Task::none();
                };
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;
                // Stays on the Proton page, which is the point of the
                // progress now being drawn in the Builds card. It used to
                // switch to Settings > Setup because that was the only place
                // a bar existed, and pressing Install and landing somewhere
                // else reads as a misclick.
                //
                // `install_build` rather than `provision::run` with a pinned
                // version: that one goes on to the prefix and Battle.net
                // afterwards, so on a machine that is not set up yet, adding
                // a Proton build would quietly provision the whole thing.
                Task::perform(provision::install_build(version, tx), Message::ProtonInstalled)
            }

            Message::ProtonInstalled(result) => {
                self.setup_running = false;
                self.setup_rx = None;
                match result {
                    Ok(()) => {
                        self.proton_choice = None;
                        self.proton_notice = Some("installed".to_owned());
                        // What is installed just changed, so re-probe rather
                        // than editing the list we happen to be holding.
                        Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::RemoveProton(version) => {
                let Some(build) = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.installed.iter().find(|b| b.version == version))
                    .cloned()
                else {
                    return Task::none();
                };
                self.proton_notice = Some(format!("removing {version}…"));
                Task::perform(
                    provision::remove_proton(self.config.clone(), build),
                    Message::ProtonRemoved,
                )
            }

            Message::ProtonRemoved(result) => {
                match result {
                    Ok(note) => {
                        self.proton_notice = Some(note);
                        // What is installed just changed, so re-probe rather
                        // than editing the list we happen to be holding.
                        return Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        );
                    }
                    Err(e) => self.proton_notice = Some(e),
                }
                Task::none()
            }

            Message::ProtonSizes(sizes) => {
                self.proton_sizes = sizes.into_iter().collect();
                Task::none()
            }

            Message::AddonsListed(listing) => {
                // A slow listing for the product shown before a switch.
                if listing.dir != self.layout.addons_dir(&self.selected.code) {
                    return Task::none();
                }
                if let Some(problem) = &listing.problem {
                    self.error = Some(format!("the addon list cannot be read: {problem}"));
                }
                self.addons = *listing;
                Task::none()
            }

            Message::TokenChanged(value) => {
                self.token_input = value;
                Task::none()
            }

            Message::ImportToken => {
                if self.addons_locked() {
                    return Task::none();
                }
                let token = self.token_input.trim().to_owned();
                if token.is_empty() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Import;
                self.error = None;

                Task::perform(
                    addonwork::import(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        token,
                        self.config.addon_channel,
                        self.import_pinned,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::UpdateAddons => {
                if self.addons_locked() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::AddonsFinished(result) => {
                self.addons_busy = false;
                self.addon_rx = None;
                match result {
                    Ok(report) => {
                        self.addon_report = Some(*report);
                        // What was installed is no longer an update waiting.
                        let installed = &self.addon_report.as_ref().expect("just set").installed;
                        self.updates.retain(|u| !installed.contains(&u.name));
                        // The list on screen is now stale in both directions.
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::ArmRemoveAddon(provider, id) => {
                self.addon_remove_armed = Some((provider, id));
                Task::none()
            }

            Message::ExportAddons => {
                match addonwork::export(&self.layout, &self.selected.code) {
                    Ok(token) => {
                        self.export_token = token;
                        self.status = "export ready".to_owned();
                    }
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }

            Message::CopyExport => iced::clipboard::write(self.export_token.clone()),

            Message::ExportEdited(text) => {
                self.export_token = text;
                Task::none()
            }

            Message::CancelRemoveAddon => {
                self.addon_remove_armed = None;
                Task::none()
            }

            Message::AddonFilterChanged(text) => {
                self.addon_filter = text;
                Task::none()
            }

            Message::OpenAddonsDir => {
                if let Err(e) = tasks::open_path(self.addons.dir.clone()) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::RemoveAddon(provider, id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.addon_remove_armed = None;
                Task::perform(
                    addonwork::remove(
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                    ),
                    Message::AddonRemoved,
                )
            }

            Message::AddonRemoved(result) => {
                match result {
                    Ok(folders) => self.status = format!("removed {}", folders.join(", ")),
                    Err(e) => self.error = Some(e),
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::OpenUrl(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::AddonTab(tab) => {
                self.addon_tab = tab;
                self.error = None;
                // Leaving the list disarms it: a Remove waiting to be
                // confirmed must not survive out of sight of the row it
                // belongs to.
                self.addon_remove_armed = None;
                // Browse opens on the most-downloaded addons instead of on an
                // empty list: "what is there?" has a useful answer before
                // anybody types, and the catalogue it comes out of is already
                // on disk.
                match tab == AddonTab::Browse && self.search_input.trim().is_empty() {
                    true => self.popular_if_unasked(),
                    false => Task::none(),
                }
            }

            Message::SettingsTab(tab) => {
                self.settings_tab = tab;
                self.error = None;
                match tab {
                    // Asked on the way in, so the install picker is
                    // populated by the time anyone looks at it. It used to
                    // be hidden behind a button, and the caption where it
                    // should have been told you to go and press that button
                    // in a different section of the page.
                    //
                    // Only when there is nothing to show: the answer is
                    // cached by ETag, but a request per tab click is still a
                    // request per tab click.
                    SettingsTab::Proton if self.proton_available.is_empty() => {
                        Task::done(Message::CheckProton)
                    }
                    SettingsTab::Storage => {
                        // Scanned when the page opens rather than polled: a
                        // drive appearing while the page is shut is not news,
                        // and this is also where a library that moved gets
                        // found again.
                        self.refresh_drives();
                        self.drive_notice = None;
                        Task::batch([self.measure_storage(), self.check_unused()])
                    }
                    _ => Task::none(),
                }
            }

            Message::StorageMeasured(report) => {
                self.storage_measuring = false;
                self.storage = Some(*report);
                Task::none()
            }

            Message::InstallOffered(code, result) => {
                if let Ok(offerings) = result.as_ref() {
                    self.remember_optional(&code, offerings.optional.clone());
                }
                if let Some(dialog) = self.install_dialog.as_mut().filter(|d| d.code == code) {
                    dialog.stage = match *result {
                        Ok(offerings) => DialogStage::Ready(Box::new(offerings)),
                        Err(e) => DialogStage::Failed(e),
                    };
                }
                Task::none()
            }

            Message::InstallHighRes(on) => {
                if let Some(dialog) = self.install_dialog.as_mut() {
                    dialog.high_res = on;
                }
                Task::none()
            }

            Message::InstallTarget(index) => {
                if let Some(dialog) = self.install_dialog.as_mut() {
                    if index < dialog.targets.len() {
                        dialog.chosen = index;
                    }
                }
                Task::none()
            }

            Message::RefreshDrives => {
                self.refresh_drives();
                self.measure_storage()
            }

            Message::AddDrive(index) => {
                match self.add_drive(index) {
                    Ok(()) => {
                        self.refresh_drives();
                        self.measure_storage()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::SetDefaultDrive(id) => {
                // The button is held for a drive that is away, but this also
                // decides where the *next* install goes and what Battle.net is
                // told, neither is worth pointing at a drive that is not here.
                if let Some(drive) = self
                    .config
                    .libraries
                    .iter()
                    .find(|l| l.id == id)
                    .filter(|l| self.drive_states.get(&l.id) == Some(&library::State::Missing))
                    .map(|l| l.label.clone())
                {
                    self.error = Some(format!("{drive} is not mounted"));
                    return Task::none();
                }
                for library in &mut self.config.libraries {
                    library.is_default = library.id == id;
                }
                self.save_config();
                self.rebuild_layout();
                // Battle.net's `DefaultInstallPath` follows the default drive,
                // and it is only written where nothing in the prefix is running,
                // so it is re-asserted by the next pre-flight, not here. The
                // client itself does not move: it lives in the prefix.
                self.status = "new installs will go to this drive".to_owned();
                self.measure_storage()
            }

            Message::RemoveDrive(id) => {
                match self.remove_drive(&id) {
                    // `refresh_drives` as well as the measurement: a drive
                    // that is no longer ours goes back to being a partition
                    // the page can offer, and that list is only rebuilt here.
                    // Without it the row stays gone until you leave Settings
                    // and come back.
                    Ok(()) => {
                        self.refresh_drives();
                        self.measure_storage()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::ClearDrive(id) => {
                let Some(library) = self.config.libraries.iter().find(|l| l.id == id).cloned()
                else {
                    return Task::none();
                };
                self.drive_notice = Some((id.clone(), "clearing…".to_owned()));
                let layout = self.layout.clone();
                Task::perform(
                    tasks::clear_library(layout, library),
                    move |result| Message::DriveCleared(id.clone(), result),
                )
            }

            Message::UnusedPlanned(id, result) => {
                if !matches!(self.unused.get(&id), Some(UnusedData::Freeing { .. })) {
                    let state = match result {
                        Ok(plan) => UnusedData::Ready(plan),
                        Err(e) => UnusedData::Said(e),
                    };
                    self.unused.insert(id, state);
                }
                Task::none()
            }

            Message::FreeUnused(id) => {
                let Some(library) = self.config.libraries.iter().find(|l| l.id == id) else {
                    return Task::none();
                };
                let install = self.layout.wow_install_in(library);
                if self.downloading_into(&install) {
                    return Task::none();
                }
                // Compaction moves and deletes archives for minutes; nothing
                // else may write the store meanwhile, and the client may not
                // have it open. `pump` and pre-flight both hold while it runs.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first"));
                    return Task::none();
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".to_owned());
                    return Task::none();
                }
                let total = match self.unused.get(&id) {
                    Some(UnusedData::Ready(plan)) => plan.moved,
                    _ => return Task::none(),
                };
                let copied = Arc::<std::sync::atomic::AtomicU64>::default();
                self.unused.insert(id.clone(), UnusedData::Freeing { copied: copied.clone(), total });
                Task::perform(
                    tasks::free_unused(self.http.clone(), self.layout.clone(), install, copied),
                    move |result| Message::UnusedFreed(id.clone(), result),
                )
            }

            Message::UnusedFreed(id, result) => {
                let note = match result {
                    Ok(bytes) => format!("freed {}", size_of(bytes)),
                    Err(e) => e,
                };
                self.unused.insert(id, UnusedData::Said(note));
                Task::batch([self.measure_storage(), self.pump()])
            }

            Message::DriveCleared(id, result) => {
                self.drive_notice = Some(match result {
                    Ok(0) => (id, "nothing to clear".to_owned()),
                    Ok(bytes) => (id, format!("freed {}", size_of(bytes))),
                    Err(e) => (id, e),
                });
                self.measure_storage()
            }

            Message::OpenDrive(id) => {
                if let Some(library) = self.config.libraries.iter().find(|l| l.id == id) {
                    // A drive with nothing installed on it yet has no `games/`
                    // folder, so this opens the nearest thing that does exist,
                    // the place the folder would appear.
                    let path = library
                        .games()
                        .ancestors()
                        .find(|p| p.is_dir())
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| library.games().to_path_buf());
                    if let Err(e) = tasks::open_path(path) {
                        self.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::InstallCancel => {
                self.install_dialog = None;
                Task::none()
            }

            Message::InstallStart => self.start_install(),

            Message::AskUninstall => {
                self.gear_open = false;
                let code = self.selected.code.clone();
                self.uninstall_dialog = Some(UninstallDialog {
                    code: code.clone(),
                    label: self.selected.label.clone(),
                    plan: None,
                    working: false,
                    copied: Arc::default(),
                });
                Task::perform(
                    tasks::uninstall_plan(self.http.clone(), self.layout.clone(), code.clone()),
                    move |plan| Message::UninstallPlanned(code.clone(), Box::new(plan)),
                )
            }

            Message::UninstallPlanned(code, plan) => {
                if let Some(dialog) = self.uninstall_dialog.as_mut().filter(|d| d.code == code) {
                    dialog.plan = Some(*plan);
                }
                Task::none()
            }

            Message::DowngradeCancel => {
                self.downgrade_dialog = None;
                Task::none()
            }

            Message::DowngradeConfirm => {
                let Some(dialog) = self.downgrade_dialog.take() else {
                    return Task::none();
                };
                // Remembered before the launch, so the retry below walks
                // straight past the guard it just answered.
                self.downgrade_ok.insert(dialog.pick);
                self.preflight(dialog.what)
            }

            Message::UninstallCancel => {
                self.uninstall_dialog = None;
                Task::none()
            }

            Message::UninstallConfirm => {
                // Any writer, not only this product's own: the store is shared,
                // and a reclaim rewrites archives another flavour is writing.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first"));
                    return Task::none();
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".to_owned());
                    return Task::none();
                }
                let Some(dialog) = self.uninstall_dialog.as_mut() else {
                    return Task::none();
                };
                dialog.working = true;
                let code = dialog.code.clone();
                let http = self.http.clone();
                Task::perform(
                    tasks::uninstall(http, self.layout.clone(), code.clone(), dialog.copied.clone()),
                    move |result| Message::Uninstalled(code.clone(), result),
                )
            }

            Message::Uninstalled(code, result) => {
                let label = self.uninstall_dialog.take().map(|d| d.label).unwrap_or_default();
                match result {
                    Ok(freed) => {
                        // A paused or queued download of it would only put
                        // the files straight back.
                        self.queue.retain(|d| d.code != code);
                        if code == self.selected.code {
                            self.status = format!("uninstalled {label} — freed {}", size_of(freed));
                        }
                        Task::batch([
                            Task::perform(
                                tasks::installed_versions(self.layout.clone()),
                                Message::InstalledVersions,
                            ),
                            self.reload_if_showing(&code),
                            self.pump(),
                        ])
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.pump()
                    }
                }
            }

            Message::ClearCache => {
                let Some(dirs) = self.dirs.clone() else {
                    return Task::none();
                };
                self.cache_notice = Some("clearing…".to_owned());
                Task::perform(tasks::clear_cache(dirs), Message::CacheCleared)
            }

            Message::CacheCleared(result) => {
                self.cache_notice = Some(match result {
                    Ok(freed) => format!("freed {}", size_of(freed)),
                    Err(e) => e,
                });
                self.measure_storage()
            }

            Message::OpenConfigFolder => {
                if let Some(dirs) = &self.dirs {
                    if let Err(e) = tasks::open_path(dirs.config_dir().to_path_buf()) {
                        self.status = e;
                    }
                }
                Task::none()
            }

            Message::OpenLibrary => {
                let root = self.layout.root().to_path_buf();
                let dir = match root.is_dir() {
                    true => root,
                    // Not created yet: show where it would go.
                    false => root
                        .ancestors()
                        .find(|p| p.is_dir())
                        .map(Path::to_path_buf)
                        .unwrap_or(root),
                };
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e);
                }
                Task::none()
            }

            Message::LocationChanged(value) => {
                self.location_input = value;
                Task::none()
            }

            Message::UseLocation => {
                match self.relocate() {
                    Ok(()) => Task::batch([
                        Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        ),
                        self.measure_storage(),
                        self.reload(self.selected.code.clone()),
                    ]),
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::SearchChanged(value) => {
                self.search_input = value;
                self.search_seq += 1;
                let seq = self.search_seq;

                // Typing is the search. The button stays. It is how you ask
                // again after a provider was down, and pressing Enter has to
                // do something, but nobody should have to reach for it.
                //
                // Two letters is where a search stops matching half the
                // catalogue, and the pause is long enough that a word typed at
                // speed goes out once rather than letter by letter.
                // Emptying the box is not "no search", it is the question
                // the tab opened with: back to the popular list.
                if self.search_input.trim().is_empty() {
                    return self.popular_if_unasked();
                }
                if self.search_input.trim().chars().count() < SEARCH_MIN {
                    return Task::none();
                }
                Task::perform(
                    async move {
                        tokio::time::sleep(SEARCH_DEBOUNCE).await;
                        seq
                    },
                    Message::SearchSettled,
                )
            }

            Message::SearchSettled(seq) => {
                // A later keystroke has already scheduled its own search.
                match seq == self.search_seq && !self.searching {
                    true => self.update(Message::RunSearch),
                    false => Task::none(),
                }
            }

            Message::RunSearch => {
                // An empty query is not a no-op: it asks for the most
                // downloaded addons, which is what the tab shows before the
                // first keystroke.
                let query = self.search_input.trim().to_owned();
                self.searching = true;
                Task::perform(
                    addonwork::search(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        query,
                    ),
                    |outcome| Message::SearchDone(Box::new(outcome)),
                )
            }

            Message::SearchDone(outcome) => {
                self.searching = false;
                // Ignore a reply for a query the user has already moved past.
                if outcome.query == self.search_input.trim() {
                    self.search = *outcome;
                }
                Task::none()
            }

            Message::InstallAddon(provider, id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.error = None;

                Task::perform(
                    addonwork::install_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::ReinstallAddon(provider, id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.addon_menu_open = None;
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::reinstall_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::CheckAddonUpdates => {
                let code = self.selected.code.clone();
                Task::perform(
                    addonwork::check_updates(
                        self.http.clone(),
                        self.layout.clone(),
                        code.clone(),
                        self.config.addon_channel,
                    ),
                    move |result| Message::AddonUpdatesChecked(code.clone(), result),
                )
            }

            Message::AddonUpdatesChecked(code, result) => {
                if code != self.selected.code {
                    return Task::none();
                }
                match result {
                    Ok(statuses) => self.updates = statuses,
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }

            Message::CopyDebug => {
                let action = self.action();
                let info = DebugInfo::gather(
                    &self.layout,
                    &self.selected.code,
                    &format!("{action:?}"),
                    tasks::proton_for(&self.selected.code).as_ref(),
                );
                self.status = "debug info copied".to_owned();
                iced::clipboard::write(info.render())
            }

            Message::SystemInfo(info) => {
                self.system = Some(*info);
                Task::none()
            }

            Message::OpenReport(kind) => {
                self.close_panels();
                // Gathered here because this is where the four things
                // `DebugInfo::gather` needs are in scope, and gathered once:
                // the card shows exactly the text that gets sent, not a
                // second rendering of it.
                let shown: Vec<String> = match kind {
                    ReportKind::Bug => {
                        self.history.iter().map(|line| lightforge_core::debug::scrub(line)).collect()
                    }
                    ReportKind::Feedback => Vec::new(),
                };
                let (debug, tags) = match kind {
                    ReportKind::Bug => {
                        let action = self.action();
                        let mut info = DebugInfo::gather(
                            &self.layout,
                            &self.selected.code,
                            &format!("{action:?}"),
                            tasks::proton_for(&self.selected.code).as_ref(),
                        );
                        // What the renderer picked is the one thing core
                        // cannot know, so the GUI fills it in here.
                        if let Some(system) = &self.system {
                            info.graphics = Some(system.graphics_adapter.clone());
                            info.graphics_backend = Some(system.graphics_backend.clone());
                            info.cpu = Some(system.cpu_brand.clone());
                            info.memory = Some(system.memory_total);
                        }
                        let tags = report_tags(&info);
                        // The recent status lines go into the shown block as
                        // well as into the breadcrumbs, because the card
                        // promises that this is what gets sent, and they are
                        // scrubbed like everything else, since a failure
                        // message can carry a path.
                        let mut text = info.render();
                        if !shown.is_empty() {
                            text.push_str("\n--- recent activity ---\n");
                            for line in &shown {
                                text.push_str(line);
                                text.push('\n');
                            }
                        }
                        (text, tags)
                    }
                    ReportKind::Feedback => (String::new(), Vec::new()),
                };
                self.report_dialog = Some(ReportDialog {
                    kind,
                    note: iced::widget::text_editor::Content::new(),
                    category: None,
                    tags,
                    shot: None,
                    // Remembered between reports: someone who gave it once
                    // should not have to type it again to answer a follow-up.
                    contact: self.config.contact.clone().unwrap_or_default(),
                    base: debug.clone(),
                    debug,
                    history: shown,
                    stage: ReportStage::Writing,
                });
                Task::none()
            }

            Message::ReportNote(action) => {
                // `text_editor` has no "read only" of its own, so a report on
                // its way out is held here instead.
                if let Some(dialog) = self
                    .report_dialog
                    .as_mut()
                    .filter(|d| !matches!(d.stage, ReportStage::Sending | ReportStage::Sent))
                {
                    // Editing is how someone gets out of the cooldown: the
                    // text that was refused is no longer the text they have.
                    if dialog.stage == ReportStage::TooSoon && action.is_edit() {
                        dialog.stage = ReportStage::Writing;
                    }
                    dialog.note.perform(action);
                }
                Task::none()
            }

            Message::ReportShotPick => Task::perform(
                // The desktop's own dialog, asked for over the XDG portal.
                //
                // Spoken directly rather than through a file-dialog crate:
                // `rfd` drives the portal through `libdbus` on a thread of its
                // own, and inside this process that call never returns, the
                // dialog does not appear and the thread sits in a futex. The
                // same call from a bare test binary works, so it is this
                // process it dislikes, not the machine. `ashpd` is pure Rust
                // over the `zbus` connection iced already builds.
                async {
                    let request = ashpd::desktop::file_chooser::OpenFileRequest::default()
                        .title("Choose a screenshot")
                        .modal(true)
                        .multiple(false)
                        .filter(
                            ashpd::desktop::file_chooser::FileFilter::new("Images")
                                .mimetype("image/png")
                                .mimetype("image/jpeg")
                                .mimetype("image/webp")
                                .mimetype("image/bmp"),
                        );

                    let files = request.send().await.ok()?.response().ok()?;
                    // Cancelling comes back as an empty list, which is not an
                    // error and means nothing happened. `to_file_path` rather
                    // than the URL's own `path`, which is still percent-encoded,
                    // and "Screenshot 2026-10-09.png" is the normal case.
                    files.uris().first().and_then(|uri| uri.to_file_path().ok())
                },
                Message::ReportShotPicked,
            ),

            Message::ReportShotPicked(path) => {
                // Cancelling is the common answer and means nothing happened.
                let Some(path) = path else { return Task::none() };
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || art::load_shot(&path))
                            .await
                            .unwrap_or_else(|e| Err(chain(&e)))
                            .map(Box::new)
                    },
                    Message::ReportShotLoaded,
                )
            }

            Message::ReportShotLoaded(result) => {
                match result {
                    Ok(shot) => {
                        if let Some(dialog) = self.report_dialog.as_mut() {
                            dialog.shot = Some(*shot);
                        }
                    }
                    Err(e) => self.status = format!("that file is not a screenshot: {e}"),
                }
                Task::none()
            }

            Message::ReportCopy => {
                let Some(dialog) = self.report_dialog.as_ref() else {
                    return Task::none();
                };
                // What they wrote first, then the diagnostics: pasted into a
                // chat window it reads as a report rather than a dump.
                let mut text = dialog.note.text();
                if !dialog.debug.is_empty() {
                    text.push('\n');
                    text.push_str(&dialog.debug);
                }
                self.status = "report copied".to_owned();
                iced::clipboard::write(text)
            }

            Message::ReportShotCleared => {
                if let Some(dialog) = self.report_dialog.as_mut() {
                    dialog.shot = None;
                }
                Task::none()
            }

            Message::ReportContact(contact) => {
                let Some(dialog) = self.report_dialog.as_mut() else {
                    return Task::none();
                };
                dialog.contact = contact;
                let contact = dialog.contact.trim().to_owned();
                self.config.contact = (!contact.is_empty()).then_some(contact);
                self.save_config();
                Task::none()
            }

            Message::ReportCategory(category) => {
                // Gathered before the dialog is borrowed: the attachment reads
                // the addon folder, the queue and the config off `self`.
                let attached = match self.report_dialog.as_ref().map(|d| d.kind) {
                    Some(ReportKind::Bug) => self.report_attachment(category),
                    _ => String::new(),
                };
                if let Some(dialog) = self
                    .report_dialog
                    .as_mut()
                    .filter(|d| !matches!(d.stage, ReportStage::Sending | ReportStage::Sent))
                {
                    dialog.category = Some(category);
                    if dialog.kind == ReportKind::Bug {
                        dialog.debug = format!("{}{attached}", dialog.base);
                    }
                }
                Task::none()
            }

            Message::ReportSend => {
                let Some(dialog) = self.report_dialog.as_mut() else {
                    return Task::none();
                };
                let note = dialog.note.text();
                // From a failed send too: that is the "Try again" button.
                let ready = matches!(dialog.stage, ReportStage::Writing | ReportStage::Failed);
                let Some(category) = dialog.category.filter(|_| ready && dialog.can_send()) else {
                    return Task::none();
                };
                // The same words twice in five minutes is the Send button
                // being pressed again, not a second problem.
                let repeat = self
                    .last_report
                    .as_ref()
                    .is_some_and(|(last, when)| last == &note && when.elapsed() < REPORT_COOLDOWN);
                if repeat {
                    dialog.stage = ReportStage::TooSoon;
                    return Task::none();
                }

                dialog.stage = ReportStage::Sending;
                let report = report::Report {
                    kind: match dialog.kind {
                        ReportKind::Bug => report::Kind::Bug(dialog.debug.clone()),
                        ReportKind::Feedback => report::Kind::Feedback,
                    },
                    category,
                    note: note.clone(),
                    contact: dialog.contact.clone(),
                    tags: dialog.tags.clone(),
                    shot: dialog.shot.as_ref().map(|shot| report::Shot {
                        name: shot.name.clone(),
                        bytes: shot.bytes.clone(),
                        media_type: shot.media_type,
                    }),
                    history: dialog.history.clone(),
                };
                Task::perform(report::send(report, self.http.clone()), Message::ReportSent)
            }

            Message::ReportSent(arrived) => {
                if let Some(dialog) = self.report_dialog.as_mut() {
                    dialog.stage = match arrived {
                        true => ReportStage::Sent,
                        false => ReportStage::Failed,
                    };
                    // Only a report that arrived counts against the cooldown:
                    // one that failed has to be sendable again at once.
                    if arrived {
                        self.last_report = Some((dialog.note.text(), Instant::now()));
                    }
                }
                self.status = match arrived {
                    true => "report sent".to_owned(),
                    false => "the report could not be sent".to_owned(),
                };
                Task::none()
            }

            Message::ReportCancel => {
                self.report_dialog = None;
                Task::none()
            }

            Message::ShowSettingsTab(tab) => {
                self.settings_tab = tab;
                self.update(Message::Show(Screen::Settings))
            }

            Message::Tweak(scope, which, on) => {
                self.edit_tweaks(scope, |tweaks| match which {
                    // Stored as an explicit refusal rather than a bare `false`:
                    // the default is on wherever the tool exists, so "off" has
                    // to be recorded to survive a restart.
                    Tweak::GamePerformance => tweaks.game_performance = Some(on),
                    Tweak::GameMode => tweaks.gamemode = on,
                    Tweak::MangoHud => tweaks.mangohud = on,
                    Tweak::SkipAddonProfiling => tweaks.skip_addon_profiling = on,
                    Tweak::ReduceRaidEffects => tweaks.reduce_raid_effects = on,
                    Tweak::FasterZoneLoads => tweaks.faster_zone_loads = on,
                    Tweak::RawMouse => tweaks.raw_mouse = on,
                    Tweak::NoScreenEffects => tweaks.no_screen_effects = on,
                    Tweak::MaxDrawDistance => tweaks.max_draw_distance = on,
                    Tweak::NoClothPhysics => tweaks.no_cloth_physics = on,
                    Tweak::SoftwareCursor => tweaks.software_cursor = on,
                    Tweak::ClipCursor => tweaks.clip_cursor = on,
                    Tweak::NoMouseAcceleration => tweaks.no_mouse_acceleration = on,
                    Tweak::ShowCursorInRelativeMode => {
                        tweaks.show_cursor_in_relative_mode = on
                    }
                    Tweak::Wayland => tweaks.wayland = on,
                    Tweak::Hdr => tweaks.hdr = on,
                    Tweak::BigShaderCache => tweaks.big_shader_cache = on,
                });
                Task::none()
            }

            Message::TweakPreset(preset) => {
                self.edit_tweaks(Scope::Product, |tweaks| preset.apply(tweaks));
                Task::none()
            }

            Message::TweakCustom => {
                self.edit_tweaks(Scope::Product, Tweaks::clear_managed);
                Task::none()
            }

            Message::TweakEnvToggle(scope) => {
                match scope {
                    Scope::Product => self.env_open = !self.env_open,
                    Scope::BattleNet => self.bnet_env_open = !self.bnet_env_open,
                }
                Task::none()
            }

            Message::TweakApi(ApiChoice(api)) => {
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.graphics_api = api);
                Task::none()
            }

            Message::TweakFpsCap(value) => {
                // Digits only: a frame cap that is not a number is not a cap.
                // Clearing it is how you say uncapped.
                self.fps_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.fps_cap = cap);
                Task::none()
            }

            Message::TweakFpsCapBackground(value) => {
                self.fps_bk_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_bk_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.fps_cap_background = cap);
                Task::none()
            }

            Message::TweakWaylandMonitor(scope, value) => {
                // A connector name, so no spaces: `DP-1`, `HDMI-A-1`.
                let field = match scope {
                    Scope::Product => &mut self.monitor_input,
                    Scope::BattleNet => &mut self.bnet_monitor_input,
                };
                *field = value.chars().filter(|c| !c.is_whitespace()).collect();
                let name = (!field.is_empty()).then(|| field.clone());
                self.edit_tweaks(scope, |tweaks| tweaks.wayland_monitor = name);
                Task::none()
            }

            Message::TweakEnvKey(value) => {
                // No spaces or `=`: the two halves are separate fields here,
                // and a key holding either would not survive the environment.
                self.env_key_input =
                    value.chars().filter(|c| !c.is_whitespace() && *c != '=').collect();
                Task::none()
            }

            Message::TweakEnvValue(value) => {
                self.env_value_input = value;
                Task::none()
            }

            Message::TweakEnvAdd(scope) => {
                let key = std::mem::take(&mut self.env_key_input);
                let value = std::mem::take(&mut self.env_value_input);
                if !key.is_empty() {
                    self.edit_tweaks(scope, |tweaks| {
                        tweaks.env.insert(key, value);
                    });
                }
                Task::none()
            }

            Message::TweakEnvRemove(scope, key) => {
                self.edit_tweaks(scope, |tweaks| {
                    tweaks.env.remove(&key);
                });
                Task::none()
            }

            Message::OptionalContent(name, on) => {
                self.set_optional(&self.selected.code.clone(), &name, on);
                Task::none()
            }

            Message::TogglePicker(key) => {
                self.picker_open = match self.picker_open.as_ref() == Some(&key) {
                    true => None,
                    false => Some(key),
                };
                Task::none()
            }

            Message::Picked(message) => {
                self.picker_open = None;
                self.update(*message)
            }

            Message::ToggleGear => {
                self.gear_open = !self.gear_open;
                Task::none()
            }

            Message::Dismiss => {
                self.close_panels();
                Task::none()
            }

            Message::Nav(from, input) => {
                let now = Instant::now();
                trace::pad(format_args!("nav {from:?} {input:?} focused={}", self.window_focused));
                if from == NavFrom::Pad && !self.window_focused {
                    return Task::none();
                }
                // Steam can deliver one press both ways: as keys from its
                // layout, and raw from a pad it did not name as translated.
                // Whichever arrives first keeps the floor while presses keep
                // coming, so neither a tap nor a held repeat counts twice.
                if let Some((last, at)) = self.nav_last {
                    if last != from && now.duration_since(at) < NAV_DOUBLED {
                        trace::pad("  dropped: doubled");
                        return Task::none();
                    }
                }
                self.nav_last = Some((from, now));
                // A field with the cursor in it takes a pad's buttons the way
                // it takes the keys: arrows in a field are the field's, and
                // only Escape gets past it. So B leaves the field and stays
                // on it, A asks for the keyboard again, and an arrow leaves
                // and goes on from there.
                if from == NavFrom::Pad && !self.keys_belong_elsewhere() {
                    if let Some(field) = focus::typing_in() {
                        // The ring can be somewhere else by now, moved off
                        // the field while it kept the cursor: A is then for
                        // what the ring is on.
                        let on_field = self.focus.as_ref().is_none_or(|f| *f == field);
                        match input {
                            pad::Input::Back => {
                                self.focus = Some(field.clone());
                                focus::leave(field);
                                self.steam_keyboard_closed();
                                return Task::none();
                            }
                            pad::Input::Press if on_field => {
                                self.focus = Some(field.clone());
                                self.steam_keyboard(&field);
                                return Task::none();
                            }
                            pad::Input::Press => focus::leave(field),
                            pad::Input::Step(_) => {
                                if on_field {
                                    self.focus = Some(field.clone());
                                }
                                focus::leave(field);
                            }
                        }
                    }
                }
                self.update(match input {
                    pad::Input::Step(step) => Message::FocusStep(step),
                    pad::Input::Press => Message::FocusPress,
                    pad::Input::Back => Message::FocusBack,
                })
            }

            Message::TypingDone(field) => {
                self.focus = Some(field);
                self.steam_keyboard_closed();
                Task::none()
            }

            Message::WindowFocused(focused) => {
                self.window_focused = focused;
                Task::none()
            }

            Message::FocusStep(step) => {
                if self.keys_belong_elsewhere() {
                    return Task::none();
                }
                let spots = focus::live();
                // The first press only shows where the ring is.
                let current = self.focus.clone().filter(|t| spots.iter().any(|p| &p.target == t));
                let Some(current) = current else {
                    self.focus = self.focus_start(&spots);
                    return match self.focus {
                        Some(_) => Task::none(),
                        // Nothing to select, only something to read.
                        None => self.scroll_page(step, None),
                    };
                };
                match focus::step(&spots, &current, step) {
                    Some(next) => {
                        Self::scroll_into_view(&next);
                        self.focus = Some(next);
                        Task::none()
                    }
                    None => self.scroll_page(step, Some(&current)),
                }
            }

            Message::FocusPress => {
                if self.keys_belong_elsewhere() {
                    return Task::none();
                }
                let spots = focus::live();
                let current = self.focus.clone().filter(|t| spots.iter().any(|p| &p.target == t));
                let Some(current) = current else {
                    self.focus = self.focus_start(&spots);
                    return Task::none();
                };
                // A Settings tab is a way in as much as a choice: the ring
                // goes on to the page it opens, and Escape brings it back.
                if let focus::Target::SettingsTab(_) = current {
                    self.focus_into = Some(SETTINGS_PAGE);
                }
                self.focus_pressed_at =
                    spots.iter().find(|p| p.target == current).map(|p| (p.bounds, self.screen));
                let pressed = match focus::spot(&current).map(|spot| spot.press) {
                    Some(focus::Press::Send(message)) => self.update(message),
                    Some(focus::Press::Click) => {
                        // A controller has no keys to type with. Steam has a
                        // keyboard of its own, and this is how to ask for it.
                        if focus::spot(&current).is_some_and(|spot| spot.field) {
                            self.steam_keyboard(&current);
                        }
                        focus::click(current);
                        Task::none()
                    }
                    Some(focus::Press::Off) | None => return Task::none(),
                };
                // Whatever the press opened or closed, the ring follows once
                // it has been drawn.
                self.focus_entering = FOCUS_ENTER_FRAMES;
                Task::batch([pressed, focus_settle()])
            }

            Message::FocusSettle => {
                if self.focus_entering == 0 {
                    return Task::none();
                }
                self.focus_entering -= 1;
                if let Some(region) = self.focus_into {
                    if let Some(first) = focus::first(&focus::live_in(region)) {
                        self.focus_into = None;
                        self.focus_entering = 0;
                        self.focus = Some(first);
                        return Task::none();
                    }
                }
                let spots = focus::live();
                let drawn = |target: &focus::Target| spots.iter().any(|p| &p.target == target);
                match self.focus.clone() {
                    // Covered by a menu or a dialog that has just opened, or
                    // gone with the one that just closed: on to what is on
                    // top now. Only a control still there under the cover is
                    // somewhere to come back to; one that is gone, Install
                    // inside a dialog once pressed, leaves the way back as
                    // it was.
                    Some(was) if !drawn(&was) && !spots.is_empty() => {
                        let covered = focus::spot(&was).is_some();
                        let at = self
                            .focus_pressed_at
                            .take()
                            .filter(|(_, screen)| *screen == self.screen)
                            .map(|(at, _)| at);
                        self.focus = match (covered, at) {
                            // Pressed and gone, on the same page, with nothing
                            // open that it leads back to: whatever is now
                            // nearest to where it was, which after Keep is the
                            // Remove beside it.
                            (false, Some(at)) if self.focus_return.as_ref().is_none_or(|t| !drawn(t)) => {
                                focus::nearest(&spots, at)
                            }
                            _ => self.focus_start(&spots),
                        };
                        if covered {
                            self.focus_return = Some(was);
                        }
                        self.focus_entering = 0;
                        Task::none()
                    }
                    Some(_) => focus_settle(),
                    None => Task::none(),
                }
            }

            Message::FocusBack => {
                if self.keys_belong_elsewhere() {
                    return Task::none();
                }
                // Out of a Settings page, back onto its tab in the list.
                let in_page = self.focus.as_ref().and_then(focus::spot).and_then(|spot| spot.region)
                    == Some(SETTINGS_PAGE);
                if self.screen == Screen::Settings
                    && in_page
                    && self.dialog_dismissal().is_none()
                    && !self.panel_open()
                {
                    self.focus = Some(focus::Target::SettingsTab(self.settings_tab));
                    return Task::none();
                }
                let closed = match (self.dialog_dismissal(), self.panel_open()) {
                    (Some(dismiss), _) => self.update(dismiss),
                    // Out of Settings from its list of sections: back to Play,
                    // wherever Settings was opened from. Its pages are left
                    // for the list the same way, above, so B twice from any
                    // switch is the dashboard.
                    (None, false) if self.screen == Screen::Settings && self.wizard.is_none() => {
                        self.update(Message::Show(Screen::Dashboard))
                    }
                    // Setup has no pages behind it: Escape is its Back, and
                    // never its way out.
                    (None, false) if self.wizard.is_some() => {
                        self.update(Message::Wizard(wizard::Msg::Back))
                    }
                    (None, true) => {
                        self.close_panels();
                        Task::none()
                    }
                    (None, false) if !self.visited.is_empty() => self.update(Message::GoBack),
                    (None, false) => return Task::none(),
                };
                self.focus_entering = FOCUS_ENTER_FRAMES;
                Task::batch([closed, focus_settle()])
            }

            Message::FocusHidden => {
                if let Some(was) = self.focus.take() {
                    self.focus_resume = Some(was);
                }
                self.focus_return = None;
                self.focus_entering = 0;
                Task::none()
            }

            Message::MenuClick => Task::none(),

            Message::CheckForUpdates => {
                self.gear_open = false;
                self.busy = true;
                self.error = None;
                self.status = "checking for updates…".to_owned();
                let code = self.selected.code.clone();
                Task::perform(
                    tasks::offerings(
                        self.http.clone(),
                        code.clone(),
                        self.config.region,
                        self.opted_optional(&code),
                        None,
                    ),
                    move |result| Message::CheckedForUpdates(code.clone(), Box::new(result)),
                )
            }

            Message::CheckedForUpdates(code, result) => {
                self.busy = false;
                match *result {
                    Ok(offerings) => {
                        // The point of the round trip: the optional list is
                        // only knowable from the manifest, and this is the
                        // cheap way to learn it. Kept whichever product is
                        // showing by now; the rest only describes this one.
                        self.remember_optional(&code, offerings.optional.clone());
                        if code != self.selected.code {
                            return Task::none();
                        }
                        let current = self.local.version.as_deref() == Some(
                            offerings.remote.versions_name.as_str(),
                        );
                        self.remote = Some(offerings.remote.clone());
                        self.status = match current {
                            true => format!("up to date on {}", offerings.remote.versions_name),
                            false => {
                                format!("{} is available", offerings.remote.versions_name)
                            }
                        };
                    }
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }

            Message::ScanAndRepair => {
                self.gear_open = false;
                // It drops damaged entries from the indices, so nothing else
                // may be writing the store, and the client may not have it
                // open.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first"));
                    return Task::none();
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".to_owned());
                    return Task::none();
                }
                self.error = None;
                self.scanning = true;
                self.scan_for = Some(self.selected.code.clone());
                self.busy = true;
                self.scan_sample = Default::default();
                self.status = "checking stored files against their headers…".to_owned();

                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(
                    tasks::scan_and_repair(self.layout.clone(), self.selected.code.clone(), tx),
                    |result| Message::Scanned(Box::new(result)),
                )
            }

            Message::DeepScan => {
                self.gear_open = false;
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                    self.status = "stopping the deep scan…".to_owned();
                    return Task::none();
                }
                // Every WoW flavour shares one store, so any transfer at all
                // is writing to what this would read.
                let transferring = self.winding_down.is_some()
                    || self
                        .queue
                        .iter()
                        .any(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued));
                if transferring {
                    self.error = Some(
                        "pause downloads first — they write to the files being checked".to_owned(),
                    );
                    return Task::none();
                }
                if let Some(what) = self.maintenance() {
                    self.error = Some(format!("wait for {what} to finish first"));
                    return Task::none();
                }
                self.scan_for = Some(self.selected.code.clone());

                self.error = None;
                self.scanning = true;
                self.owed_repair = None;
                self.scan_sample = Default::default();
                self.status = "checking every stored byte — the game can stay open".to_owned();

                let cancel = Cancel::new();
                self.deep_cancel = Some(cancel.clone());
                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(tasks::deep_scan(self.layout.clone(), self.selected.code.clone(), cancel, tx), |result| {
                    Message::DeepScanned(Box::new(result))
                })
            }

            Message::DeepScanned(result) => {
                self.scanning = false;
                let scanned = self.scan_for.take().unwrap_or_else(|| self.selected.code.clone());
                self.scan_rx = None;
                let stopped = self.deep_cancel.take().is_some_and(|c| c.is_cancelled());
                match *result {
                    Err(_) if stopped => {
                        self.status = "deep scan stopped".to_owned();
                        self.pump()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.pump()
                    }
                    Ok(report) if report.is_healthy() => {
                        self.status = format!(
                            "{} files checked down to the byte — none damaged",
                            report.checked
                        );
                        self.pump()
                    }
                    // Repaired before the queue is released: a transfer
                    // landing first would move the store and void the
                    // findings.
                    Ok(report) => {
                        self.status = format!(
                            "{} damaged of {} — repairing…",
                            report.damaged.len(),
                            report.checked
                        );
                        self.owed_repair = Some((scanned, Box::new(report)));
                        self.update(Message::RepairDamaged)
                    }
                }
            }

            Message::RepairDamaged => {
                self.gear_open = false;
                let Some((code, report)) = self.owed_repair.clone() else {
                    return Task::none();
                };
                // Kept owed, and said so, rather than started: the game has
                // the store open, or something else is writing it.
                let held = match self.store_writer() {
                    Some(what) => Some(format!("wait for {what} to finish")),
                    None => self.launching.is_some().then(|| "close the game".to_owned()),
                };
                if let Some(held) = held {
                    self.status = format!(
                        "{} damaged files found — {held}, then choose Repair Damaged Files",
                        report.damaged.len()
                    );
                    return self.pump();
                }
                // A writer of its own rather than `busy`, which belongs to a
                // launch and a session: clearing it here once re-enabled Play
                // in the middle of one.
                self.repairing = Some(code.clone());
                Task::perform(tasks::repair(self.layout.clone(), code, *report), Message::Repaired)
            }

            Message::Repaired(result) => {
                let code = self.repairing.take().unwrap_or_else(|| self.selected.code.clone());
                match result {
                    Ok(removed) => {
                        self.owed_repair = None;
                        if code == self.selected.code {
                            self.status =
                                format!("{removed} damaged files dropped — fetching them again");
                        }
                        self.enqueue_for(code)
                    }
                    Err(tasks::Unrepaired::InUse) => {
                        let damaged = self.owed_repair.as_ref().map_or(0, |(_, r)| r.damaged.len());
                        self.status = format!(
                            "{damaged} damaged files found — close the game and Battle.net, \
                             then choose Repair Damaged Files"
                        );
                        self.pump()
                    }
                    Err(tasks::Unrepaired::Failed(e)) => {
                        self.owed_repair = None;
                        self.error = Some(e);
                        self.pump()
                    }
                }
            }

            Message::Scanned(result) => {
                self.scanning = false;
                self.busy = false;
                self.scan_rx = None;
                let scanned = self.scan_for.take().unwrap_or_else(|| self.selected.code.clone());
                match *result {
                    Err(e) => {
                        self.error = Some(e);
                        self.pump()
                    }
                    Ok(scan) => {
                        // Either way the download path runs: repair works by
                        // removing damaged entries so the planner counts them
                        // missing, and the same plan also covers whatever was
                        // never fetched in the first place.
                        self.status = match scan.damaged {
                            0 => format!(
                                "{} files verified — none damaged; checking for anything missing",
                                scan.checked
                            ),
                            n => format!(
                                "{n} damaged of {} — {} dropped, fetching them again",
                                scan.checked, scan.removed
                            ),
                        };
                        self.enqueue_for(scanned)
                    }
                }
            }

            Message::ClearShaderCache => {
                let Some(dirs) = self.dirs.as_ref() else {
                    self.error = Some("no cache directory available".to_owned());
                    return Task::none();
                };
                // Per product, so this cannot take another product's shaders
                // with it, the reason the cache is keyed that way at all.
                let dir = dirs.shader_cache_dir(&self.selected.code);
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => self.status = "shader cache cleared".to_owned(),
                    // Nothing there is the desired end state, not a failure.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.status = "no shader cache to clear".to_owned()
                    }
                    Err(e) => self.error = Some(format!("{}: {e}", dir.display())),
                }
                Task::none()
            }
        }
    }

    /// Record what a build offers, if it has changed.
    ///
    /// Saved rather than held in memory: the list is the only thing standing
    /// between the settings panel and a hardcoded guess, and a guess is what
    /// put a 37 GiB texture pack in a plan nobody asked for.
    /// The optional downloads a product is opted into, limited to the ones
    /// Lightforge offers; see [`OFFERED_OPTIONAL`]. An opt-in to anything
    /// else, from before that list existed, is ignored rather than fetched
    /// behind a switch the user can no longer see.
    fn opted_optional(&self, code: &ProductCode) -> Vec<String> {
        offered_only(
            self.config.product(code).map(|p| p.profile.optional_content.as_slice()).unwrap_or(&[]),
        )
    }

    fn remember_optional(&mut self, code: &ProductCode, offered: Vec<String>) {
        let changed = self
            .config
            .product(code)
            .is_some_and(|p| p.profile.optional_available != offered);
        if !changed {
            return;
        }
        if let Some(product) = self.config.product_mut(code) {
            product.profile.optional_available = offered;
        }
        self.save_config();
    }

    /// This product's performance settings.
    fn tweaks(&self) -> Tweaks {
        self.tweaks_of(Scope::Product)
    }

    /// One scope's performance settings.
    fn tweaks_of(&self, scope: Scope) -> Tweaks {
        match scope {
            Scope::Product => self
                .config
                .product(&self.selected.code)
                .map(|p| p.profile.tweaks.clone())
                .unwrap_or_default(),
            Scope::BattleNet => self.config.battle_net_tweaks.clone(),
        }
    }

    /// Change them and persist, which is what every switch on a panel does.
    fn edit_tweaks(&mut self, scope: Scope, edit: impl FnOnce(&mut Tweaks)) {
        match scope {
            Scope::Product => {
                if let Some(product) = self.config.product_mut(&self.selected.code) {
                    edit(&mut product.profile.tweaks);
                }
            }
            Scope::BattleNet => edit(&mut self.config.battle_net_tweaks),
        }
        self.save_config();
    }

    /// Refill the numeric fields from the selected product's settings.
    fn load_tweak_inputs(&mut self) {
        let tweaks = self.tweaks();
        self.fps_input = tweaks.fps_cap.map(|v| v.to_string()).unwrap_or_default();
        self.fps_bk_input =
            tweaks.fps_cap_background.map(|v| v.to_string()).unwrap_or_default();
        self.monitor_input = tweaks.wayland_monitor.clone().unwrap_or_default();
        self.bnet_monitor_input =
            self.config.battle_net_tweaks.wayland_monitor.clone().unwrap_or_default();
        self.env_key_input.clear();
        self.env_value_input.clear();
    }

    /// Persist the config, surfacing a write failure rather than swallowing it.
    /// The queue as the next start should find it.
    fn queue_snapshot(&self) -> transfers::Queue {
        // Paused only because a game is running: owed a restart, and one the
        // next start should give it rather than leave it waiting.
        let owed = {
            let mut owed = self.downloads.pending();
            owed.extend(self.resume_after_play.iter().cloned());
            owed
        };
        let entries = self
            .queue
            .iter()
            .map(|d| {
                let (standing, reached) = match &d.stage {
                    _ if owed.contains(&d.code) => (Standing::Running, 0),
                    Stage::Running { .. } => (Standing::Running, 0),
                    Stage::Queued => (Standing::Queued, d.job.reached),
                    Stage::Paused(p) => (Standing::Paused, p.downloaded_bytes),
                };
                Saved { code: d.code.clone(), standing, job_bytes: d.job.total, reached }
            })
            .collect();
        transfers::Queue { entries }
    }

    /// Write the queue if it changed. Called after every message, which is
    /// what makes a crash or a kill keep it too, not only a clean close.
    fn persist_queue(&mut self) {
        if self.queue_frozen || self.config_error.is_some() {
            return;
        }
        let snapshot = self.queue_snapshot();
        if snapshot == self.saved_queue {
            return;
        }
        if let Some(dirs) = &self.dirs {
            if let Err(e) = snapshot.save(&dirs.queue_file()) {
                trace::mark(format_args!("saving the queue failed: {e}"));
                return;
            }
        }
        self.saved_queue = snapshot;
    }

    /// Put back the queue the last run left, for products still configured.
    fn restore_queue(&mut self) {
        let Some(dirs) = &self.dirs else {
            return;
        };
        let saved = match transfers::Queue::load(&dirs.queue_file()) {
            Ok(saved) => saved,
            Err(e) => {
                trace::mark(format_args!("ignoring the saved queue: {e}"));
                return;
            }
        };
        for entry in &saved.entries {
            let Some(choice) = self.choices.iter().find(|c| c.code == entry.code) else {
                continue;
            };
            let job = Job { total: entry.job_bytes, reached: entry.reached };
            let stage = match entry.standing {
                Standing::Running | Standing::Queued => Stage::Queued,
                Standing::Paused => Stage::Paused(state::Progress {
                    downloaded_bytes: entry.reached,
                    total_bytes: entry.job_bytes.unwrap_or(0),
                    bytes_per_second: 0,
                }),
            };
            self.queue.push(Download {
                code: entry.code.clone(),
                label: choice.label.clone(),
                stage,
                sample: TransferState { stage: "queued".to_owned(), ..TransferState::default() },
                rate: Rate::default(),
                job,
            });
        }
        self.saved_queue = self.queue_snapshot();
    }

    /// Open the install dialog and start pricing the build.
    fn open_install(&mut self) -> Task<Message> {
        let code = self.selected.code.clone();
        let targets = self.install_targets(&code);
        // Opens on the drive this product is already assigned to, else the
        // default one, not on the first row, which would silently re-target a
        // product whose files are elsewhere.
        let assigned = self
            .config
            .product(&code)
            .and_then(|p| p.profile.library.clone());
        let chosen = assigned
            .and_then(|id| targets.iter().position(|t| t.id == id))
            .unwrap_or(0);
        self.install_dialog = Some(InstallDialog {
            code: code.clone(),
            label: self.selected.label.clone(),
            stage: DialogStage::Calculating,
            high_res: self.opted_optional(&code).iter().any(|t| t == HIGH_RES),
            targets,
            chosen,
        });
        // Priced with no opt-ins, since the dialog adds each extra's own size,
        // and against the store already there, so files another WoW version
        // shares are not counted.
        Task::perform(
            tasks::offerings(
                self.http.clone(),
                code.clone(),
                self.config.region,
                Vec::new(),
                Some(self.layout.wow_install(&code)),
            ),
            move |result| Message::InstallOffered(code.clone(), Box::new(result)),
        )
    }

    /// Apply the dialog's answers and queue the install.
    fn start_install(&mut self) -> Task<Message> {
        let Some(dialog) = self.install_dialog.clone() else {
            return Task::none();
        };
        if dialog.code != self.selected.code {
            self.install_dialog = None;
            return Task::none();
        }
        // Re-checked rather than trusted from when the dialog opened: the
        // drive may have gone away while the user read the size.
        if let Some(target) = dialog.target() {
            if let Err(e) = self.target_install(&dialog.code, &target.id) {
                self.error = Some(e);
                return Task::none();
            }
        }
        if let DialogStage::Ready(offerings) = &dialog.stage {
            let offered = offerings.optional_bytes.iter().any(|(t, b)| t == HIGH_RES && *b > 0);
            if offered {
                self.set_optional(&dialog.code, HIGH_RES, dialog.high_res);
            }
        }
        self.install_dialog = None;
        self.enqueue()
    }

    /// Every drive this product could be installed to, main first.
    fn install_targets(&self, code: &ProductCode) -> Vec<InstallTarget> {
        let mut targets: Vec<InstallTarget> = self
            .config
            .libraries
            .iter()
            .map(|library| InstallTarget {
                id: library.id.clone(),
                label: library.label.clone(),
                install: self.layout.wow_install_in(library),
                disk: library.disk(),
                state: library.state(),
                shares: storage::holds_wow(&self.layout, library),
            })
            .collect();
        let main = self.config.libraries.iter().position(|l| l.is_default).unwrap_or(0);
        if main < targets.len() {
            let first = targets.remove(main);
            targets.insert(0, first);
        }
        let _ = code;
        targets
    }

    /// Assign a product to a library and make the prefix able to see it.
    ///
    /// Writing the assignment is the cheap half. The other half is that the
    /// library has to be set up, its folder and marker written, and linked
    /// into the prefix under its own drive letters, or the download would land
    /// somewhere Wine cannot address and the in-game shop would have no drive
    /// root to start on.
    fn target_install(&mut self, code: &ProductCode, id: &str) -> Result<(), String> {
        let Some(library) = self.config.libraries.iter_mut().find(|l| l.id == id) else {
            return Err("that drive is no longer configured".to_owned());
        };
        if library.state() == library::State::Missing {
            return Err(format!("{} is not mounted", library.label));
        }
        library.claim().map_err(|e| chain(&e))?;
        if let Some(product) = self.config.product_mut(code) {
            product.profile.library = Some(id.to_owned());
        }
        self.save_config();
        self.rebuild_layout();

        // Before setup there is no prefix to link into, and setup makes the
        // links itself; creating `drive_c` early would get in wineboot's way.
        if lightforge_core::prefix::PrefixStatus::inspect(&self.layout).initialized {
            lightforge_core::prefix::link_libraries(&self.layout).map_err(|e| chain(&e))?;
        }
        Ok(())
    }

    /// Rebuild the layout from the config, after a library is added, removed,
    /// made main, or assigned to a product.
    /// Re-read what is provisioned, off the UI thread.
    fn setup_status(&self) -> Task<Message> {
        Task::perform(
            provision::status(self.layout.clone(), self.config.clone()),
            |status| Message::SetupStatus(status.map(Box::new)),
        )
    }

    /// Bring the header's list back in line with the configured products.
    ///
    /// Adoption can add a product nobody chose, a found install of Classic Era
    /// is a game the user has, whether or not they would have thought to add
    /// the tile, so the two lists are reconciled rather than appended to.
    fn sync_choices(&mut self) {
        self.choices = self
            .config
            .products
            .iter()
            .map(|p| Choice { code: p.code.clone(), label: p.display_name.clone() })
            .collect();
        // A game opened from Add a game is shown without being in the band;
        // it stays shown until something else is picked.
        let previewing = self.catalog.get(&self.selected.code).is_some();
        if !previewing && !self.choices.iter().any(|c| c.code == self.selected.code) {
            if let Some(first) = self.choices.first().cloned() {
                self.selected = first;
            }
        }
    }

    /// Put the game on screen into the band, if it was only being looked at.
    ///
    /// Called by every action that starts something for it: a download or a
    /// game running for a product the band does not show would have nowhere
    /// to report to, and nowhere to be found again.
    fn track_selected(&mut self) {
        if self.choices.iter().any(|c| c.code == self.selected.code) {
            return;
        }
        let choice = self.selected.clone();
        if self.config.product(&choice.code).is_none() {
            self.config.products.push(Product::new(choice.code.clone(), choice.label.clone()));
            self.save_config();
        }
        self.choices.push(choice);
    }

    fn rebuild_layout(&mut self) {
        if let Ok(layout) = self.config.layout() {
            self.layout = layout;
        }
    }

    /// Re-read the mount table: which partitions could be added, and whether a
    /// library that was absent has come back somewhere else.
    ///
    /// The relocation is the half worth having. A drive that was mounted at
    /// `/run/media/user/games` and comes back at `/run/media/user/games1` is
    /// still the same volume, and its marker says so, so the recorded path is
    /// corrected rather than the library being declared lost.
    fn refresh_drives(&mut self) {
        let mut moved = false;
        for library in &mut self.config.libraries {
            if library.state() != library::State::Missing {
                continue;
            }
            if let Some(found) = library::relocate(library) {
                library.games = found;
                library.probe();
                moved = true;
            }
        }
        if moved {
            self.save_config();
            self.rebuild_layout();
        }
        self.partitions = library::partitions(&self.config.libraries);
        self.check_drives();
    }

    /// Add the partition at `index` as a library.
    fn add_drive(&mut self, index: usize) -> Result<(), String> {
        let Some(partition) = self.partitions.get(index).cloned() else {
            return Err("that drive is no longer there".to_owned());
        };
        let (games_letter, wow_letter) = library::allocate_letters(&self.config.libraries)
            .ok_or_else(|| library::LibraryError::NoLettersLeft.to_string())?;
        let mut library = library::Library::on_partition(
            &partition.point,
            partition.label.clone(),
            partition.uuid.clone(),
        );
        library.games_letter = games_letter;
        library.wow_letter = wow_letter;
        library.claim().map_err(|e| chain(&e))?;
        if self.config.libraries.iter().any(|l| l.id == library.id) {
            return Err(format!("{} is already a Lightforge drive", partition.label));
        }
        self.config.libraries.push(library);
        self.save_config();
        self.rebuild_layout();
        if lightforge_core::prefix::PrefixStatus::inspect(&self.layout).initialized {
            lightforge_core::prefix::link_libraries(&self.layout).map_err(|e| chain(&e))?;
        }
        Ok(())
    }

    /// Take a drive off Lightforge's list. Nothing on it is deleted, the
    /// games are still there, and re-adding the drive picks them back up by
    /// its marker.
    fn remove_drive(&mut self, id: &str) -> Result<(), String> {
        let Some(library) = self.config.libraries.iter().find(|l| l.id == id).cloned() else {
            return Ok(());
        };
        if library.is_default && self.config.libraries.len() > 1 {
            return Err("make another drive the default one first".to_owned());
        }
        if self.config.libraries.len() == 1 {
            return Err("this is the only drive Lightforge has".to_owned());
        }
        if storage::holds_wow(&self.layout, &library) {
            return Err(format!(
                "{} holds a World of Warcraft install — uninstall it first",
                library.label
            ));
        }
        // The letters go with it, and the prefix keeps the links until the
        // next provision tidies them: a dangling `dosdevices` entry is inert,
        // and removing one while the client is up is not worth the race.
        self.config.libraries.retain(|l| l.id != id);
        for product in &mut self.config.products {
            if product.profile.library.as_deref() == Some(id) {
                product.profile.library = None;
            }
        }
        self.save_config();
        self.rebuild_layout();
        Ok(())
    }

    /// Opt one product in or out of one optional download.
    fn set_optional(&mut self, code: &ProductCode, name: &str, on: bool) {
        if let Some(product) = self.config.product_mut(code) {
            let list = &mut product.profile.optional_content;
            list.retain(|n| n != name);
            if on {
                list.push(name.to_owned());
                list.sort();
            }
        }
        self.save_config();
    }

    /// Work out each library's unused game data, off the UI thread. A
    /// library being freed right now is left to finish.
    fn check_unused(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for library in &self.config.libraries {
            let install = self.layout.wow_install_in(library);
            if matches!(self.unused.get(&library.id), Some(UnusedData::Freeing { .. })) {
                continue;
            }
            if !install.join(".build.info").is_file() {
                self.unused.remove(&library.id);
                continue;
            }
            self.unused.insert(library.id.clone(), UnusedData::Checking);
            let id = library.id.clone();
            tasks.push(Task::perform(tasks::unused_plan(self.http.clone(), install), move |result| {
                Message::UnusedPlanned(id.clone(), result)
            }));
        }
        Task::batch(tasks)
    }

    /// Whether a download, running or waiting, writes into `install`. What it
    /// has fetched so far belongs to no version yet, so freeing unused data
    /// would throw it away.
    fn downloading_into(&self, install: &Path) -> bool {
        self.queue.iter().any(|d| self.layout.wow_install(&d.code) == install)
    }

    /// Re-measure what Settings > Storage shows, off the UI thread.
    fn measure_storage(&mut self) -> Task<Message> {
        let Some(dirs) = self.dirs.clone() else {
            return Task::none();
        };
        self.storage_measuring = true;
        Task::perform(tasks::storage(self.layout.clone(), dirs), |report| {
            Message::StorageMeasured(Box::new(report))
        })
    }

    /// Point the library at the typed location.
    ///
    /// Only while nothing is installed in the current one: then it is a
    /// config change. With games or a prefix in it, it would be a data
    /// migration, re-pointing the prefix's game symlink, moving 100+ GB,
    /// and leaving the old library behind unannounced is not an answer.
    fn relocate(&mut self) -> Result<(), String> {
        let path = parse_path(&self.location_input)?;
        if path == self.layout.root() {
            return Ok(());
        }
        let downloading = self.queue.iter().any(|d| matches!(d.stage, Stage::Running { .. }));
        if self.setup_running || downloading {
            return Err("wait for setup and downloads to finish first".to_owned());
        }
        if !storage::library_is_empty(&self.layout) {
            return Err("the current library has games or a prefix in it; moving an \
                        installed library is not supported yet"
                .to_owned());
        }

        let default = Layout::default_for_user().ok();
        self.config.blizzard_root = match default.as_ref().map(Layout::root) {
            Some(root) if root == path => None,
            _ => Some(path.clone()),
        };
        // The first library moves with the root when it was under it, which
        // it is unless the user pointed `games_dir` somewhere else. Nothing is
        // installed (checked above), so this is a path change, not a move.
        let old = self.layout.games_root();
        if let Some(main) = self.config.libraries.iter_mut().find(|l| l.is_default) {
            if main.games == old {
                main.games = path.join("games");
                main.claimed = false;
                main.probe();
            }
        }
        self.save_config();
        self.rebuild_layout();
        self.check_drives();
        self.location_input = path.display().to_string();
        self.error = None;
        Ok(())
    }

    fn save_config(&mut self) {
        // Never over a file that did not load: it is the user's to fix.
        if self.config_error.is_some() {
            return;
        }
        let Some(dirs) = self.dirs.as_ref() else {
            self.error = Some("no config directory available".to_owned());
            return;
        };
        if let Err(e) = self.config.save(&dirs.config_file()) {
            self.error = Some(chain(&e));
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        // Only while something is moving: an idle dashboard should wake up for
        // nothing.
        let closing = iced::window::close_requests().map(Message::CloseRequested);

        // The hint chord, and the keys that move the selection ring. Only
        // keys no widget took, so arrows in a text field stay the field's,
        // and only these produce a message, so typing wakes nothing else.
        let hints = iced::keyboard::listen().filter_map(|event| {
            use iced::keyboard::key::Named;
            let iced::keyboard::Event::KeyPressed { ref key, modifiers, .. } = event else {
                return None;
            };
            if hints_chord(key, modifiers) {
                return Some(Message::ToggleHints);
            }
            let input = match key.as_ref() {
                iced::keyboard::Key::Named(Named::ArrowUp) => pad::Input::Step(focus::Step::Up),
                iced::keyboard::Key::Named(Named::ArrowDown) => pad::Input::Step(focus::Step::Down),
                iced::keyboard::Key::Named(Named::ArrowLeft) => pad::Input::Step(focus::Step::Left),
                iced::keyboard::Key::Named(Named::ArrowRight) => pad::Input::Step(focus::Step::Right),
                iced::keyboard::Key::Named(Named::Enter) => pad::Input::Press,
                iced::keyboard::Key::Named(Named::Escape) => pad::Input::Back,
                _ => return None,
            };
            Some(Message::Nav(NavFrom::Keys, input))
        });

        // Gamepads, for when Steam hands us a pad rather than keys. Read on a
        // thread of their own for the whole run; see [`pad`].
        let pads = Subscription::run(pad::inputs).map(|input| Message::Nav(NavFrom::Pad, input));
        let window_focus = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused(true)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocused(false)),
            _ => None,
        });

        // While the ring is up, the mouse moving takes it away, the way a
        // browser drops its focus outline once you reach for the mouse.
        let pointing = match self.focus.is_some() {
            true => iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { .. }) => {
                    Some(Message::FocusHidden)
                }
                _ => None,
            }),
            false => Subscription::none(),
        };

        // Resize events are only interesting to a human reading a trace, and
        // subscribing to them makes every resize step rebuild the view, so
        // this stays out of a normal run entirely.
        let resizes = match trace::frames() {
            true => iced::window::resize_events().map(|(_, size)| Message::ResizeTraced(size)),
            false => Subscription::none(),
        };

        // A held tile follows the cursor, and only a held one: hover alone
        // comes from the tiles' own mouse areas, so moving across the window
        // wakes nothing.
        let dragging = match self.tile_drag.is_some() {
            true => iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::TileDragged(position.x))
                }
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::TileDropped)
                }
                _ => None,
            }),
            false => Subscription::none(),
        };

        // The splash's bar sweeps off the clock, so it needs the tick as much
        // as a live transfer does.
        match self.splash.is_some()
            || self.running().is_some()
            || self.setup_running
            || self.addons_busy
            || self.scanning
            || self.uninstall_dialog.as_ref().is_some_and(|d| d.working)
            || self.unused.values().any(|u| matches!(u, UnusedData::Freeing { .. }))
        {
            true => Subscription::batch([
                closing,
                hints,
                pads,
                window_focus,
                resizes,
                dragging,
                pointing,
                iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            ]),
            false => Subscription::batch([closing, hints, pads, window_focus, resizes, dragging, pointing]),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        use std::sync::atomic::AtomicBool;
        static FIRST: AtomicBool = AtomicBool::new(false);
        static THEMED: AtomicBool = AtomicBool::new(false);
        trace::once(&FIRST, "first frame");
        if self.branding.is_some() {
            trace::once(&THEMED, "first frame with branding");
        }

        let started = Instant::now();
        focus::set_current(self.focus.clone());
        theme::set_console(self.console());
        if let Some(error) = &self.config_error {
            return self.broken_config_screen(error);
        }
        // Before the wizard: the window is still the small one, and the
        // wizard's card does not fit in it.
        if let Some(splash) = &self.splash {
            return self.splash_screen(splash);
        }
        // The first run is a layer over the app rather than a `Screen`: there
        // is no way out of it except the two buttons in its footer, so nothing
        // else may be reachable while it is up.
        if let Some(open) = &self.wizard {
            let card = wizard::view(self, open);
            // An open dropdown closes on a click anywhere else, as it does
            // on every other page.
            let card = match self.panel_open() {
                true => Element::from(mouse_area(card).on_press(Message::Dismiss)),
                false => card,
            };
            return focus::root(self.with_hints(
                container(card).width(Length::Fill).height(Length::Fill).style(theme::backdrop).into(),
            ));
        }
        let screen = match self.screen {
            Screen::Dashboard => self.dashboard(),
            Screen::News => self.news_screen(),
            Screen::Updates => self.updates_screen(),
            Screen::Settings => self.settings(),
            Screen::Addons => self.addons_screen(),
            Screen::Tweaks => self.tweaks_screen(),
        };

        // The manager hangs under its button on every screen, not just the
        // dashboard: a download keeps running while you are in Settings, and
        // that is exactly when you want to look at it. Its button is at the
        // end of the ribbon, so a stacked layer pinned to the top right lands
        // under it, padded clear of the ribbon it drops out of. (Add-a-game
        // used to share this corner; it now hangs off the `+` itself, see
        // [`Lightforge::strip`].)
        let overlay: Option<Element<Message>> =
            match self.manager_open && !self.queue.is_empty() {
                true => Some(self.manager()),
                false => None,
            };
        // Clicks inside a panel are absorbed so they never reach the dismissal
        // beneath it; without this, clicking the gap between two rows of the
        // download manager would close it.
        let overlay = overlay.map(|panel| focus::layer(mouse_area(panel).on_press(Message::MenuClick)));

        let screen = match overlay {
            Some(panel) => stack![
                screen,
                container(panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding([theme::XL + theme::LG, theme::LG]),
            ]
            .into(),
            None => screen,
        };


        // Modal: over everything, including the panels above. The scrim takes
        // every press that the card itself does not, and a `Stack` stops
        // delivering an event once a layer captures it, so nothing beneath
        // can be clicked while the dialog is up.
        let card = match (
            &self.install_dialog,
            &self.uninstall_dialog,
            &self.cancel_dialog,
            &self.downgrade_dialog,
            &self.version_picker,
            &self.addon_detail,
            &self.report_dialog,
            &self.game_settings,
            &self.patch_notes,
            &self.update_notice,
        ) {
            (Some(dialog), ..) => Some(self.install_card(dialog)),
            (_, Some(dialog), ..) => Some(self.uninstall_card(dialog)),
            (_, _, Some(dialog), ..) => Some(self.cancel_card(dialog)),
            (_, _, _, Some(dialog), ..) => Some(self.downgrade_card(dialog)),
            (_, _, _, _, Some(picker), ..) => Some(self.version_card(picker)),
            (_, _, _, _, _, Some(detail), ..) => Some(self.detail_card(detail)),
            (_, _, _, _, _, _, Some(dialog), ..) => Some(self.report_card(dialog)),
            (_, _, _, _, _, _, _, Some(settings), ..) => Some(self.game_settings_card(settings)),
            // Last, so a dialog someone opened themselves is never buried
            // under the one the app opened for them. The notes come before
            // the update notice: they are about the build that is running,
            // which is the one the user can do something with right now.
            (.., Some(notes), _) => Some(self.patch_notes_card(notes)),
            (.., Some(available)) => Some(self.update_card(available)),
            _ => None,
        };
        let screen = match card {
            Some(card) => stack![
                screen,
                focus::layer(
                    mouse_area(container(card).center(Length::Fill).style(theme::scrim))
                        .on_press(Message::MenuClick),
                ),
            ]
            .into(),
            None => screen,
        };

        // Anything open closes on a click that no widget wanted. `mouse_area`
        // hands the event to its child first and only fires when the child
        // ignored it, so a button still does its own job rather than being
        // swallowed by the dismissal.
        let screen = match self.panel_open() {
            true => Element::from(mouse_area(screen).on_press(Message::Dismiss)),
            false => screen,
        };

        // The lit corner goes behind everything, last, so every screen gets it
        // and nothing has to opt in. Where a page lays key art over the whole
        // window the art simply wins, which is the right order: the glow is
        // for the chrome, not for the picture.
        let screen = Element::from(
            container(screen)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::backdrop),
        );

        trace::view_built(started.elapsed());
        focus::root(self.with_hints(screen))
    }

    /// Title, product picker, and navigation.
    fn header(&self) -> Element<'_, Message> {
        let nav = |label: &'static str, screen: Screen| {
            let entry = button(text(label).size(theme::tap_text(TAB_LABEL)).font(theme::BOLD))
                .padding(theme::tap([theme::SM, theme::SM]));
            // Addons and Game Optimizations are a game's own pages, opened
            // from its panel, so Play stays lit on them, and stays clickable,
            // since it is the way back.
            let within = match screen {
                Screen::Dashboard => matches!(self.screen, Screen::Addons | Screen::Tweaks),
                _ => false,
            };
            match (self.screen == screen, within) {
                (true, _) => entry.style(theme::selected),
                (false, true) => entry.style(theme::selected).on_press(Message::Show(screen)),
                (false, false) => entry.style(theme::ghost).on_press(Message::Show(screen)),
            }
        };

        // A tab that leaves the app rather than changing the screen, so it
        // cannot go through `nav`: there is no `Screen` to compare against and
        // nothing behind it to light up. It wears the arrow for exactly that
        // reason. It is the only thing in this row that opens a browser, and
        // without the mark it reads as a page that never arrives.
        let outbound = |label: &'static str, url: &'static str| {
            button(
                row![
                    text(label).size(theme::tap_text(TAB_LABEL)).font(theme::BOLD),
                    image(outbound_icon())
                        .width(Length::Fixed(OUTBOUND_GLYPH))
                        .height(Length::Fixed(OUTBOUND_GLYPH)),
                ]
                .spacing(OUTBOUND_GAP)
                .align_y(Alignment::Center),
            )
            .padding(theme::tap([theme::SM, theme::SM]))
            .style(theme::ghost)
            .on_press(Message::OpenLink(url))
        };

        // The wordmark is the app menu's button, so it carries a chevron the
        // way every other menu in the app does.
        let wordmark = button(
            row![
                image(wordmark_image())
                    .width(Length::Fixed(WORDMARK_HEIGHT * WORDMARK_ASPECT))
                    .height(Length::Fixed(WORDMARK_HEIGHT)),
                image(chevron_icon())
                    .width(Length::Fixed(APP_MENU_CHEVRON))
                    .height(Length::Fixed(APP_MENU_CHEVRON)),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .padding(theme::tap([theme::XS, theme::SM]))
        .style(match self.app_menu_open {
            true => theme::selected,
            false => theme::ghost,
        })
        .on_press(Message::ToggleAppMenu);

        // Title, then the pages, then everything that is not a page pushed to
        // the far end. The band is no longer in this row at all, it has one
        // of its own below, so the tiles answer "which game" where the tabs
        // answer "which page", instead of the two sharing a line.
        let mut bar = row![popover::popover(
            self.ring(focus::Target::AppMenu, Some(Message::ToggleAppMenu), wordmark),
            // No `mouse_area` guard here: a popover captures the pointer over
            // its own bounds, which is what that guard is for on the stacked
            // panels; see [`popover`].
            self.app_menu_open.then(|| self.app_menu()),
        )
        .align_left()]
        .align_y(Alignment::Center)
        .spacing(theme::SM);

        // Back and forward, between the wordmark and the tabs as in any
        // browser. Both are drawn always and lit only when there is somewhere
        // to go: a control that appears and disappears moves the tabs beside
        // it, and arrows that vanish are harder to find than arrows that are
        // simply dim.
        let step = |icon: image::Handle, message: Message, enabled: bool| {
            button(
                image(icon)
                    .width(Length::Fixed(BAND_GLYPH * 0.85))
                    .height(Length::Fixed(BAND_GLYPH * 0.85))
                    .opacity(match enabled {
                        true => 1.0_f32,
                        false => DIMMED,
                    }),
            )
            .padding(theme::tap([theme::XS, theme::SM]))
            .style(theme::ghost)
            .on_press_maybe(enabled.then_some(message))
        };

        bar = bar.push(Space::new().width(Length::Fixed(GROUP_GAP)));
        bar = bar.push(self.ring(
            focus::Target::Back,
            (!self.visited.is_empty()).then_some(Message::GoBack),
            step(back_icon(), Message::GoBack, !self.visited.is_empty()),
        ));
        bar = bar.push(self.ring(
            focus::Target::Forward,
            (!self.forward.is_empty()).then_some(Message::GoForward),
            step(forward_icon(), Message::GoForward, !self.forward.is_empty()),
        ));

        // The pages in a row of their own rather than loose in `bar`, so
        // [`TAB_GAP`] sets the air between tabs and nothing else: the row above
        // separates groups, wordmark, arrows, pages, and those two distances
        // are not the same distance.
        bar = bar.push(Space::new().width(Length::Fixed(GROUP_GAP)));
        bar = bar.push(
            row![
                self.ring(
                    focus::Target::Tab(Screen::Dashboard),
                    Some(Message::Show(Screen::Dashboard)),
                    nav("Play", Screen::Dashboard),
                ),
                self.ring(
                    focus::Target::Tab(Screen::News),
                    Some(Message::Show(Screen::News)),
                    nav("News", Screen::News),
                ),
                self.ring(
                    focus::Target::Shop,
                    Some(Message::OpenLink(SHOP_URL)),
                    outbound("Battle.net Shop", SHOP_URL),
                ),
            ]
            .spacing(TAB_GAP)
            .align_y(Alignment::Center),
        );
        bar = bar.push(Space::new().width(Length::Fill));

        // Only while something is in flight. An idle launcher has no
        // downloads to manage, and a permanent button that opens an empty
        // panel is furniture.
        if !self.queue.is_empty() {
            let count = self.queue.len();
            bar = bar.push(self.ring(
                focus::Target::Downloads,
                Some(Message::ToggleManager),
                // A drawn mark, like the back and forward arrows beside it; a
                // typed arrow sat on the text baseline at the font's weight
                // and read as a stray character.
                button(
                    row![
                        image(download_icon())
                            .width(Length::Fixed(BAND_GLYPH * 0.85))
                            .height(Length::Fixed(BAND_GLYPH * 0.85)),
                        text(count.to_string()).size(TAB_LABEL),
                    ]
                    .spacing(theme::XS)
                    .align_y(Alignment::Center),
                )
                .padding(theme::tap([theme::XS, theme::SM]))
                .style(match self.manager_open {
                    true => theme::selected,
                    false => theme::ghost,
                })
                .on_press(Message::ToggleManager),
            ));
            bar = bar.push(Space::new().width(Length::Fixed(theme::SM)));
        }

        // No backdrop behind the tabs. The wordmark, the arrows and the pages
        // are chrome, and chrome over artwork needs no bar of its own to be
        // legible, what made the old ribbon necessary was the band sharing
        // its line, and the band has moved out.
        let mut rows =
            column![container(bar).width(Length::Fill).padding([theme::SM, theme::LG])];

        // The band as a card of its own, inset from the window and detached
        // from the row above, the same `floating` treatment as the action
        // panel at the foot of the dashboard, so the two read as the same kind
        // of object: a surface laid over the art rather than a strip welded to
        // the window's edge. `responsive` hands `strip` the room it actually
        // has, which is what decides how many tiles fit before the overflow
        // takes the rest.
        if self.shows_band() {
            rows = rows.push(
                container(
                    container(
                        row![
                            // Labelled, the way Battle.net labels its own
                            // band: the tiles are marks without words, and one
                            // quiet word says what the row is before you know
                            // them.
                            // Centred on the letters, not on the line box;
                            // see [`optically_centred`].
                            container(optically_centred(
                                muted("GAMES".to_owned(), theme::LABEL).font(theme::BOLD),
                            ))
                            .height(Length::Fill)
                            .align_y(Alignment::Center),
                            responsive(|space| self.strip(space.width)),
                        ]
                        // Only where there is a client to open. Declining
                        // Battle.net during setup is a supported choice, and a
                        // button that opens something that is not installed is
                        // worse than no button.
                        .extend((!self.config.battle_net_declined).then(|| {
                            Element::from(
                                container(self.ring(
                                    focus::Target::BattleNet,
                                    (!self.busy).then_some(Message::OpenBattleNet),
                                    self.battle_net_button(),
                                ))
                                    .height(Length::Fill)
                                    .align_y(Alignment::Center),
                            )
                        }))
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                    )
                    // Centred, not merely given the height: a container aligns
                    // its content to the top by default, so the row's slack
                    // all fell below the tiles and they sat against the card's
                    // upper edge. The row's own `align_y` cannot fix that, it
                    // centres the tiles within the row, which is exactly as
                    // tall as they are.
                    .align_y(Alignment::Center)
                    .height(Length::Fixed(strip_height()))
                    .padding([0.0, theme::MD])
                    .style(theme::floating),
                )
                .width(Length::Fill)
                .padding([0.0, theme::LG]),
            );
        }

        // The hairline only where the band card is not: the card is the
        // separation where it appears, and on a page without one a header with
        // no backdrop would otherwise run straight into the content.
        match self.shows_band() {
            true => rows.push(Space::new().height(Length::Fixed(theme::SM))).into(),
            false => rows
                .push(
                    container(Space::new().height(Length::Fixed(1.0)))
                        .width(Length::Fill)
                        .style(theme::ribbon_edge),
                )
                .into(),
        }
    }

    /// The configured lines, in the order their first product appears.
    ///
    /// A line, not a product: PTR and beta are builds of a game, not games
    /// beside it, so five World of Warcrafts collapse to three tiles and a
    /// version picker.
    fn lines(&self) -> Vec<ProductLine> {
        let mut lines: Vec<ProductLine> = Vec::new();
        for choice in &self.choices {
            let line = self.catalog.line_of(&choice.code);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        lines
    }

    /// Move the line at `from` in the band to `to`, its products with it.
    ///
    /// The band's order is the configured products' order, so that is what
    /// changes; the choices follow so the band redraws without a reload. A
    /// stable sort keeps a line's PTR and beta where they were behind it.
    fn move_line(&mut self, from: usize, to: usize) {
        let mut order = self.lines();
        let line = order.remove(from);
        order.insert(to, line);
        let catalog = &self.catalog;
        let rank = |code: &ProductCode| {
            let line = catalog.line_of(code);
            order.iter().position(|l| l == &line).unwrap_or(usize::MAX)
        };
        self.config.products.sort_by_key(|p| rank(&p.code));
        self.choices.sort_by_key(|c| rank(&c.code));
    }

    /// Rebuild the catalog from whatever manifest is in hand.
    fn refresh_catalog(&mut self) {
        let mut catalog = match &self.manifest {
            Some(manifest) => manifest.catalog(),
            None => Catalog::builtin(),
        };
        catalog.reconcile(&self.known_codes);
        self.catalog = catalog;
    }

    /// The catalog: everything that could be added to the header.
    ///
    /// Two halves, because they behave differently and saying so once here
    /// beats a surprise at the button. The WoW family Lightforge downloads;
    /// everything else is Battle.net's to install and start.
    fn browse(&self) -> Element<'_, Message> {
        use lightforge_core::product::CatalogEntry;

        let tracked = |code: &ProductCode| self.choices.iter().any(|c| &c.code == code);
        let mut rows = column![text("All Games").size(theme::HEADING)]
            .spacing(theme::SM)
            .width(Length::Fixed(400.0));

        let row_for = |entry: &CatalogEntry| -> Element<Message> {
            let mut facts = column![text(entry.label.clone()).size(theme::BODY)];
            // Whether it is installed, not which build: the version belongs to
            // the panel you launch from, and this list is for choosing which
            // games to keep in the band.
            if self.installed_versions.contains_key(entry.code.as_str()) {
                facts = facts.push(muted("installed".to_owned(), theme::CAPTION));
            }
            // Remove takes the whole line, so it is held only when nothing
            // outside that line is left to fall back to.
            let elsewhere = |code: &ProductCode| {
                let line = self.catalog.line_of(code);
                self.choices.iter().any(|c| self.catalog.line_of(&c.code) != line)
            };
            let action: Element<Message> = match tracked(&entry.code) {
                true => secondary("Remove", Message::RemoveProduct(entry.code.clone()))
                    .keyed(format!("game {}", entry.code.as_str()), elsewhere(&entry.code)),
                false => secondary("Add", Message::AddProduct(entry.code.clone()))
                    .keyed(format!("game {}", entry.code.as_str()), true),
            };
            // The mark the band would wear, so a row and its tile are plainly
            // the same game. The column is held even when there is no icon,
            // hand-offs have none, and art arrives one product at a time, so
            // the labels stay in line instead of stepping in and out.
            let mark: Element<Message> = match self.icons.get(&entry.code) {
                Some(mark) => image(mark.handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fixed(BROWSE_MARK))
                    .height(Length::Fixed(BROWSE_MARK))
                    .into(),
                None => Space::new().width(Length::Fixed(BROWSE_MARK)).into(),
            };
            // The whole row opens the game's page, tracked or not; Add and
            // Remove inside it take their own clicks first and stay the only
            // things that change the band.
            let open = Message::OpenProduct(entry.code.clone());
            tappable(
                format!("open {}", entry.code.as_str()),
                Some(open.clone()),
                button(
                    row![mark, facts.width(Length::Fill), action]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(theme::SM)
                .style(theme::row_button)
                .on_press(open),
            )
        };

        // Only line heads: a line's other branches are reached from the
        // version picker, not added here as games of their own.
        // Tracked lines stay listed, or there would be no way to remove one.
        // By head, not by `Branch::Live`: Forever's head is its beta.
        let managed: Vec<&CatalogEntry> =
            self.catalog.managed().filter(|e| e.code == e.line.head()).collect();
        if !managed.is_empty() {
            rows = rows.push(muted("World of Warcraft".to_owned(), theme::LABEL));
            rows = rows.extend(self.hint("Downloaded and launched by Lightforge.").map(Element::from));
            for entry in managed {
                rows = rows.push(row_for(entry));
            }
        }

        rows = rows.push(muted("Other Blizzard games".to_owned(), theme::LABEL));
        match self.config.battle_net_declined {
            // Nothing to hand them to. Listed anyway would be a row whose Add
            // button adds a tile that cannot do anything.
            true => {
                rows = rows.push(muted(
                    "Battle.net is not installed — add it from Settings › Setup.".to_owned(),
                    theme::CAPTION,
                ));
            }
            false => {
                rows = rows.extend(self.hint("Installed and started through Battle.net. Lightforge only opens them.").map(Element::from));
                for entry in self.catalog.hand_offs() {
                    rows = rows.push(row_for(entry));
                }
            }
        }

        // Embedded rather than floating: the default scrollbar is drawn over
        // the content, and the rows end in a button, so it landed on top of
        // Add and Remove. Given its own column it sits beside them instead.
        let panel = container(
            focus::region(
                "browse",
                scrollable(rows)
                    .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                    .style(theme::scrollbar),
            ),
        )
        .padding(theme::MD)
        .max_height(560.0)
        .style(theme::floating);
        focus::layer(panel)
    }

    /// The download manager: every transfer this session knows about.
    ///
    /// One row each, with the controls that row can actually offer, a queued
    /// transfer has no progress to show and a running one cannot be resumed.
    fn manager(&self) -> Element<'_, Message> {
        let mut rows = column![
            row![
                text("Downloads").size(theme::HEADING),
                Space::new().width(Length::Fill),
                muted(
                    match self.queue.len() {
                        1 => "1 in queue".to_owned(),
                        n => format!("{n} in queue"),
                    },
                    theme::CAPTION,
                ),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(380.0));

        for entry in &self.queue {
            let mut item = column![text(entry.label.clone()).size(theme::BODY)].spacing(theme::XS);

            match &entry.stage {
                Stage::Queued => {
                    item = item.push(muted("waiting".to_owned(), theme::CAPTION));
                }
                Stage::Running { .. } => {
                    let progress = entry.progress();
                    item = item.push(progress_bar(0.0..=1.0, progress.fraction()).girth(5));
                    item = item.push(
                        row![
                            mono(bytes_of(progress.downloaded_bytes, progress.total_bytes)),
                            dot(),
                            mono(rate_of(progress.bytes_per_second)),
                            dot(),
                            mono(eta_of(progress)),
                        ]
                        .spacing(theme::XS)
                        .align_y(Alignment::Center),
                    );
                }
                Stage::Paused(progress) => {
                    item = item.push(progress_bar(0.0..=1.0, progress.fraction()).girth(5));
                    item = item.push(muted("paused".to_owned(), theme::CAPTION));
                }
            }

            let control = match &entry.stage {
                Stage::Running { .. } => secondary("Pause", Message::PauseDownload(entry.code.clone())),
                Stage::Queued | Stage::Paused(_) => {
                    secondary("Resume", Message::ResumeDownload(entry.code.clone()))
                }
            }
            .enabled(true);

            rows = rows.push(
                container(
                    row![
                        item.width(Length::Fill),
                        column![
                            control,
                            secondary("Remove", Message::CancelDownload(entry.code.clone()))
                                .enabled(true),
                        ]
                        .spacing(theme::XS),
                    ]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
                )
                .padding(theme::SM)
                .style(theme::row),
            );
        }

        container(rows)
            .padding(theme::MD)
            .style(theme::floating)
            .into()
    }

    /// Explanatory prose under a control, drawn only when the hints are on.
    ///
    /// `None` rather than an empty widget so a column simply does not lay the
    /// line out: an invisible child still costs its spacing, which would leave
    /// the controls unevenly spread once the prose is gone.
    fn hint<'a>(&self, content: impl Into<String>) -> Option<iced::widget::Text<'a>> {
        self.hints.then(|| muted(content.into(), theme::CAPTION))
    }

    /// What to call a line on screen.
    ///
    /// The name the user gave it first, then the catalog's, and only then the
    /// line's own, which for a line Lightforge does not model is the bare
    /// product code, and `hs_beta` is not a thing to label a tile with.
    fn name_of(&self, line: &ProductLine) -> String {
        let head = line.head();
        self.choices
            .iter()
            .find(|c| c.code == head)
            .map(|c| c.label.clone())
            .or_else(|| self.catalog.get(&head).map(|e| e.label.clone()))
            .unwrap_or_else(|| line.label().to_owned())
    }

    /// The art a line's tile wears, if any.
    ///
    /// The line's mark is its live build's, since that is the product the art
    /// was published against; a PTR is the same game wearing the same face.
    ///
    /// Except for a hand-off: the manifest's `default` answers for every code
    /// it does not name, so asking would put Retail's mark on Hearthstone.
    /// The mark a line's tile wears, if the manifest gives it one.
    ///
    /// Hand-offs were excluded here when nothing in the manifest described
    /// them, which left Diablo and StarCraft as text tiles beside WoW's
    /// icons. They have marks now, and a game is a game: what decides whether
    /// a tile shows art is whether art exists, not who installs the game.
    ///
    /// The head's mark first, then any other branch's: Forever's head is
    /// `wowf`, whose art stays on `wow_classic_beta` in the manifest (see
    /// `Manifest::head_theme`).
    fn tile_art(&self, line: &ProductLine) -> Option<&tasks::Mark> {
        line.branches().iter().find_map(|code| self.icons.get(code))
    }

    /// One tile.
    ///
    /// A line with no icon in the manifest falls back to its label, so the
    /// band is complete and usable before any art exists and fills in one line
    /// at a time as art is published.
    fn tile(&self, line: &ProductLine) -> Element<'_, Message> {
        let selected = line == &self.catalog.line_of(&self.selected.code);
        let face: Element<Message> = match self.tile_art(line) {
            Some(mark) => image(mark.handle.clone())
                .content_fit(ContentFit::Contain)
                .width(Length::Fixed(theme::tile()))
                .height(Length::Fixed(theme::tile()))
                .into(),
            None => container(text(self.name_of(line)).size(theme::LABEL))
                .center_x(Length::Shrink)
                .center_y(Length::Fixed(theme::tile()))
                .into(),
        };

        // The button only for its look. A pressable one keeps its pressed
        // state by position in the row, and a drag moves tiles between
        // positions, so a tile let go after a drag could fire a different
        // tile's press. The mouse area around it takes press, hover, and
        // release instead, and [`Message::TileDropped`] tells a click from a
        // drag. With no `on_press` the button always reports `Disabled`, so
        // the hover it would have shown is passed in from what is tracked.
        let hovered = self.tile_hover.as_ref() == Some(line);
        let held = self.tile_drag.as_ref().is_some_and(|drag| &drag.line == line);
        let status = match hovered || held {
            true => button::Status::Hovered,
            false => button::Status::Active,
        };
        let tile = button(face).padding([theme::XS, theme::SM]);
        let tile = match selected {
            true => tile.style(theme::selected),
            false => tile.style(move |theme, _| theme::ghost(theme, status)),
        };
        let dragging = self.tile_drag.as_ref().is_some_and(|drag| drag.moving);
        let tile = self.ring(
            focus::Target::Tile(line.clone()),
            Some(Message::LineSelected(line.clone())),
            tile,
        );
        let tile = mouse_area(tile)
            .on_press(Message::TileGrabbed(line.clone()))
            .on_right_press(Message::TileMenu(line.clone()))
            .on_enter(Message::TileEntered(line.clone()))
            .on_exit(Message::TileLeft(line.clone()))
            .interaction(match dragging {
                true => iced::mouse::Interaction::Grabbing,
                false => iced::mouse::Interaction::Pointer,
            });

        // The menu hangs under the tile, where the tooltip would, so the
        // tooltip gives way while it is open rather than being drawn over it.
        if self.tile_menu.as_ref() == Some(line) {
            return popover::popover(tile, Some(self.tile_menu_panel(line))).align_left().below().into();
        }
        tooltip(
            tile,
            container(text(self.name_of(line)).size(theme::LABEL))
                .padding([theme::XS, theme::SM])
                .style(theme::floating),
            tooltip::Position::Bottom,
        )
        .into()
    }

    /// A tile's right-click menu.
    ///
    /// Remove takes the tile out of the band and nothing else: the game stays
    /// installed and can be added back from Add a game. Held when it is the
    /// last tile, for the reason [`Message::RemoveProduct`] refuses it.
    fn tile_menu_panel(&self, line: &ProductLine) -> Element<'_, Message> {
        let elsewhere = self.lines().iter().any(|other| other != line);
        let face = row![
            image(close_icon())
                .width(Length::Fixed(MENU_GLYPH))
                .height(Length::Fixed(MENU_GLYPH))
                .opacity(if elsewhere { 1.0_f32 } else { 0.4_f32 }),
            text("Remove from list").size(theme::BODY).color_maybe((!elsewhere).then_some(theme::MUTED)),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
        let entry = button(face.width(Length::Fill))
            .padding(theme::tap([theme::XS, theme::SM]))
            .style(theme::ghost)
            .on_press_maybe(elsewhere.then(|| Message::RemoveProduct(line.head())));
        container(column![entry].width(Length::Fixed(200.0)))
            .padding(theme::SM)
            .style(theme::menu)
            .into()
    }

    /// How wide a tile will be.
    ///
    /// Exact for art: a fixed square in fixed padding. A line falling back to
    /// its name cannot be measured without a renderer, so its width is
    /// estimated from the glyph count at a width no glyph of this size
    /// exceeds. Erring wide costs a little unused band; erring narrow would
    /// cut a tile in half against the navigation, so the estimate leans wide
    /// on purpose.
    fn tile_width(&self, line: &ProductLine) -> f32 {
        let padding = theme::SM * 2.0;
        match self.tile_art(line) {
            Some(_) => theme::tile() + padding,
            None => {
                let glyphs = self.name_of(line).chars().count() as f32;
                glyphs * theme::LABEL * 0.62 + padding
            }
        }
    }

    /// The game band: the tiles that fit, then Add a game, and whatever did
    /// not fit behind an overflow button.
    ///
    /// `room` is the width the header has left over once the title, the
    /// navigation and the buttons beside it have taken theirs, so the band
    /// never has to know the window's size; see [`Self::header`].
    fn strip(&self, room: f32) -> Element<'_, Message> {
        let lines = self.lines();
        // The tile the ring is on stays in view in place of the selected one,
        // or the arrows would walk into the overflow and out of sight.
        let current = match &self.focus {
            Some(focus::Target::Tile(line)) => line.clone(),
            _ => self.catalog.line_of(&self.selected.code),
        };
        let selected = lines.iter().position(|line| line == &current).unwrap_or(0);
        let widths: Vec<f32> = lines.iter().map(|line| self.tile_width(line)).collect();

        // Add a game sits at the end of the band whatever happens, so its
        // width is not the band's to spend.
        let (shown, hidden) = divide_strip(&widths, selected, room - BAND_BUTTON);

        // Fills the height it is given, rather than shrinking to the tiles.
        // `responsive` lays its content out at the top of the box, so a band
        // that is only as tall as its tiles leaves all of the card's slack
        // underneath them and they ride high in it. Filling first, then
        // centring, puts the slack on both sides.
        let mut band =
            row![].height(Length::Fill).align_y(Alignment::Center).spacing(theme::SM);
        for id in shown {
            band = band.push(self.tile(&lines[id]));
        }

        if !hidden.is_empty() {
            let hidden: Vec<ProductLine> = hidden.into_iter().map(|id| lines[id].clone()).collect();
            let more = self.ring(
                focus::Target::More,
                Some(Message::ToggleStrip),
                band_button(
                    image(chevron_icon())
                        .width(Length::Fixed(BAND_GLYPH))
                        .height(Length::Fixed(BAND_GLYPH)),
                    Message::ToggleStrip,
                    self.strip_open,
                    format!("{} more game{}", hidden.len(), match hidden.len() {
                        1 => "",
                        _ => "s",
                    }),
                ),
            );
            // The panel hangs under the button it belongs to rather than in
            // the corner like the download manager: which games are behind it
            // is the button's own answer, and the corner is a long way from
            // the band on a wide window.
            band = band.push(popover::popover(
                more,
                self.strip_open.then(|| self.overflow_menu(&hidden)),
            ));
        }

        // Under the `+` itself, not in the corner: the button moves along the
        // band as games are added and removed, and the panel is the button's
        // own answer. Pinned by its left edge, since the `+` has the rest of
        // the ribbon to its right to grow into.
        band = band.push(
            popover::popover(
                self.ring(
                    focus::Target::AddGame,
                    Some(Message::ToggleBrowse),
                    band_button(
                        image(plus_icon())
                            .width(Length::Fixed(BAND_GLYPH))
                            .height(Length::Fixed(BAND_GLYPH)),
                        Message::ToggleBrowse,
                        self.browse_open,
                        "All Games".to_owned(),
                    ),
                ),
                // The popover absorbs clicks inside the panel itself, so no
                // `mouse_area` guard is needed here; see [`popover`].
                self.browse_open.then(|| self.browse()),
            )
            .align_left(),
        );
        band.into()
    }

    /// Go to a page, remembering where you were.
    ///
    /// Asking for the page you are on is not a history step, or back would
    /// walk through the same page once per click of its tab.
    fn go_to(&mut self, screen: Screen) {
        if screen != self.screen {
            self.visited.push(self.screen);
            self.forward.clear();
            self.screen = screen;
        }
    }

    /// Work a page needs on the way in.
    ///
    /// Called from all three ways of getting somewhere, a tab, back, and
    /// forward, because a page reached by the back button is just as opened
    /// as one reached by clicking it. Doing this in `go_to` instead is not
    /// possible: that returns nothing, and this has to hand a `Task` back to
    /// the runtime.
    fn arrived(&mut self) -> Task<Message> {
        match self.screen {
            Screen::News => {
                // Decoded on first arrival rather than at startup: half a
                // megabyte of JPEG is not work the first frame should do for
                // a page nobody has opened.
                let backdrop = match self.news_bg.is_none() {
                    true => Task::perform(
                        tasks::news_background(NEWS_BACKGROUND),
                        Message::NewsBackground,
                    ),
                    false => Task::none(),
                };
                Task::batch([backdrop, self.fetch_news(false)])
            }
            // The dashboard needs them too now that the rail is on it. The
            // same once-per-session fetch, `fetch_news` is idempotent, so
            // arriving here first and the News page second costs one request
            // between them, not two. With the rail switched off there is
            // nothing here to fill, and the News page asks for itself.
            Screen::Dashboard if self.config.news_on_play => self.fetch_news(false),
            // Arriving back on Browse with nothing in it, by the back
            // button, or on a first visit, is the same question the tab
            // itself answers: what is there?
            Screen::Addons
                if self.addon_tab == AddonTab::Browse
                    && self.search_input.trim().is_empty()
                    && self.search.listings.is_empty() =>
            {
                self.popular_if_unasked()
            }
            _ => Task::none(),
        }
    }

    /// Ask for the headlines.
    ///
    /// `force` is the Refresh button. Without it this is a once-per-session
    /// fetch: revalidation is the proxy's job and its answer is cached on
    /// disk, so re-asking on every visit to the tab would buy nothing but
    /// requests.
    fn fetch_news(&mut self, force: bool) -> Task<Message> {
        if self.news_pending || (!force && self.news_asked) {
            return Task::none();
        }
        let Some(dirs) = self.dirs.clone() else {
            return Task::none();
        };
        self.news_pending = true;
        Task::perform(
            tasks::headlines(
                self.http.clone(),
                self.config.news_origin(),
                dirs.news_cache_file(),
            ),
            |headlines| Message::NewsLoaded(headlines.map(Box::new)),
        )
    }

    /// Ask for the pictures of whatever headlines are now on the page.
    fn fetch_news_images(&self) -> Task<Message> {
        let (Some(dirs), Some(headlines)) = (self.dirs.clone(), self.news.as_ref()) else {
            return Task::none();
        };
        let urls: Vec<String> = headlines
            .feed
            .items
            .iter()
            .filter_map(|item| item.image.clone())
            .filter(|url| !url.is_empty())
            .collect();
        if urls.is_empty() {
            return Task::none();
        }

        let have = self.news_images.keys().cloned().collect();
        Task::perform(
            tasks::news_images(self.http.clone(), dirs.news_image_dir(), urls, have),
            Message::NewsImages,
        )
    }

    /// Which screens wear the game band under the tabs.
    ///
    /// The pages you move between freely, the dashboard and News, keep the
    /// same chrome, so switching tabs changes the content under a header that
    /// does not move. Settings and a game's own pages (Addons, Game
    /// Optimizations) are somewhere you went *into*: they are about one thing,
    /// and a band offering to switch games above them invites a click that
    /// would throw away what you are doing.
    fn shows_band(&self) -> bool {
        matches!(self.screen, Screen::Dashboard | Screen::News)
    }

    /// The hand-off into Battle.net, at the right-hand end of the game card.
    ///
    /// App-level, not per-product: everything outside the WoW family is a
    /// hand-off, so it belongs with the games rather than on a panel that
    /// describes one of them, and the card is where games live now.
    ///
    /// The mark comes out of the prefix rather than the binary: see
    /// [`Layout::battle_net_icon`]. Before Battle.net is installed there is
    /// none, and the button is the word alone.
    fn battle_net_button(&self) -> Element<'_, Message> {
        let mut face = row![].spacing(theme::SM).align_y(Alignment::Center);
        if let Some(icon) = &self.bnet_icon {
            face = face.push(
                image(icon.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fixed(BNET_GLYPH))
                    .height(Length::Fixed(BNET_GLYPH)),
            );
        }
        // Same optical correction as the band's label, the word has no
        // descenders either, and it sits beside a mark that fills its box.
        let face = face.push(optically_centred(
            text("Battle.net").size(theme::LABEL).font(theme::BOLD),
        ));

        tooltip(
            button(face)
                .padding([theme::SM, theme::MD])
                .style(theme::ghost)
                .on_press_maybe((!self.busy).then_some(Message::OpenBattleNet)),
            container(
                text("Open Battle.net for other Blizzard titles".to_owned()).size(theme::LABEL),
            )
            .padding([theme::XS, theme::SM])
            .style(theme::floating),
            tooltip::Position::Bottom,
        )
        .into()
    }

    /// Whether any dismissable panel is showing.
    ///
    /// One list, used both to arm the click-anywhere dismissal and to clear
    /// everything in [`Message::Dismiss`]. Adding a panel and forgetting to
    /// name it here is what left the wordmark menu unable to be closed by
    /// clicking the page, so the two now read from the same place.
    /// Whether this provider/id pair is in the ledger for the open game.
    ///
    /// Asked by the detail card twice over, once to pick the opening tab,
    /// once to decide between Install and nothing, so it is one place.
    fn addon_is_installed(&self, provider: &str, id: &str) -> bool {
        self.addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .any(|m| m.provider == provider && m.id == id)
    }

    /// Whether the arrows, Enter and Escape are someone else's right now.
    ///
    /// Whether the controller and touch look is on: the answer setup asks
    /// for on SteamOS, or the switch in Settings › General anywhere.
    fn console(&self) -> bool {
        self.config.steamos_use == Some(SteamOsUse::Console)
    }

    /// Put this program in the Steam library of the account that signed in
    /// last, with its artwork; the outcome goes under the Settings button.
    ///
    /// Written straight away whether or not Steam is running: Steam reads
    /// its shortcuts when it starts, so the entry is there next time.
    fn add_to_steam(&mut self) {
        use lightforge_core::steam_shortcut;

        let outcome = (|| {
            let steam = lightforge_core::steam::SteamRoot::discover()
                .ok_or_else(|| chain(&steam_shortcut::ShortcutError::NoSteam))?;
            let exe = std::env::current_exe().map_err(|e| format!("where Lightforge is: {e}"))?;
            steam_shortcut::add(&steam, "Lightforge", &exe, &STEAM_ART).map_err(|e| chain(&e))
        })();
        self.steam_shortcut = Some(outcome.map(|added| match added.updated {
            true => "Lightforge's entry in Steam is up to date. Steam shows changes the next \
                     time it starts."
                .to_owned(),
            false => "Lightforge is in your Steam library from the next time Steam starts."
                .to_owned(),
        }));
    }

    /// Fullscreen on a console, a window anywhere else.
    ///
    /// Game Mode shows one window at a time anyway, and a window the
    /// launcher's own 1280x820 on a 1280x800 screen was shrunk to fit with a
    /// bar down either side. The minimum size goes with it, or the window
    /// could not be made smaller than the screen it fills.
    fn window_mode(&self) -> Task<Message> {
        let console = self.console();
        iced::window::oldest().and_then(move |id| match console {
            true => iced::window::set_min_size(id, None)
                .chain(iced::window::set_mode(id, iced::window::Mode::Fullscreen)),
            false => iced::window::set_mode(id, iced::window::Mode::Windowed)
                .chain(iced::window::set_min_size(id, Some(WINDOW_MIN))),
        })
    }

    /// Along the foot of the window on a console: what the two buttons do,
    /// the way Steam's own screens say it.
    fn button_hints(&self) -> Element<'_, Message> {
        let glyph = |letter: &'static str| {
            container(text(letter).size(theme::LABEL).font(theme::BOLD).color(theme::BG))
                .center_x(Length::Fixed(HINT_GLYPH))
                .center_y(Length::Fixed(HINT_GLYPH))
                .style(|_| container::Style {
                    background: Some(theme::TEXT.into()),
                    border: iced::Border { radius: (HINT_GLYPH / 2.0).into(), ..Default::default() },
                    ..container::Style::default()
                })
        };
        let hint = |letter, label: &'static str| {
            row![glyph(letter), text(label).size(theme::BODY)]
                .spacing(theme::SM)
                .align_y(Alignment::Center)
        };
        container(
            row![Space::new().width(Length::Fill), hint("A", "Select"), hint("B", "Back")]
                .spacing(theme::LG)
                .align_y(Alignment::Center),
        )
        .padding([theme::SM, theme::LG])
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(theme::SURFACE.into()),
            ..container::Style::default()
        })
        .into()
    }

    /// `screen` with the button hints under it, on a console.
    fn with_hints<'a>(&'a self, screen: Element<'a, Message>) -> Element<'a, Message> {
        match self.console() {
            true => column![container(screen).height(Length::Fill), self.button_hints()].into(),
            false => screen,
        }
    }

    /// The splash and the broken-config screen have nothing to press.
    fn keys_belong_elsewhere(&self) -> bool {
        self.config_error.is_some() || self.splash.is_some()
    }

    /// What closes the dialog on top, the way its Cancel or Close does.
    ///
    /// In the order the view stacks them, so Escape closes the one on top.
    fn dialog_dismissal(&self) -> Option<Message> {
        Some(match () {
            _ if self.install_dialog.is_some() => Message::InstallCancel,
            _ if self.uninstall_dialog.is_some() => Message::UninstallCancel,
            _ if self.cancel_dialog.is_some() => Message::CancelDownloadDismiss,
            _ if self.downgrade_dialog.is_some() => Message::DowngradeCancel,
            _ if self.version_picker.is_some() => Message::CancelVersionPicker,
            _ if self.addon_detail.is_some() => Message::CloseAddonDetail,
            _ if self.report_dialog.is_some() => Message::ReportCancel,
            _ if self.game_settings.is_some() => Message::CloseGameSettings,
            _ if self.patch_notes.is_some() => Message::DismissPatchNotes,
            _ if self.update_notice.is_some() => Message::DismissUpdate,
            _ => return None,
        })
    }

    /// Scroll the page, for an arrow with no control in its direction.
    ///
    /// The region the ring is in, or failing that the largest on top: the
    /// page itself, or the text of the dialog over it.
    fn scroll_page(&self, step: focus::Step, from: Option<&focus::Target>) -> Task<Message> {
        let dy = match step {
            focus::Step::Up => -FOCUS_SCROLL,
            focus::Step::Down => FOCUS_SCROLL,
            focus::Step::Left | focus::Step::Right => return Task::none(),
        };
        let region = from.and_then(focus::spot).and_then(|spot| spot.region).or_else(focus::main_region);
        if let Some(region) = region {
            focus::scroll(region, dy);
        }
        Task::none()
    }

    /// Where the ring appears when it is not on anything yet.
    ///
    /// Back on the control that opened the menu just closed, then PLAY, the
    /// thing a controller user came for, then wherever reading starts, which
    /// in a menu is its first entry.
    /// Ask Steam for its on-screen keyboard for `field`, on a console that
    /// Steam started. Where the field is goes with it, the way SDL asks, so
    /// Steam can keep the keyboard off it.
    fn steam_keyboard(&mut self, field: &focus::Target) {
        if !self.console() || !launched_by_steam() {
            return;
        }
        let url = match focus::spot(field) {
            Some(spot) => {
                let at = spot.screen();
                format!(
                    "{STEAM_KEYBOARD}?XPosition={}&YPosition={}&Width={}&Height={}&Mode={}",
                    at.x.round() as i32,
                    at.y.round() as i32,
                    at.width.round() as i32,
                    at.height.round() as i32,
                    // Steam's floating keyboard modes: 0 one line, 1 several.
                    u8::from(spot.lines),
                )
            }
            None => STEAM_KEYBOARD.to_owned(),
        };
        if let Err(e) = tasks::open_url(&url) {
            self.error = Some(e);
        }
    }

    /// Put Steam's keyboard away, typing being done.
    fn steam_keyboard_closed(&mut self) {
        if !self.console() || !launched_by_steam() {
            return;
        }
        if let Err(e) = tasks::open_url(STEAM_KEYBOARD_CLOSE) {
            self.error = Some(e);
        }
    }

    fn focus_start(&mut self, spots: &[focus::Place]) -> Option<focus::Target> {
        let drawn = |target: &focus::Target| spots.iter().any(|p| &p.target == target);
        // Kept until it can be gone back to: a button that vanishes inside
        // a dialog, Install turning into nothing, must not make the ring
        // forget what opened the dialog.
        if let Some(back) = self.focus_return.clone().filter(|t| drawn(t)) {
            self.focus_return = None;
            return Some(back);
        }
        if let Some(resume) = self.focus_resume.take().filter(|t| drawn(t)) {
            return Some(resume);
        }
        if drawn(&focus::Target::Primary) {
            return Some(focus::Target::Primary);
        }
        // In setup, the slide's own button, so Enter after Enter walks it.
        if let Some(forward) = self.wizard.as_ref().map(wizard::forward).filter(|t| drawn(t)) {
            return Some(forward);
        }
        focus::first(spots)
    }

    /// Scroll the region `target` is in so that it can be seen.
    ///
    /// The control and the part of the region showing were recorded in the
    /// region's own coordinates, so the distance to scroll is the difference
    /// between them. A row's worth of room is kept beyond it: a list only
    /// draws the rows it shows, and the next one has to be drawn to be
    /// somewhere the arrows can go. Scrolling past either end is clamped by
    /// the list itself.
    fn scroll_into_view(target: &focus::Target) {
        let Some(spot) = focus::spot(target) else {
            return;
        };
        let Some(region) = spot.region else {
            return;
        };
        let (shown, at) = (spot.viewport, spot.bounds);
        let ahead = at.height + theme::MD * 2.0;
        let dy = match () {
            _ if at.y - ahead < shown.y => at.y - ahead - shown.y,
            _ if at.y + at.height + ahead > shown.y + shown.height => {
                at.y + at.height + ahead - (shown.y + shown.height)
            }
            _ => return,
        };
        focus::scroll(region, dy);
    }

    /// `content`, as a control the ring can land on, sending `press` when
    /// Enter is pressed on it. `None` is a disabled control.
    fn ring<'a>(
        &self,
        target: focus::Target,
        press: Option<Message>,
        content: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let press = press.map_or(focus::Press::Off, focus::Press::Send);
        focus::ring(target, press, content)
    }

    fn panel_open(&self) -> bool {
        self.picker_open.is_some()
            || self.gear_open
            || self.manager_open
            || self.browse_open
            || self.strip_open
            || self.app_menu_open
            || self.addon_menu_open.is_some()
            || self.tile_menu.is_some()
    }

    /// Ask for the open Game Settings popup's backups.
    fn list_backups(&self) -> Task<Message> {
        match &self.game_settings {
            Some(settings) => Task::perform(
                tasks::list_backups(self.layout.clone(), settings.code.clone()),
                Message::BackupsListed,
            ),
            None => Task::none(),
        }
    }

    /// Close every one of them.
    ///
    /// Anything that answers the question a panel was asking closes it:
    /// clicking the page, choosing a page, picking a game. A panel left
    /// hanging over the thing you just changed is the bug this prevents.
    fn close_panels(&mut self) {
        self.picker_open = None;
        self.gear_open = false;
        self.manager_open = false;
        self.browse_open = false;
        self.strip_open = false;
        self.app_menu_open = false;
        self.addon_menu_open = None;
        self.tile_menu = None;
    }

    /// The menu under the wordmark: the app's own business, as opposed to the
    /// band's (which game) and the tabs' (which page).
    ///
    /// Settings lives here rather than in the ribbon because it is somewhere
    /// you go rarely and deliberately, unlike Play and News.
    fn app_menu(&self) -> Element<'_, Message> {
        // No destination yet means no press and a dimmed face: the entry says
        // what is coming without pretending it is here.
        let entry = |icon: image::Handle, label: &'static str, message: Option<Message>| {
            let live = message.is_some();
            let face = row![
                image(icon)
                    .width(Length::Fixed(MENU_GLYPH))
                    .height(Length::Fixed(MENU_GLYPH))
                    .opacity(match live {
                        true => 1.0_f32,
                        false => DIMMED,
                    }),
                match live {
                    true => text(label).size(theme::BODY),
                    false => muted(label.to_owned(), theme::BODY),
                },
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
            self.ring(
                focus::Target::Menu(label),
                message.clone(),
                button(face.width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::ghost)
                    .on_press_maybe(message),
            )
        };
        let square = |icon: image::Handle, url: Option<&'static str>| {
            self.ring(
                focus::Target::Link(url.unwrap_or_default()),
                url.map(Message::OpenLink),
                link_square(icon, url, MENU_GLYPH),
            )
        };

        let rows = column![
            entry(gear_icon(), "Settings", Some(Message::Show(Screen::Settings))),
            entry(updates_icon(), "Lightforge Updates", Some(Message::Show(Screen::Updates))),
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
            entry(bug_icon(), "Report a Bug", Some(Message::OpenReport(ReportKind::Bug))),
            entry(
                feedback_icon(),
                "Send Feedback",
                Some(Message::OpenReport(ReportKind::Feedback)),
            ),
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
            entry(exit_icon(), "Exit", Some(Message::Exit)),
            // Where to find me, and where to say thanks. One row rather
            // than two entries with words: these are marks people recognise,
            // and a menu that ends in a line of labels reading "Ko-fi" and
            // "PayPal" would read as the point of the menu.
            row![
                square(discord_icon(), DISCORD_URL),
                square(x_icon(), X_URL),
                Space::new().width(Length::Fill),
                square(kofi_icon(), KOFI_URL),
                square(paypal_icon(), PAYPAL_URL),
            ]
            .spacing(theme::XS),
        ]
        .spacing(theme::XS)
        .width(Length::Fixed(240.0));

        focus::layer(container(rows).padding(theme::SM).style(theme::menu))
    }

    /// One addon's own settings: what is true of this addon rather than what
    /// to do to it right now.
    ///
    /// A version pin and re-install belong here too, which is the reason for
    /// a menu rather than more buttons on an already full row.
    fn addon_row_menu(
        &self,
        managed: &addonwork::Managed,
        version: &str,
        name: &str,
    ) -> Element<'static, Message> {
        let (provider, id) = (managed.provider.clone(), managed.id.clone());
        let default_auto = self.config.addon_auto_update;
        let entry = |label: String, message: Message| {
            focus::ring(
                focus::Target::Key(format!("addon menu entry {label}")),
                focus::Press::Send(message.clone()),
                button(text(label).size(theme::BODY).width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::ghost)
                    .on_press(message),
            )
        };

        // What this addon does today: its own choice where it has one, the
        // app's default otherwise.
        let auto = managed.auto_update.unwrap_or(default_auto);
        let mut rows = column![
            entry(
                match managed.ignored {
                    true => "Stop ignoring".to_owned(),
                    false => "Ignore this addon".to_owned(),
                },
                Message::AddonIgnore(provider.clone(), id.clone(), !managed.ignored),
            ),
            entry(
                match auto {
                    true => "Do not update automatically".to_owned(),
                    false => "Update automatically".to_owned(),
                },
                Message::AddonAutoUpdate(provider.clone(), id.clone(), Some(!auto)),
            ),
        ]
        .spacing(theme::XS)
        .width(Length::Fixed(240.0));

        // Only worth offering once the addon has an opinion to give up, and
        // worth naming the default so clearing it is not a leap of faith.
        if managed.auto_update.is_some() {
            rows = rows.push(entry(
                match default_auto {
                    true => "Follow the default (on)".to_owned(),
                    false => "Follow the default (off)".to_owned(),
                },
                Message::AddonAutoUpdate(provider.clone(), id.clone(), None),
            ));
        }

        // Choosing an older build on purpose, which is the other half of
        // pinning: "Pin to <version>" holds what is here, this goes and gets
        // something else. Only CurseForge publishes a build list, so this is
        // the only provider it is offered for.
        if managed.provider.eq_ignore_ascii_case("curse") {
            rows = rows.push(entry(
                "Details and changelog…".to_owned(),
                Message::ShowAddonDetail(provider.clone(), id.clone(), name.to_owned()),
            ));
            rows = rows.push(entry(
                "Choose a version…".to_owned(),
                Message::PickVersion(
                    provider.clone(),
                    id.clone(),
                    name.to_owned(),
                    managed.file_id.clone(),
                ),
            ));
        }

        // Pinning names the version it will hold, because "pin" alone does
        // not say *to what*: and after an update that is a different file.
        // An addon whose file the ledger never recorded has nothing to hold.
        match (&managed.pinned, &managed.file_id) {
            (Some(_), _) => {
                rows = rows.push(entry(
                    format!("Unpin (held at {version})"),
                    Message::AddonPin(provider, id, None),
                ));
            }
            (None, Some(file_id)) => {
                rows = rows.push(entry(
                    format!("Pin to {version}"),
                    Message::AddonPin(provider, id, Some(file_id.clone())),
                ));
            }
            (None, None) => {}
        }

        // Where the addon came from and who wrote it. Above the rule
        // because they leave Lightforge rather than change anything in it,
        // and the project page first: on CurseForge that page is where an
        // author is actually paid, so sending people to it is the point
        // rather than a courtesy.
        //
        // Each entry appears only when there is somewhere to go. A provider
        // that publishes no repository, and an addon installed before the
        // ledger kept these, both leave the menu one line shorter, which is
        // better than a line that opens nothing.
        let support = managed.support_url().map(str::to_owned);
        if managed.website_url.is_some() || support.is_some() {
            rows = rows.push(
                container(Space::new().height(Length::Fixed(1.0)))
                    .width(Length::Fill)
                    .style(theme::divider),
            );
        }
        if let Some(url) = managed.website_url.clone() {
            rows = rows.push(entry("Open project page".to_owned(), Message::OpenUrl(url)));
        }
        // Source before author profile, because that is where the money can
        // actually go: a repository usually carries a Sponsor button, where a
        // provider's author page is a list of their projects. Neither is a
        // donation link as such, no catalogue publishes one, so the label
        // promises help, not a payment form.
        if let Some(url) = support {
            rows = rows.push(entry("Support the author…".to_owned(), Message::OpenUrl(url)));
        }

        // An action, not a setting, so it sits under a rule of its own,
        // and busy means busy: a second fetch into the same directory while
        // one is running is the race the whole page guards against.
        rows = rows.push(
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
        );
        let reinstall = (!self.addons_locked())
            .then(|| Message::ReinstallAddon(managed.provider.clone(), managed.id.clone()));
        rows = rows.push(self.ring(
            focus::Target::Key("addon menu entry Re-install".to_owned()),
            reinstall.clone(),
            button(text("Re-install".to_owned()).size(theme::BODY).width(Length::Fill))
                .padding(theme::tap([theme::XS, theme::SM]))
                .style(theme::ghost)
                .on_press_maybe(reinstall),
        ));

        // Why either is worth having, in the menu that offers them: the words
        // alone do not say that the addon stays installed and working.
        rows = rows.push(muted(
            match (managed.ignored, managed.pinned.is_some()) {
                (true, _) => "Ignored: left out of every pass, automatic or not. \
                              Updating it from its own row still works."
                    .to_owned(),
                (false, true) => "Pinned: held at this version until you unpin it, \
                                  and its row offers no update."
                    .to_owned(),
                (false, false) => "Ignoring leaves it out of Check for updates and \
                                   Update all; pinning holds it at one version."
                    .to_owned(),
            },
            theme::CAPTION,
        ));

        focus::layer(container(rows).padding(theme::SM).style(theme::menu))
    }

    /// The games the band had no room for.
    fn overflow_menu(&self, hidden: &[ProductLine]) -> Element<'_, Message> {
        let mut rows = column![].spacing(theme::XS).width(Length::Fixed(220.0));
        for line in hidden {
            let mut entry = row![].spacing(theme::SM).align_y(Alignment::Center);
            if let Some(mark) = self.tile_art(line) {
                entry = entry.push(
                    image(mark.handle.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(theme::TILE * 0.6))
                        .height(Length::Fixed(theme::TILE * 0.6)),
                );
            }
            entry = entry.push(text(self.name_of(line)).size(theme::BODY));
            rows = rows.push(self.ring(
                focus::Target::Overflow(line.clone()),
                Some(Message::LineSelected(line.clone())),
                button(entry.width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::ghost)
                    .on_press(Message::LineSelected(line.clone())),
            ));
        }
        focus::layer(container(rows).padding(theme::SM).style(theme::menu))
    }

    /// A dropdown the ring can open and move through.
    ///
    /// A button and a menu of our own rather than a `pick_list`, whose list
    /// keeps its open state to itself, so a keyboard can neither open it nor
    /// move through it. Looks the same, and a mouse uses it the same way.
    /// `key` names it, and has to be unique on the page.
    #[allow(clippy::too_many_arguments)]
    fn picker<'a, T: ToString + PartialEq + Clone>(
        &self,
        key: String,
        options: Vec<T>,
        current: Option<T>,
        placeholder: &str,
        on_pick: impl Fn(T) -> Message,
        width: Length,
        size: f32,
    ) -> Element<'a, Message> {
        let label = match &current {
            Some(current) => text(current.to_string()).size(size),
            None => text(placeholder.to_owned()).size(size).color(theme::MUTED),
        };
        let toggle = Message::TogglePicker(key.clone());
        let face = button(
            row![
                label.width(Length::Fill),
                image(chevron_icon())
                    .width(Length::Fixed(APP_MENU_CHEVRON))
                    .height(Length::Fixed(APP_MENU_CHEVRON)),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .padding(theme::tap([theme::XS + 2.0, theme::SM]))
        .width(width)
        .style(theme::picker_button)
        .on_press(toggle.clone());
        let face = focus::ring(focus::Target::Key(format!("picker {key}")), focus::Press::Send(toggle), face);

        let menu = (self.picker_open.as_ref() == Some(&key)).then(|| {
            let mut rows = column![].spacing(theme::XS).width(Length::Fixed(240.0));
            for option in options {
                let chosen = Message::Picked(Box::new(on_pick(option.clone())));
                let name = option.to_string();
                let entry = button(text(name.clone()).size(size).width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .width(Length::Fill)
                    .style(match current.as_ref() == Some(&option) {
                        true => theme::selected,
                        false => theme::ghost,
                    })
                    .on_press(chosen.clone());
                rows = rows.push(focus::ring(
                    focus::Target::Key(format!("pick {key} {name}")),
                    focus::Press::Send(chosen),
                    entry,
                ));
            }
            focus::layer(
                mouse_area(container(rows).padding(theme::XS).style(theme::menu))
                    .on_press(Message::MenuClick),
            )
        });
        popover::popover(face, menu).align_left().into()
    }

    /// Every branch of the selected line, configured or not.
    ///
    /// `None` when the line has exactly one branch to offer, a picker with a
    /// single entry is furniture.
    fn branch_choices(&self) -> Option<(Vec<BranchChoice>, BranchChoice)> {
        let line = self.catalog.line_of(&self.selected.code);
        let choices: Vec<BranchChoice> = self
            .catalog
            .branches(&line)
            .into_iter()
            .map(|entry| BranchChoice {
                configured: self.choices.iter().any(|c| c.code == entry.code),
                branch: entry.branch,
                name: self
                    .families
                    // The UI is English; the install locale is for the game's files.
                    .version_name(entry.code.as_str(), "default")
                    .map(str::to_owned),
                code: entry.code.clone(),
                label: entry.label.clone(),
            })
            .collect();

        if choices.len() < 2 {
            return None;
        }
        let current = choices.iter().find(|c| c.code == self.selected.code)?.clone();
        Some((choices, current))
    }

    /// Switch to `line`, keeping whichever of its branches was last in use.
    ///
    /// Falls back to the line's live build, then to whatever of it *is*
    /// configured: selecting a tile must always land somewhere.
    fn select_line(&mut self, line: &ProductLine) -> Task<Message> {
        let remembered = self.last_branch.get(line).cloned();
        let configured = |code: &ProductCode| self.choices.iter().find(|c| &c.code == code).cloned();

        let choice = remembered
            .as_ref()
            .and_then(configured)
            .or_else(|| configured(&line.head()))
            .or_else(|| {
                self.choices
                    .iter()
                    .find(|c| &self.catalog.line_of(&c.code) == line)
                    .cloned()
            });

        match choice {
            Some(choice) => self.select(choice),
            None => Task::none(),
        }
    }

    /// Switch to one product, remembering it as its line's current branch.
    fn select(&mut self, choice: Choice) -> Task<Message> {
        if choice.code == self.selected.code {
            return Task::none();
        }
        self.last_branch.insert(self.catalog.line_of(&choice.code), choice.code.clone());
        self.selected = choice.clone();
        self.local = Local::default();
        self.remote = None;
        self.branding = None;
        self.status = "checking…".to_owned();
        // These are per-product, so they follow the selection rather than
        // carrying Retail's frame cap over to Classic's panel.
        self.load_tweak_inputs();
        self.reset_addon_screen();
        Task::batch([
            self.reload(choice.code),
            Task::done(Message::AutoUpdateAddons),
            // The popular list is per flavor, so it follows the selection for
            // the same reason the installed list does.
            match self.addon_tab == AddonTab::Browse {
                true => self.popular_if_unasked(),
                false => Task::none(),
            },
        ])
    }

    /// Load the popular list, unless a search is already in flight.
    ///
    /// Three things ask for it, opening Browse, clearing the box, switching
    /// product, and all three can happen while a typed search is still out.
    /// Its reply would be dropped by the query check anyway; not sending it is
    /// cheaper and keeps the spinner honest.
    fn popular_if_unasked(&mut self) -> Task<Message> {
        match self.searching {
            true => Task::none(),
            false => Task::done(Message::RunSearch),
        }
    }

    /// Clear everything on the addon screen that belongs to one product.
    ///
    /// A search is run against a product's flavor, so Retail's hits are the
    /// wrong answer for Classic and a filter typed over Retail's installed
    /// list hides Classic's. An armed Remove is worse than wrong: it is a
    /// confirmation the user gave about an addon they are no longer looking
    /// at. An in-flight search needs no cancelling, its reply is matched
    /// against the current query and dropped.
    fn reset_addon_screen(&mut self) {
        self.search_input.clear();
        self.search = addonwork::SearchOutcome::default();
        self.addon_filter.clear();
        self.addon_remove_armed = None;
        self.addon_menu_open = None;
        self.addon_auto_note = None;
        // Another product's addons, and their badges with them.
        self.updates.clear();
    }

    /// A screen that is not the dashboard: header, title, content.
    /// A page with a title, optionally something at the far end of that line,
    /// and a scrolling body.
    ///
    /// `aside` is for a fact about the page rather than a control, a count,
    /// a state. It rides on the title's line because that is where the eye
    /// already is, and because a figure like that under the title reads as a
    /// subtitle explaining the page, which it is not.
    fn page<'a>(
        &'a self,
        title: &'a str,
        subtitle: Option<String>,
        aside: Option<Element<'a, Message>>,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let mut heading = column![text(title).size(theme::TITLE)].spacing(theme::XS);
        if let Some(subtitle) = subtitle {
            heading = heading.push(muted(subtitle, theme::LABEL));
        }

        let mut top = row![heading].align_y(Alignment::Center);
        if let Some(aside) = aside {
            top = top.push(Space::new().width(Length::Fill)).push(aside);
        }

        column![
            self.header(),
            column![
                top,
                focus::region(
                    "page",
                    scrollable(content)
                        .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                        .style(theme::scrollbar)
                        .height(Length::Fill),
                ),
            ]
            .spacing(theme::LG)
            .padding(theme::LG),
        ]
        .into()
    }

    fn dashboard(&self) -> Element<'_, Message> {
        let action = self.action();
        let kind = self.kind_of(&self.selected.code);

        // A hand-off wears art only when the manifest names it *itself*.
        //
        // The `default` entry catches every code the manifest does not name,
        // which is deliberate for a WoW product, a line released after this
        // binary should look like WoW rather than like nothing, and wrong for
        // a hand-off, where it would dress Hearthstone in Retail's logo and
        // key art. That was the whole reason hand-offs showed no art at all.
        //
        // Now that each one carries its own logo, the rule can be the precise
        // one instead of the blunt one: inherit if you are WoW, otherwise show
        // art only if it is yours. A Blizzard game that appears in the catalog
        // before its art is published still gets nothing, which is correct.
        let themed_itself = self.manifest.as_ref().is_some_and(|manifest| {
            manifest
                .own_theme(&self.selected.code)
                .is_some_and(|theme| theme.logo.is_some())
        });
        let branding = match kind {
            Kind::Managed => self.branding.as_ref(),
            Kind::HandOff => self.branding.as_ref().filter(|_| themed_itself),
        };

        // The wordmark hangs under the app title in the top-left rather than
        // floating mid-page: the centre of the dashboard belongs to the art,
        // and a product logo reads as branding when it sits with Lightforge's
        // own rather than competing with the background for the middle.
        let logo: Element<Message> = match branding {
            // Drawn at the size [`theme::logo_size`] gives it, equal area
            // for every mark, however differently shaped, inside a box that
            // never moves.
            //
            // Near the box's left edge and centred down it. The marks are
            // trimmed to their own ink and no two are the same shape, so
            // something has to stay still as you move along the band: the
            // middle line does, and so does the left edge for everything but
            // the narrowest few, which [`theme::logo_offset`] nudges in so
            // they do not read as hanging further out than the rest. Centring
            // horizontally instead would hold them steadier still, but it
            // pushes a narrow mark most of a hundred points in from the page,
            // and the page edge is where a wordmark under the band belongs.
            Some(b) => {
                let (width, height) = theme::logo_size(b.logo_aspect, b.logo_ink);
                container(
                    image(b.logo.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(width))
                        .height(Length::Fixed(height)),
                )
                .align_x(Alignment::Start)
                .center_y(Length::Fixed(theme::LOGO_BOX_HEIGHT))
                .width(Length::Fixed(theme::LOGO_BOX_WIDTH))
                .padding(iced::Padding {
                    left: theme::logo_offset(width),
                    ..iced::Padding::ZERO
                })
                .into()
            }
            None => column![
                text(self.selected.label.clone()).size(theme::DISPLAY),
                muted("no artwork configured".to_owned(), theme::LABEL),
            ]
            .spacing(theme::XS)
            .into(),
        };

        let content = column![
            self.header(),
            column![
                // No gap of its own above. The wordmark belongs to the band
                // it hangs under, and anything between them leaves it
                // floating in the middle of the art with nothing to hold it.
                // The page's own top padding below is the whole gap; the
                // space beneath still separates it from the background.
                //
                // Inset from the left, though, rather than flush with the
                // panel below it. Each logo is trimmed to its own artwork, so
                // flush meant *touching* the page edge with no margin of its
                // own, alignment the eye reads as a mistake rather than as a
                // column. How far in is [`theme::LOGO_INSET`].
                // The art's own region: the wordmark and whatever space is
                // left, with the headlines down the right of it. A row rather
                // than more rows in the column, because the rail wants the
                // *height* of this region and the column would give it a
                // share of it instead.
                row![
                    column![
                        container(logo)
                            .align_x(Alignment::Start)
                            .width(Length::Fill)
                            .padding(iced::Padding {
                                top: 0.0,
                                right: 0.0,
                                bottom: theme::MD,
                                left: theme::LOGO_INSET,
                            }),
                        Space::new().height(Length::Fill),
                    ]
                    .width(Length::Fill),
                ]
                .extend(self.news_rail())
                .spacing(theme::MD)
                .height(Length::Fill),
                // A product Battle.net owns gets a panel that offers only what
                // Lightforge can actually do for it. Running the normal one
                // would put an INSTALL button on a game this launcher has no
                // way to install, and, worse, one that would plan a CASC
                // download into the WoW directory.
                match kind {
                    Kind::Managed => self.action_panel(&action),
                    Kind::HandOff => self.handoff_panel(),
                },
            ]
            .spacing(theme::MD)
            // No inset at all at the top, where the other three sides take
            // the page's: that inset is right for the action panel against
            // the window edge and wrong for a wordmark, which should read as
            // hanging from the band directly above it. The logo box carries
            // its own slack anyway, the mark is centred in it, so the gap
            // under the band is real without being spent here.
            .padding(iced::Padding {
                top: 0.0,
                right: theme::LG,
                bottom: theme::LG,
                left: theme::LG,
            }),
        ];

        match branding {
            Some(b) => stack![
                image(b.background.clone())
                    .content_fit(ContentFit::Cover)
                    .width(Length::Fill)
                    .height(Length::Fill),
                // A scrim at both ends. Art is chosen to look good, not to be
                // legible behind text, so the page darkens where text sits:
                // the controls at the bottom, and the navigation row at the
                // top, which hangs straight over the picture with no backdrop
                // of its own. That was a deliberate choice when the header was
                // rebuilt, a bar behind it fought the art, and this is the
                // fix the note left in its place called for: not a bar, a
                // gradient, so bright art loses contrast to the text instead
                // of the other way round.
                //
                // Offsets run bottom (0.0) to top (1.0). The top pair holds
                // rather than ramps: the navigation row sits about 6% down, so
                // a gradient that starts fading at the very edge leaves the
                // text in the middle of the fade at roughly half strength,
                // legible on most art and marginal on a bright sky. Holding
                // full strength across the row and fading below it puts the
                // whole of the text on even cover, and the fade lands over the
                // game band, which carries its own card and needs none.
                container(Space::new().height(Length::Fill))
                    .width(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                            iced::gradient::Linear::new(0.0)
                                .add_stop(0.0, iced::Color { a: 0.95, ..theme::BG })
                                .add_stop(0.45, iced::Color { a: 0.35, ..theme::BG })
                                .add_stop(0.84, iced::Color { a: 0.12, ..theme::BG })
                                .add_stop(0.93, iced::Color { a: 0.75, ..theme::BG })
                                .add_stop(1.0, iced::Color { a: 0.75, ..theme::BG }),
                        ))),
                        ..container::Style::default()
                    }),
                content,
            ]
            .into(),
            None => content.into(),
        }
    }

    /// Headlines for the game on screen, down the right of the key art.
    ///
    /// `None` draws nothing at all, which is the right answer more often than
    /// it looks:
    ///
    /// - **For a game Battle.net hands off**: Diablo, Hearthstone, Overwatch,
    ///   there will never be an article. Every feed the proxy merges is
    ///   Wowhead, so an empty rail beside StarCraft would not be "nothing
    ///   today", it would be "nothing, ever". See [`news::Item::line`].
    /// - **Before the fetch has been asked for**, so the rail does not appear
    ///   and then change shape under the reader.
    /// - **When the fetch failed with nothing cached**, because a panel whose
    ///   whole content is an error is worse than the art it is covering.
    ///
    /// A line with no recent coverage *does* get the rail, saying so. That is
    /// a fact about this week rather than about the feed, and the difference
    /// matters to someone wondering whether the thing is broken.
    fn news_rail(&self) -> Option<Element<'_, Message>> {
        /// Width of the rail. Taken from the shape Gabriel drew over a
        /// screenshot rather than derived from anything.
        const RAIL: f32 = 260.0;
        /// How many headlines fit before the rail wants scrolling, which it
        /// will not get: this is a glance, and the News page is one click
        /// away at the foot of it.
        const MOST: usize = 5;

        if !self.config.news_on_play {
            return None;
        }
        // Hand-offs never match, but asking the feed would also mean drawing
        // the rail while the fetch is in flight for a game that cannot have
        // news. Answered from the product instead.
        if self.kind_of(&self.selected.code) == Kind::HandOff {
            return None;
        }
        let line = self.catalog.line_of(&self.selected.code);

        let body: Element<'_, Message> = match &self.news {
            Some(headlines) => {
                let items = headlines.feed.about(&line, MOST);
                match items.is_empty() {
                    true => muted(
                        format!("Nothing recent for {}.", line.label()),
                        theme::CAPTION,
                    )
                    .into(),
                    false => column(items.into_iter().map(|item| self.rail_item(item)))
                        .spacing(theme::XS)
                        .into(),
                }
            }
            // Pending. `news_asked` without a feed means it failed with
            // nothing on disk, which is the case that draws no rail at all.
            None if !self.news_asked => {
                muted("Fetching headlines…".to_owned(), theme::CAPTION).into()
            }
            None => return None,
        };

        let more = button(
            row![
                text("More in News").size(theme::LABEL),
                Space::new().width(Length::Fill),
                text("→").size(theme::LABEL),
            ]
            .align_y(Alignment::Center),
        )
        .padding(theme::tap([theme::XS, theme::SM]))
        .width(Length::Fill)
        .style(theme::ghost)
        .on_press(Message::Show(Screen::News));

        Some(
            container(
                column![
                    muted("LATEST NEWS".to_owned(), theme::CAPTION),
                    body,
                    // Pinned to the foot however few headlines there are, so
                    // the link does not wander up the rail on a quiet week.
                    Space::new().height(Length::Fill),
                    more,
                ]
                .spacing(theme::SM),
            )
            .width(Length::Fixed(RAIL))
            .height(Length::Fill)
            .padding(theme::MD)
            .style(theme::floating)
            .into(),
        )
    }

    /// One headline in the rail: the date, then the title, and nothing else.
    ///
    /// No summary and no picture, unlike [`Lightforge::news_card`] on the News
    /// page. This is 260 points wide standing over artwork, a thumbnail
    /// would leave room for about four words, and the summary is the reason
    /// to click rather than something to read here.
    fn rail_item<'a>(&self, item: &'a news::Item) -> Element<'a, Message> {
        button(
            column![
                muted(short_day(&item.published), theme::CAPTION),
                text(item.title.clone()).size(theme::LABEL),
            ]
            .spacing(2.0)
            .width(Length::Fill),
        )
        .padding([theme::XS, theme::SM])
        .width(Length::Fill)
        .style(theme::card_button)
        .on_press(Message::OpenArticle(item.url.clone()))
        .into()
    }

    /// Status, progress, and every action for the selected product.
    fn action_panel(&self, action: &ActionState) -> Element<'_, Message> {
        let color = match action {
            ActionState::Install => theme::INSTALL,
            ActionState::Update { .. } => theme::UPDATE,
            ActionState::Play => theme::PLAY,
            ActionState::Downloading(_) | ActionState::Paused(_) | ActionState::Queued => {
                theme::PAUSE
            }
            ActionState::Error { .. } => theme::ERROR,
            ActionState::Unknown | ActionState::Offline { .. } => theme::PAUSE,
        };

        let mut left = column![].spacing(theme::SM).width(Length::Fill);

        // The branch belongs here rather than in the header: it is not
        // navigation, it decides what every line below it refers to, which
        // version is "latest", which install "installed", which files "Open
        // Files" opens. Only shown where there is a choice to make.
        if let Some((branches, current)) = self.branch_choices() {
            left = left.push(
                row![
                    muted("version".to_owned(), theme::CAPTION),
                    self.picker(
                        "version".to_owned(),
                        branches,
                        Some(current),
                        "",
                        Message::BranchSelected,
                        Length::Shrink,
                        theme::LABEL,
                    ),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }

        left = left.push(match (&self.error, action) {
            (Some(e), _) => text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR),
            // The drive outranks whatever the last refresh had to say: a
            // "latest 11.2.7" under a game that cannot be started is the
            // wrong fact to be reading.
            (None, ActionState::Offline { drive }) => text(format!(
                "{drive} is not mounted — the game is still on it, untouched"
            ))
            .size(theme::LABEL)
            .color(theme::ERROR),
            (None, _) => text(self.status.clone()).size(theme::LABEL).color(theme::MUTED),
        });

        left = self.scan_progress(left);

        if let Some(progress) = progress_of(action) {
            // Two jobs for one bar. While bytes are moving it reports them.
            // Before the total is known, and again after the last byte lands
            // while the install is still extracting and writing `.build.info`,
            // there is no fraction to report but there is still work going on,
            // so the bar sweeps instead of sitting at a number that would be
            // a lie in one direction or the other.
            let sweeping = progress.total_bytes == 0
                || progress.downloaded_bytes >= progress.total_bytes;
            let value = match sweeping {
                true => sweep(),
                false => progress.fraction(),
            };
            left = left.push(progress_bar(0.0..=1.0, value).girth(6));
            left = left.push(
                row![
                    mono(bytes_of(progress.downloaded_bytes, progress.total_bytes)),
                    dot(),
                    mono(rate_of(progress.bytes_per_second)),
                    dot(),
                    mono(eta_of(progress)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
            let stage = self
                .download_for(&self.selected.code)
                .map(|d| d.sample.stage.clone())
                .unwrap_or_default();
            if !stage.is_empty() {
                left = left.push(muted(stage, theme::CAPTION));
            }
        } else if let Some(version) = &self.local.version {
            left = left.push(
                row![muted("installed".to_owned(), theme::CAPTION), mono(version.clone())]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
            );
        }

        // What an Install or Update would actually fetch beyond the base game.
        // The size surprise this prevents is the whole reason the gate exists:
        // on the Forever beta the texture pack alone is 37 GiB.
        let opted = self.opted_optional(&self.selected.code);
        if !opted.is_empty() && matches!(action, ActionState::Install | ActionState::Update { .. })
        {
            let names: Vec<String> = opted.iter().map(|t| optional_label(t)).collect();
            left = left.push(muted(
                format!("includes optional content: {}", names.join(", ")),
                theme::CAPTION,
            ));
        }

        // Three words in a row with nothing on them read as a caption, not as
        // somewhere to go. A mark in front of each says "this is a control"
        // before the word is read at all.
        //
        // Padded on all four sides, not just vertically: the hover fill is the
        // button's own bounds, so with no horizontal padding the glyph and the
        // last letter sit flush against the rounded edge and the label reads as
        // if it were spilling out of the box. The cost is that the group starts
        // one [`theme::SM`] in from "version" and "installed" above it, which
        // is a smaller wrong than text touching its own border.
        let link = |mark: image::Handle, label: &'static str, message: Option<Message>| {
            button(
                row![
                    image(mark)
                        .width(Length::Fixed(LINK_GLYPH))
                        .height(Length::Fixed(LINK_GLYPH))
                        .opacity(match message.is_some() {
                            true => 1.0_f32,
                            false => DIMMED,
                        }),
                    text(label).size(theme::tap_text(theme::LABEL)),
                ]
                .spacing(theme::XS)
                .align_y(Alignment::Center),
            )
            .on_press_maybe(message)
            .style(theme::ghost)
            .padding(theme::tap([theme::XS, theme::SM]))
        };

        left = left.push(
            row![
                // Addons are already per-product, the screen titles itself
                // with the selection and every task takes its code, so this
                // is navigation, not a second copy of it. Disabled until
                // there is an install to put an addon into.
                self.ring(
                    focus::Target::Addons,
                    self.local.version.is_some().then_some(Message::Show(Screen::Addons)),
                    link(
                        package_icon(),
                        "Manage Addons",
                        self.local.version.is_some().then_some(Message::Show(Screen::Addons)),
                    ),
                ),
                self.ring(
                    focus::Target::Tweaks,
                    Some(Message::Show(Screen::Tweaks)),
                    link(sliders_icon(), "Game Optimizations", Some(Message::Show(Screen::Tweaks))),
                ),
                self.ring(
                    focus::Target::Subscription,
                    Some(Message::ManageSubscription),
                    link(card_icon(), "Manage Subscription", Some(Message::ManageSubscription)),
                ),
            ]
            // Each link now carries its own air, so the row's gap only has to
            // keep two hover fills from touching; [`theme::MD`] on top of that
            // padding would read as three separate groups.
            .spacing(theme::XS)
            .align_y(Alignment::Center),
        );

        // Battle.net's own words for a game that downloads before it opens.
        // A caption and a label only: PLAY stays PLAY, because whether the
        // game is open is decided on Blizzard's side, and this cache can
        // still say "pre-install" on release day.
        let release_date = self.families.release_date(self.selected.code.as_str(), "default");
        let label = match (action, release_date) {
            (ActionState::Install, Some(_)) => "PRE-INSTALL",
            _ => action.label(),
        };
        let primary = button(
            container(text(label).size(theme::HEADING).color(iced::Color::WHITE))
                .center_x(Length::Fill),
        )
        .style(theme::filled_left(color))
        .padding([theme::MD, theme::LG])
        // 190 + the tab's 50 is 240: the width of the Battle.net button
        // underneath, so the two stack flush rather than nearly flush.
        .width(Length::Fixed(190.0))
        .height(Length::Fixed(theme::CONTROL_HEIGHT));
        let battle_net = button(
            container(text("Launch via Battle.net").size(theme::tap_text(theme::LABEL))).center_x(Length::Fill),
        )
        .style(theme::outlined)
        .padding(theme::tap([theme::SM, theme::MD]))
        .width(Length::Fixed(240.0));

        // A split button, welded to the primary action rather than floating
        // beside it: the cog is a second way into the same thing, which is how
        // Battle.net builds it. Zero spacing and complementary corner radii
        // are what make the pair read as one control.
        //
        // The icon is an image, not a glyph, the UI font has no U+2699 and
        // renders it as a tofu box.
        let gear = button(
            image(gear_icon())
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(18.0)),
        )
        .style(theme::filled_right(color))
        .padding([theme::MD, theme::MD])
        // The same fixed height as the action half, never `Fill`: inside a
        // column, `Fill` resolves against the *column*, and the tab grows into
        // a green stripe the height of the whole stack.
        .height(Length::Fixed(theme::CONTROL_HEIGHT))
        .on_press(Message::ToggleGear);

        // Pinned to the control rather than placed near it. Everything else
        // was layout, and layout cannot put a child over its sibling: in the
        // panel's column the menu sized the card, and above the card its top
        // edge fell between the menu and the button.
        let control = popover::popover(
            row![
                self.ring(
                    focus::Target::Primary,
                    (!self.busy && !matches!(action, ActionState::Unknown)).then_some(Message::Primary),
                    match self.busy || matches!(action, ActionState::Unknown) {
                        true => primary,
                        false => primary.on_press(Message::Primary),
                    },
                ),
                self.ring(focus::Target::Gear, Some(Message::ToggleGear), gear),
            ]
            .align_y(Alignment::Center),
            self.gear_open.then(|| self.gear_menu()),
        );

        let right = column![
            control,
            self.ring(
                focus::Target::LaunchViaBattleNet,
                (!self.busy).then_some(Message::BattleNet),
                match self.busy {
                    true => battle_net,
                    false => battle_net.on_press(Message::BattleNet),
                },
            ),
        ]
        .push(release_date.map(|date| muted(servers_open(date), theme::CAPTION)))
        .spacing(theme::SM)
        .align_x(Alignment::Center)
        .width(Length::Shrink);

        container(row![left, right].spacing(theme::XL).align_y(Alignment::Center))
            .padding(theme::LG)
            .style(theme::floating)
            .into()
    }

    /// Per-product performance settings: what this machine can do for the
    /// game, and what the client should be told to do for itself.
    /// The Game Optimizations page.
    ///
    /// Laid out as a preset row over a two-column grid, which is a change of
    /// shape rather than of content: the switches used to be grouped by the
    /// *mechanism* that applies them, host binaries, DXVK, `Config.wtf`
    /// lines, environment, so a card's height tracked how many cvars happened
    /// to share a plumbing detail. Six full-width cards of wildly different
    /// heights was the result. They are grouped by what somebody is trying to
    /// do instead, which balances them, and the grid pins the rest: four cards
    /// of one size, two to a row, so the page reads as a grid rather than as a
    /// list of boxes that each happened to end somewhere.
    fn tweaks_screen(&self) -> Element<'_, Message> {
        let tweaks = self.tweaks();

        // Four cards on one grid, every box the same size whatever is in it.
        // The row carries the height and the cards fill it, which is the only
        // arrangement that gives a uniform box: sized to their content they
        // end at four different places, and iced cannot stretch two fluid
        // siblings against each other with nothing to measure (see
        // [`Lightforge::tweak_card`]).
        let top = Length::Fixed(self.card_height(CardRow::Top, &tweaks));
        let bottom = Length::Fixed(self.card_height(CardRow::Bottom, &tweaks));
        let body = column![
            self.preset_row(&tweaks),
            row![self.performance_card(&tweaks), self.looks_card(&tweaks)]
                .spacing(theme::MD)
                .height(top),
            row![self.input_card(&tweaks), self.advanced_card(&tweaks)]
                .spacing(theme::MD)
                .height(bottom),
            self.env_card(Scope::Product, &tweaks),
        ]
        .spacing(theme::MD);

        // The one sentence that used to be a paragraph inside the biggest
        // card. It is true of every switch above, so it belongs under all of
        // them rather than inside one.
        let mut body = body.extend(
            self.hint(
                "The client settings here are written to this product's Config.wtf and \
                 re-applied before every launch, because the client rewrites that file from \
                 memory when it exits. Change one of them in-game and the setting here wins \
                 next time.",
            )
            .map(Element::from),
        );
        if let Some(e) = &self.error {
            body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }

        self.page(
            "Game Optimizations",
            Some(format!("{} · applied when this product launches", self.selected.label)),
            None,
            body.into(),
        )
    }

    /// A checkbox in one of the cards below.
    fn tweak_check<'a>(
        &self,
        scope: Scope,
        label: &'a str,
        which: Tweak,
        on: bool,
    ) -> Element<'a, Message> {
        clickable(
            format!("tweak {label}"),
            tick(on)
                .label(label)
                .on_toggle(move |on| Message::Tweak(scope, which, on))
                .text_size(theme::tap_text(theme::BODY)),
        )
    }

    /// How tall every card on the grid is.
    ///
    /// A pinned number, which is the price of boxes that are the same size
    /// whatever is in them, and it has to clear the *fullest* card on the
    /// *busiest* machine, because a card is a fixed box and content past its
    /// edge is content nobody can reach.
    ///
    /// **Sized per row, not per page.** The two cards you can see beside each
    /// other still match exactly, which is the part that reads as a grid,
    /// but Performance and Looks are naturally shorter than Input and
    /// Advanced, and one number for all four reserved a band of nothing
    /// under the last switch of the top row. A row is the unit people
    /// compare; the page is not.
    ///
    /// Two axes beyond that. Hints are a line of prose under most switches, and
    /// a number tuned with them on wastes a third of the page with them off.
    /// The Wayland backend is the same problem again: ticking it opens two
    /// more rows in Advanced, and sizing every card for rows that are not
    /// there left a visible band of nothing under the last switch in both
    /// rows of the grid. So the grid grows when they appear and shrinks when
    /// they go. **All four cards still match each other at any moment**,
    /// which is the rule. It is the page that changes height, not one box
    /// against its neighbour.
    ///
    /// Every number here was measured on screen at the state it names, not
    /// computed: a card that clips gives no sign of it.
    ///
    /// If a card ever outgrows this, raise it here. It is the one number the
    /// layout depends on.
    fn card_height(&self, row: CardRow, tweaks: &Tweaks) -> f32 {
        let (base, backend) = match (row, self.hints) {
            // Performance is the taller of the first pair: six switches, the
            // shader-cache note, and the missing-tools line on its floor.
            // Nothing in this row grows with the Wayland backend.
            (CardRow::Top, true) => (300.0, 0.0),
            (CardRow::Top, false) => (250.0, 0.0),
            // Advanced is the taller of the second, and the only card that
            // changes height: the second number is HDR, the monitor field
            // and its own hint, which is why it is larger with hints on.
            (CardRow::Bottom, true) => (486.0, 80.0),
            (CardRow::Bottom, false) => (312.0, 66.0),
        };
        // Those rows only exist once the backend is on, and only where the
        // backend can be offered at all.
        match self.tools.wayland && tweaks.wayland {
            true => base + backend,
            false => base,
        }
    }

    /// One card of the grid: a heading, its own one-line brief, and content.
    ///
    /// Both lengths are `Fill`: the width splits the row evenly, and the
    /// height takes whatever [`Lightforge::card_height`] gave the row.
    ///
    /// **`height(Fill)` only works because the row is a fixed height.** iced's
    /// flex sizes cross-axis-fluid children against their non-fluid siblings
    /// (`layout/flex.rs`, the `cross_compress` path), so two fluid cards in a
    /// Shrink-height row have nothing to measure against, inside a vertical
    /// `scrollable`, where the height limit is unbounded, `cross` stays `0.0`
    /// and both cards draw as nothing at all. That was a real bug, not a
    /// hypothetical. Give the row a height or do not make the cards fluid.
    fn tweak_card<'a>(
        &self,
        title: &'a str,
        brief: &'a str,
        content: iced::widget::Column<'a, Message>,
    ) -> Element<'a, Message> {
        let mut inner = column![text(title).size(theme::HEADING)].spacing(theme::SM);
        inner = inner.extend(self.hint(brief).map(Element::from));
        let content = focus::group(format!("card {title}"), inner.push(content).spacing(theme::SM));
        container(content)
            .padding(theme::MD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::card)
            .into()
    }

    /// The starting point: one press for the frames-versus-scenery trade.
    ///
    /// Full width and above the grid because it is one decision about
    /// everything under it, and because a segmented control inside a column
    /// would read as a fourth setting rather than the thing that sets them.
    fn preset_row(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let current = tweaks.preset();

        // The lit one takes the ring too, though pressing it changes
        // nothing: a row the ring skips a segment of reads as broken.
        let segment = |label: &'static str, chosen: bool, message: Message| {
            self.ring(
                focus::Target::Key(format!("preset {label}")),
                Some(message.clone()),
                button(text(label).size(theme::BODY))
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .style(match chosen {
                        true => theme::selected,
                        false => theme::ghost,
                    })
                    .on_press_maybe((!chosen).then_some(message)),
            )
        };

        // Custom first, and lit whenever nothing else is. It is not a fourth
        // preset but the absence of one, which is what makes it stay lit while
        // you tick boxes: setting a switch by hand matches no preset, so this
        // is still the answer. Pressing it clears the managed switches, so the
        // panel it describes really is empty.
        let mut segments = row![segment("Custom", current.is_none(), Message::TweakCustom)]
            .spacing(theme::XS);
        for preset in Preset::ALL {
            segments = segments.push(segment(
                preset.label(),
                current == Some(preset),
                Message::TweakPreset(preset),
            ));
        }

        // The pill repeats what is lit rather than naming a second state:
        // Custom is a real position on the row now, so there is no "neither"
        // left to give its own word.
        let brief = match current {
            Some(preset) => preset.summary().to_owned(),
            None => "Your own mix. Nothing here is set for you.".to_owned(),
        };
        let mut label = column![
            row![
                text("Starting point").size(theme::HEADING),
                pill(
                    match current {
                        Some(preset) => preset.label().to_owned(),
                        None => "Custom".to_owned(),
                    },
                    match current {
                        Some(_) => theme::ACCENT,
                        None => theme::MUTED,
                    },
                ),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::XS);
        label = label.extend(self.hint(brief).map(Element::from));

        container(
            row![label.width(Length::Fill), segments]
                .spacing(theme::LG)
                .align_y(Alignment::Center),
        )
        .padding(theme::MD)
        .width(Length::Fill)
        .style(theme::card)
        .into()
    }

    /// Everything that buys frames.
    ///
    /// The host tools are detected, never assumed, so a switch missing here
    /// means the binary is, which is said at the foot of the card rather than
    /// as a hint per switch, because three copies of the same caveat was a
    /// large part of what made this page ragged.
    fn performance_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let mut content = column![].spacing(theme::SM);

        if self.tools.game_performance.is_some() {
            content = content.push(self.tweak_check(Scope::Product,
                "Hold the CPU on its performance profile",
                Tweak::GamePerformance,
                tweaks.wants_game_performance(),
            ));
        }
        if self.tools.gamemode.is_some() {
            content = content.push(self.tweak_check(Scope::Product,
                "Launch through GameMode",
                Tweak::GameMode,
                tweaks.gamemode,
            ));
        }
        content = content.push(self.tweak_check(Scope::Product,
            "Stop profiling addon execution",
            Tweak::SkipAddonProfiling,
            tweaks.skip_addon_profiling,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Drop water and weather detail in raids and battlegrounds",
            Tweak::ReduceRaidEffects,
            tweaks.reduce_raid_effects,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Skip non-critical world preloading",
            Tweak::FasterZoneLoads,
            tweaks.faster_zone_loads,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Turn off cloth and NPC physics",
            Tweak::NoClothPhysics,
            tweaks.no_cloth_physics,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Give the shader cache room",
            Tweak::BigShaderCache,
            tweaks.big_shader_cache,
        ));
        content = content.extend(
            self.hint(
                "Mesa's default ceiling is small enough that a long session throws away \
                 shaders it is about to need again, which is felt as stutter that never \
                 settles. Raises it to 10 GB.",
            )
            .map(Element::from),
        );

        // The box is a fixed height now, so this sits on its floor rather
        // than straight under the last switch.
        content = content.push(Space::new().height(Length::Fill));
        let missing: Vec<&str> = [
            self.tools.game_performance.is_none().then_some("game-performance"),
            self.tools.gamemode.is_none().then_some("GameMode"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            content = content.push(muted(
                format!("{} is not installed, so it is not offered.", missing.join(" and ")),
                theme::CAPTION,
            ));
        }

        self.tweak_card("Performance", "Everything here buys frames.", content)
    }

    /// Everything that spends frames, including the optional downloads.
    ///
    /// The glow switch is phrased as "turn off", which reads oddly beside
    /// "Maximum view distance" and was briefly inverted to match it. That was
    /// wrong twice over: the handler kept writing the raw value, so the box
    /// sprang back and looked dead; and once fixed, an empty box meant
    /// `no_screen_effects = true`, which writes `ffxGlow 0`. Custom clears
    /// every switch, so an inverted box would leave Custom visibly empty while
    /// quietly turning the glow off. A box on this panel has to mean "unset"
    /// when empty, and that settles the wording.
    fn looks_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let mut content = column![
            self.tweak_check(Scope::Product,
                "Maximum view distance and ground clutter",
                Tweak::MaxDrawDistance,
                tweaks.max_draw_distance,
            ),
            self.tweak_check(Scope::Product,
                "Turn off the full-screen glow and death filter",
                Tweak::NoScreenEffects,
                tweaks.no_screen_effects,
            ),
        ]
        .spacing(theme::SM);

        // Optional downloads used to be a card of its own, which put a storage
        // decision on an optimization page and left a 70px box next to a 300px
        // one. It is the same trade as the two above, scenery for something
        // else, so it sits with them, and says what it costs.
        let profile = self.config.product(&self.selected.code).map(|p| &p.profile);
        let known = profile.is_some_and(|p| !p.optional_available.is_empty());
        let available =
            offered_only(profile.map(|p| p.optional_available.as_slice()).unwrap_or(&[]));
        let opted = self.opted_optional(&self.selected.code);

        match (known, available.is_empty()) {
            (false, _) => {
                content = content.extend(
                    self.hint(
                        "What else a build offers is only known once its manifest has been \
                         read — use Check for Updates, under the cog beside PLAY.",
                    )
                    .map(Element::from),
                );
            }
            (true, true) => {}
            (true, false) => {
                for name in &available {
                    let on = opted.iter().any(|n| n == name);
                    let tag = name.clone();
                    content = content.push(clickable(
                        format!("optional {name}"),
                        tick(on)
                            .label(optional_label(name))
                            .on_toggle(move |on| Message::OptionalContent(tag.clone(), on))
                            .text_size(theme::tap_text(theme::BODY)),
                    ));
                }
                content = content.extend(
                    self.hint(
                        "Off unless asked for, the same default Battle.net uses. Turning one \
                         on adds it to the next download.",
                    )
                    .map(Element::from),
                );
            }
        }

        content = content.push(Space::new().height(Length::Fill));
        self.tweak_card(
            "Looks",
            "Everything here spends frames. Same trade, stated once.",
            content,
        )
    }

    /// How it feels to play, which is not how fast it runs.
    ///
    /// Four of the five are Wine or multi-monitor fixes rather than
    /// preferences, which is why they sit together: each one is the answer to
    /// a specific thing the game does wrong under Linux, and every name here
    /// was read out of the client's own string table rather than a wiki,
    /// `HardwareCursor` is real and `gxCursor`, which the widely-copied lists
    /// name, is not.
    fn input_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let mut content = column![self.tweak_check(Scope::Product,
            "Raw mouse input",
            Tweak::RawMouse,
            tweaks.raw_mouse,
        )]
        .spacing(theme::SM);
        content = content.extend(
            self.hint(
                "Also the fix for the cursor jumping back to the middle of the window after \
                 a click, which is a Wine problem rather than a game one.",
            )
            .map(Element::from),
        );
        content = content.push(self.tweak_check(Scope::Product,
            "Keep the pointer inside the window",
            Tweak::ClipCursor,
            tweaks.clip_cursor,
        ));
        content = content.extend(
            self.hint(
                "Stops the cursor walking onto another monitor mid-fight. Only worth it on \
                 more than one screen.",
            )
            .map(Element::from),
        );
        content = content.push(self.tweak_check(Scope::Product,
            "Draw a software cursor",
            Tweak::SoftwareCursor,
            tweaks.software_cursor,
        ));
        content = content.extend(
            self.hint(
                "The hardware cursor is the one that goes missing or lags behind the pointer \
                 under Wine. A software one also shows up in screenshots.",
            )
            .map(Element::from),
        );
        content = content.push(self.tweak_check(Scope::Product,
            "Ignore the desktop's mouse acceleration",
            Tweak::NoMouseAcceleration,
            tweaks.no_mouse_acceleration,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Show the cursor in relative mode",
            Tweak::ShowCursorInRelativeMode,
            tweaks.show_cursor_in_relative_mode,
        ));
        content = content.extend(
            self.hint(
                "The client's own suggestion for relative-mode bugs, in its words: it \
                 \"may work around issues in some rare cases\".",
            )
            .map(Element::from),
        );

        content = content.push(Space::new().height(Length::Fill));
        self.tweak_card("Input and feel", "Nothing here changes how fast it runs.", content)
    }

    /// The fork and the ceiling: choices with no better default.
    fn advanced_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let apis = vec![
            ApiChoice(None),
            ApiChoice(Some(GraphicsApi::D3d11)),
            ApiChoice(Some(GraphicsApi::D3d12)),
        ];
        let mut content = column![row![
            muted("Graphics API".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
            self.picker(
                "graphics-api".to_owned(),
                apis,
                Some(ApiChoice(tweaks.graphics_api)),
                "",
                Message::TweakApi,
                Length::Fill,
                theme::BODY,
            ),
        ]
        .spacing(theme::MD)
        .align_y(Alignment::Center)]
        .spacing(theme::SM);
        content = content.extend(
            self.hint(
                "A genuine fork rather than a best value: D3D12 runs on VKD3D, D3D11 on \
                 DXVK, and which is faster depends on the card.",
            )
            .map(Element::from),
        );
        content = content.push(
            row![
                muted("Frame cap".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
                clickable(
                    "frame cap",
                    text_input("uncapped", &self.fps_input)
                        .on_input(Message::TweakFpsCap)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                ),
                muted("fps".to_owned(), theme::LABEL),
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        );
        content = content.extend(
            self.hint(
                "The client's own limit rather than the driver's, so it keeps working if the \
                 Direct3D version above changes. The client's floor is 8.",
            )
            .map(Element::from),
        );
        content = content.push(
            row![
                muted("When unfocused".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
                clickable(
                    "frame cap unfocused",
                    text_input("30", &self.fps_bk_input)
                        .on_input(Message::TweakFpsCapBackground)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                ),
                muted("fps".to_owned(), theme::LABEL),
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        );
        content = content.extend(
            self.hint(
                "What the game may draw while you are in a browser. The client's own \
                 default is 30; lower gives the rest of the machine more back.",
            )
            .map(Element::from),
        );
        content = content.extend(self.runtime_rows(Scope::Product, tweaks));
        if self.tools.mangohud.is_some() {
            content = content.push(self.tweak_check(Scope::Product,
                "Show the MangoHud overlay",
                Tweak::MangoHud,
                tweaks.mangohud,
            ));
        }

        // An action rather than a setting, so it sits at the foot of the card
        // under the switches, and it belongs with the graphics fork above it,
        // because changing the Direct3D version is the usual reason to want it.
        content = content.push(Space::new().height(Length::Fill));
        content = content
            .push(secondary("Clear shader cache", Message::ClearShaderCache).enabled(true));
        content = content.extend(
            self.hint(
                "Shaders this product compiled under an older Proton build. Clearing them \
                 costs one session of stutter and affects no other product.",
            )
            .map(Element::from),
        );

        self.tweak_card("Advanced", "Choices with no better default.", content)
    }

    /// The Proton-side switches: which graphics backend the game runs on.
    ///
    /// A method of its own because the Battle.net profile in Settings offers
    /// exactly these and nothing else, a hand-off game has no `Config.wtf`,
    /// so the environment is the only thing Lightforge can set for it, and
    /// two copies of these rows would be two chances to drift.
    ///
    /// **The Wayland switch is hidden outside a Wayland session.** Proton
    /// ignores `PROTON_ENABLE_WAYLAND` without `WAYLAND_DISPLAY`, so offering
    /// it on X11 would be a control with no effect. HDR is nested under it
    /// for a different reason: Proton would set `DXVK_HDR` under XWayland
    /// too, and that is not a path to send anybody down.
    // `'static`, not an elided reborrow of `&self`. `Column::extend` unifies
    // the element lifetime exactly, unlike a single `push` which can coerce,
    // and the cards hold `Action<'static>` from [`secondary`], which is
    // invariant. Nothing in here actually borrows from `self`: `text_input`
    // takes a plain `&str` and keeps its own copy.
    fn runtime_rows(&self, scope: Scope, tweaks: &Tweaks) -> Vec<Element<'static, Message>> {
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        if !self.tools.wayland {
            return rows;
        }

        rows.push(
            self.tweak_check(scope, "Run on Proton's Wayland backend", Tweak::Wayland, tweaks.wayland),
        );
        rows.extend(
            self.hint(
                "Skips XWayland. Newer and less travelled than the X11 path, so it is off \
                 until asked for.",
            )
            .map(Element::from),
        );
        if tweaks.wayland {
            rows.push(self.tweak_check(scope, "Ask for HDR output", Tweak::Hdr, tweaks.hdr));
            rows.push(
                row![
                    muted("Monitor".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
                    clickable(
                        "monitor",
                        text_input(
                            "whichever Proton picks",
                            match scope {
                                Scope::Product => &self.monitor_input,
                                Scope::BattleNet => &self.bnet_monitor_input,
                            },
                        )
                        .on_input(move |v| Message::TweakWaylandMonitor(scope, v))
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fill),
                    ),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center)
                .into(),
            );
            rows.extend(
                self.hint("A connector name, as in DP-1 or HDMI-A-1.").map(Element::from),
            );
        }
        rows
    }

    /// The escape hatch, full width and folded away.
    ///
    /// Out of the grid and collapsed by default because it is the only thing
    /// on the page with no bound on its height: a profile with a dozen
    /// variables in it would make its column twice its neighbour's, which is
    /// the exact problem the grid is here to fix.
    fn env_card(&self, scope: Scope, tweaks: &Tweaks) -> Element<'_, Message> {
        let count = tweaks.env.len();
        let open = match scope {
            Scope::Product => self.env_open,
            Scope::BattleNet => self.bnet_env_open,
        };
        let summary = row![tappable(
            "environment",
            Some(Message::TweakEnvToggle(scope)),
            button(
                text(match open {
                    true => "Hide",
                    false => "Show",
                })
                .size(theme::LABEL)
            )
            .style(theme::ghost)
            .padding(theme::tap([theme::XS, theme::SM])),
        )];
        let mut head = row![text("Environment variables").size(theme::HEADING)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        if count > 0 {
            head = head.push(pill(count.to_string(), theme::ACCENT));
        }
        let mut inner = column![row![head.width(Length::Fill), summary]
            .spacing(theme::SM)
            .align_y(Alignment::Center)]
        .spacing(theme::SM);

        if open {
            for (key, value) in &tweaks.env {
                inner = inner.push(
                    container(
                        row![
                            mono(format!("{key}={value}")).width(Length::Fill),
                            tappable(
                                format!("remove variable {key}"),
                                Some(Message::TweakEnvRemove(scope, key.clone())),
                                button(text("Remove").size(theme::LABEL))
                                    .style(theme::ghost)
                                    .padding(theme::tap([theme::XS, theme::SM])),
                            ),
                        ]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center),
                    )
                    .padding([theme::XS, theme::SM])
                    .style(theme::row),
                );
            }
            inner = inner.push(
                row![
                    clickable(
                        "variable",
                        text_input("VARIABLE", &self.env_key_input)
                            .on_input(Message::TweakEnvKey)
                            .on_submit(Message::TweakEnvAdd(scope))
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM)
                            .width(Length::Fixed(220.0)),
                    ),
                    clickable(
                        "value",
                        text_input("value", &self.env_value_input)
                            .on_input(Message::TweakEnvValue)
                            .on_submit(Message::TweakEnvAdd(scope))
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM)
                            .width(Length::Fill),
                    ),
                    secondary("Add", Message::TweakEnvAdd(scope)).enabled(!self.env_key_input.is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
            inner = inner.extend(
                self.hint(
                    "Set for this product's launch only, after everything Lightforge \
                     assembles — so a variable here overrides the launcher's own.",
                )
                .map(Element::from),
            );
        }

        container(inner).padding(theme::MD).width(Length::Fill).style(theme::card).into()
    }

    /// The cog menu: what you do *about* a game rather than to play it.
    ///
    /// Laid out above the action panel rather than stacked over the page,
    /// because iced 0.13 has no popover and a floating layer would need the
    /// panel's height as a magic number to sit clear of it. Taking its room
    /// from the spacer above the panel puts it in the right place by
    /// construction, at any window size.
    fn gear_menu(&self) -> Element<'_, Message> {
        let item = |label: &'static str, message: Message, enabled: bool| {
            self.ring(
                focus::Target::Menu(label),
                enabled.then(|| message.clone()),
                button(text(label).size(theme::LABEL))
                    .style(theme::ghost)
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .width(Length::Fill)
                    .on_press_maybe(enabled.then_some(message)),
            )
        };

        let installed = self.local.version.is_some();
        // Offered only when the handler would go ahead: nothing else writing
        // to the store, which every flavour shares, and no launch under way.
        let store_free = self.store_writer().is_none() && self.deep_cancel.is_none();
        let can_scan = installed && !self.busy && store_free;
        let can_deep_scan = installed && !self.busy && !self.scanning && self.maintenance().is_none();
        let can_uninstall = installed && !self.busy && store_free;
        // Anything in the queue for this product can be given up, running,
        // paused or still waiting its turn. Pause is on the button itself;
        // this is the one that ends it.
        let queued = self.queue.iter().any(|d| d.code == self.selected.code);
        let menu = container(
            column![
                item("Game Settings", Message::OpenGameSettings, installed),
                item("Check for Updates", Message::CheckForUpdates, !self.busy),
                item("Scan and Repair", Message::ScanAndRepair, can_scan),
                match self.deep_cancel.is_some() {
                    true => item("Stop Deep Scan", Message::DeepScan, true),
                    false => item("Deep Scan", Message::DeepScan, can_deep_scan),
                },
                item("Open Files in Explorer", Message::OpenFiles, installed),
                item("Copy Debug Info", Message::CopyDebug, true),
                // Last and apart from the rest: the one entry that destroys.
                item("Uninstall", Message::AskUninstall, can_uninstall),
            ]
            .extend(
                self.owed_repair
                    .is_some()
                    .then(|| item("Repair Damaged Files", Message::RepairDamaged, !self.busy && store_free)))
            .extend(queued.then(|| {
                item(
                    "Cancel Download",
                    Message::AskCancelDownload(self.selected.code.clone()),
                    true,
                )
            }))
            .spacing(theme::XS),
        )
        .padding(theme::XS)
        // The width of the split button beneath it, so the menu reads as
        // belonging to that control rather than floating near it.
        .width(Length::Fixed(240.0))
        .style(theme::floating);

        focus::layer(mouse_area(menu).on_press(Message::MenuClick))
    }

    /// One headline as a card that is entirely a link.
    ///
    /// The whole card presses rather than a "read more" inside it: there is
    /// one thing to do with a headline, and a card with a single small target
    /// in it makes the reader hunt for what the card already is.
    fn news_card<'a>(&self, item: &'a news::Item) -> Element<'a, Message> {
        let mut byline = row![muted(item.source.clone(), theme::LABEL)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        if let Some(category) = item.category.as_deref().filter(|c| !c.is_empty()) {
            byline = byline
                .push(muted("·".to_owned(), theme::LABEL))
                .push(muted(category.to_owned(), theme::LABEL));
        }
        byline = byline
            .push(Space::new().width(Length::Fill))
            .push(muted(short_day(&item.published), theme::LABEL));

        let mut words = column![byline, text(item.title.clone()).size(theme::BODY).font(theme::BOLD)]
            .spacing(theme::XS)
            .width(Length::Fill);
        if !item.summary.is_empty() {
            words = words.push(muted(item.summary.clone(), theme::LABEL));
        }

        // The box is drawn whether or not the picture has arrived, so a card
        // does not resize under the reader when it does. An article with no
        // picture at all gets no box: that is a fact about the article, not a
        // load still in flight.
        let card: Element<'_, Message> = match item.image.as_deref() {
            Some(url) if !url.is_empty() => {
                let slot: Element<'_, Message> = match self.news_images.get(url) {
                    Some(handle) => image(handle.clone())
                        .width(Length::Fixed(theme::NEWS_THUMB_WIDTH))
                        .height(Length::Fixed(theme::NEWS_THUMB_HEIGHT))
                        // Cover, not Contain: the box is one shape and these
                        // pictures are not, so the alternative is letterboxing
                        // every card differently.
                        .content_fit(ContentFit::Cover)
                        .into(),
                    None => container(Space::new())
                        .width(Length::Fixed(theme::NEWS_THUMB_WIDTH))
                        .height(Length::Fixed(theme::NEWS_THUMB_HEIGHT))
                        .style(theme::row)
                        .into(),
                };
                row![slot, words].spacing(theme::MD).align_y(Alignment::Center).into()
            }
            _ => words.into(),
        };

        let open = Message::OpenArticle(item.url.clone());
        focus::ring(
            focus::Target::Key(format!("article {}", item.url)),
            focus::Press::Send(open.clone()),
            button(card).padding(theme::MD).width(Length::Fill).style(theme::card_button).on_press(open),
        )
    }

    /// News, headlines from the feeds the proxy merges.
    ///
    /// Every state of the fetch says which one it is. An empty page that
    /// looks broken is worse than one that explains itself, which was true of
    /// the placeholder this replaced and is still the rule here.
    fn news_screen(&self) -> Element<'_, Message> {
        fn centred<'a>(lines: Vec<Element<'a, Message>>) -> Element<'a, Message> {
            container(column(lines).spacing(theme::SM).align_x(Alignment::Center))
                .center(Length::Fill)
                .into()
        }

        let content: Element<'_, Message> = match &self.news {
            Some(headlines) if !headlines.feed.items.is_empty() => {
                let mut list = column![].spacing(theme::SM);

                // Said once, above the headlines, rather than on each card:
                // it is a fact about the page, not about any one article.
                if !headlines.feed.degraded.is_empty() {
                    list = list.push(muted(
                        format!(
                            "{} could not be reached, so some headlines may be missing.",
                            headlines.feed.degraded.join(" and "),
                        ),
                        theme::LABEL,
                    ));
                }
                if headlines.stale {
                    list = list.push(muted(
                        "Showing the last headlines Lightforge fetched — the feed could not be \
                         reached just now."
                            .to_owned(),
                        theme::LABEL,
                    ));
                }

                for item in &headlines.feed.items {
                    list = list.push(self.news_card(item));
                }
                list.into()
            }
            // Asked, answered, and there was nothing in it.
            Some(_) => centred(vec![muted("No headlines right now.".to_owned(), theme::BODY).into()]),
            None if self.news_pending => {
                centred(vec![muted("Fetching headlines…".to_owned(), theme::BODY).into()])
            }
            None if self.news_asked => centred(vec![
                text("No headlines").size(theme::TITLE).into(),
                muted(
                    "The news service could not be reached, and nothing is cached yet.".to_owned(),
                    theme::BODY,
                )
                .into(),
            ]),
            // Before `dirs` exists there is nowhere to cache and nothing has
            // been asked for, the setup screen is what the user is looking at.
            None => centred(vec![muted("Nothing to show yet.".to_owned(), theme::BODY).into()]),
        };

        // Wowhead's own red, and only here: the headlines are theirs, and the
        // button that goes to them is the one place in the app where another
        // brand's colour means something.
        let mut aside = row![action_button("Wowhead", theme::WOWHEAD, Message::OpenLink(WOWHEAD_URL))
            .enabled(true)]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
        if !self.news_pending {
            aside = aside.push(secondary("Refresh", Message::RefreshNews).enabled(true));
        }

        let page = self.page(
            "News",
            Some("Headlines from Wowhead".to_owned()),
            Some(aside.into()),
            content,
        );

        let Some(backdrop) = self.news_bg.clone() else {
            return page;
        };

        // Same shape as the dashboard: art underneath, a scrim between, the
        // header inside the stack rather than above it, hoisting it out
        // would push the picture below the navigation row.
        //
        // The scrim holds dark across the top and settles lighter below, the
        // same shape the dashboard arrived at and for the same reason: the
        // navigation row hangs straight over the picture with no backdrop of
        // its own, and this artwork is pale sky exactly where that row lands.
        // A single flat value cannot serve both ends, strong enough for the
        // nav row buries the art, weak enough to show the art leaves loose
        // white text on a bright sky.
        //
        // Offsets run bottom (0.0) to top (1.0). The top pair holds rather
        // than ramps so the whole of the header and the page title sit on
        // even cover instead of in the middle of a fade; the body settles to
        // [`NEWS_SCRIM`], where the cards are translucent and the picture
        // reads through the gaps between them.
        stack![
            image(backdrop)
                .content_fit(ContentFit::Cover)
                .width(Length::Fill)
                .height(Length::Fill),
            container(Space::new().height(Length::Fill))
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                        iced::gradient::Linear::new(0.0)
                            .add_stop(0.0, iced::Color { a: NEWS_SCRIM, ..theme::BG })
                            .add_stop(0.76, iced::Color { a: NEWS_SCRIM, ..theme::BG })
                            .add_stop(0.86, iced::Color { a: NEWS_SCRIM_TOP, ..theme::BG })
                            .add_stop(1.0, iced::Color { a: NEWS_SCRIM_TOP, ..theme::BG }),
                    ))),
                    ..container::Style::default()
                }),
            page,
        ]
        .into()
    }

    /// Lightforge's own release notes. Nothing here yet either.
    fn updates_screen(&self) -> Element<'_, Message> {
        let version = env!("CARGO_PKG_VERSION");
        let releases = lightforge_core::releases::all();

        let mut body = column![].spacing(theme::LG);

        if releases.is_empty() {
            // Only reachable in a build whose notes file was emptied; a test
            // in `core::releases` keeps the shipped one honest.
            body = body.push(muted("No release notes in this build.".to_owned(), theme::BODY));
        }

        for release in &releases {
            let heading = match release.version == version {
                true => format!("{} — this version", release.version),
                false => release.version.clone(),
            };
            body = body.push(
                container(
                    column![text(heading).size(theme::TITLE), release_notes(release)]
                        .spacing(theme::MD),
                )
                .padding(theme::LG)
                .width(Length::Fill)
                .style(theme::row),
            );
        }

        // The notes stay readable here whatever the popup has been told, which
        // is what makes "never show this again" a safe thing to tick.
        body = body.extend(
            self.hint(
                "These are the notes the launcher shows after an update. Settings › General \
                 decides whether that popup appears.",
            )
            .map(Element::from),
        );

        // Through `page` like every other page, which is what gives it the
        // same margins and title; built on its own it sat flush against the
        // window's edges.
        self.page(
            "Lightforge Updates",
            Some(format!("You are running {version}.")),
            None,
            body.into(),
        )
    }

    /// The splash: the wordmark, what is being fetched, and a bar that says
    /// something is happening without pretending to know for how long.
    fn splash_screen(&self, splash: &Splash) -> Element<'_, Message> {
        const MARK: f32 = 40.0;
        const SPIN: f32 = 30.0;
        let note = match splash.products {
            // No tracked products at all is a machine that has never been set
            // up, which is a wizard away rather than an artwork fetch.
            0 => "Getting ready…".to_owned(),
            1 => "Fetching artwork for 1 game…".to_owned(),
            n => format!("Fetching artwork for {n} games…"),
        };
        // The card, at [`SPLASH`]'s size. `theme::menu` rather than
        // `theme::floating`: floating is 12% transparent, which over a
        // transparent window means the desktop showing through the splash.
        let card = container(
            column![
                image(wordmark_image())
                    .width(Length::Fixed(MARK * WORDMARK_ASPECT))
                    .height(Length::Fixed(MARK)),
                muted(note, theme::CAPTION),
                canvas(Spinner).width(Length::Fixed(SPIN)).height(Length::Fixed(SPIN)),
            ]
            .spacing(theme::MD)
            .align_x(Alignment::Center),
        )
        .center(Length::Fixed(SPLASH.width))
        .height(Length::Fixed(SPLASH.height))
        .style(theme::menu);

        // Centred on a window the size of the launcher, with **no background
        // of its own**: the window was created `transparent`, so everything
        // around the card is see-through and what anyone sees is a small
        // panel in the middle of their screen. Painting a backdrop here
        // instead would show a full-size empty launcher, which is the thing
        // the splash exists to avoid.
        container(card).center(Length::Fill).into()
    }

    /// Addon management: what is installed, and what could be.
    fn addons_screen(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, which: AddonTab| {
            let entry = button(text(label).size(theme::LABEL)).padding(theme::tap([theme::SM, theme::MD]));
            let entry = match self.addon_tab == which {
                true => entry.style(theme::selected),
                false => entry.style(theme::ghost).on_press(Message::AddonTab(which)),
            };
            // The open tab takes the ring too, so the row of tabs can be
            // walked without a gap where the current one is.
            focus::ring(
                focus::Target::Key(format!("addon tab {label}")),
                focus::Press::Send(Message::AddonTab(which)),
                entry,
            )
        };

        let mut body = column![row![
            tab("My Addons", AddonTab::Mine),
            tab("Browse Addons", AddonTab::Browse),
            tab("Import", AddonTab::Import),
            tab("Export", AddonTab::Export),
        ]
        .spacing(theme::XS)]
        .spacing(theme::MD);

        if self.addons_busy {
            let progress = &self.addon_sample;
            body = body.push(
                row![
                    text(progress.line.clone()).size(theme::LABEL),
                    mono(format!("{}/{}", progress.done, progress.total)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }
        if let Some(e) = &self.error {
            body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }

        body = body.push(match self.addon_tab {
            AddonTab::Mine => self.addons_mine(),
            AddonTab::Browse => self.addons_browse(),
            AddonTab::Import => self.addons_import(),
            AddonTab::Export => self.addons_export(),
        });

        // Parent folders only, so the count matches the rows you can see,
        // BigWigs is one addon here and eleven folders on disk.
        let installed = self.addons.rows.iter().filter(|r| r.child_of.is_none()).count();
        let aside = muted(
            match installed {
                1 => "1 addon installed".to_owned(),
                n => format!("{n} addons installed"),
            },
            theme::LABEL,
        );

        self.page("Addons", None, Some(aside.into()), body.into())
    }

    fn addons_mine(&self) -> Element<'_, Message> {
        let outdated = self.updates.iter().filter(|u| u.has_update()).count();

        let toolbar = container(
            row![
                // Sized, not filling: a filter box the width of the window
                // looks like the page's search when it only narrows the list
                // below it.
                clickable(
                    "addon filter",
                    text_input("Filter installed addons…", &self.addon_filter)
                        .on_input(Message::AddonFilterChanged)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(260.0)),
                ),
                Space::new().width(Length::Fill),
                muted(
                    match (self.updates.is_empty(), outdated) {
                        (true, _) => String::new(),
                        (false, 0) => "everything is up to date".to_owned(),
                        (false, n) => format!("{n} have updates"),
                    },
                    theme::LABEL,
                ),
                // One shape for the three, so the toolbar reads as a set.
                // Colour still carries state: blue is the one to press, grey
                // is a thing you can do.
                action_button("Check for updates", theme::PAUSE, Message::CheckAddonUpdates)
                    .enabled(!self.addons_locked() && self.addons.managed > 0),
                action_button("Update all", theme::ACCENT, Message::UpdateAddons)
                    .enabled(!self.addons_locked() && self.addons.managed > 0),
                action_button("Open folder", theme::PAUSE, Message::OpenAddonsDir)
                    .enabled(true),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .padding(theme::MD)
        .style(theme::card);

        // Offered only when there is something to offer it for. A button
        // that does nothing on every press teaches people to ignore it, and
        // a library installed entirely through Lightforge has nothing to
        // recognise.
        let unmanaged = self
            .addons
            .rows
            .iter()
            .filter(|row| row.managed.is_none() && row.child_of.is_none())
            .count();

        let mut body = column![toolbar].spacing(theme::SM);

        if unmanaged > 0 {
            body = body.push(
                container(
                    row![
                        column![
                            text(match unmanaged {
                                1 => "1 addon was installed by hand".to_owned(),
                                n => format!("{n} addons were installed by hand"),
                            })
                            .size(theme::BODY),
                            muted(
                                "Lightforge can recognise them by their contents and \
                                 manage them from here — updates, channels and all."
                                    .to_owned(),
                                theme::CAPTION,
                            ),
                        ]
                        .spacing(theme::XS)
                        .width(Length::Fill),
                        action_button("Recognise them", theme::ACCENT, Message::AdoptAddons)
                            .enabled(!self.addons_locked()),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )
                .padding(theme::MD)
                .style(theme::card),
            );
        }

        // A header, because four right-hand columns of bare dates are not
        // self-describing. Indented and spaced to match a row's own columns.
        if !self.addons.rows.is_empty() {
            body = body.push(
                container(
                    row![
                        muted("NAME".to_owned(), theme::CAPTION).width(Length::Fill),
                        column_head("LATEST"),
                        column_head("RELEASED"),
                        column_head("INSTALLED"),
                        // Standing in for the controls each row carries, so
                        // the three headings sit over their own columns
                        // rather than over the buttons.
                        Space::new().width(Length::Fixed(ADDON_ACTIONS)),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )
                .padding([0.0, theme::MD]),
            );
        }

        // What happened without anyone asking, said where the addons are.
        // Nothing interrupted the user to tell them, so this is the only
        // place it is said at all.
        if let Some(note) = &self.addon_auto_note {
            body = body.push(muted(note.clone(), theme::CAPTION));
        }

        if let Some(report) =
            self.addon_report.as_ref().filter(|_| self.addon_report_tab == AddonTab::Mine)
        {
            for item in &report.unavailable {
                let mut line = row![column![
                    text(item.name.clone()).size(theme::BODY),
                    muted(item.reason.clone(), theme::CAPTION),
                ]
                .spacing(theme::XS)]
                .spacing(theme::SM)
                .align_y(Alignment::Center);

                line = line.push(Space::new().width(Length::Fill));
                if let Some(url) = &item.website_url {
                    line = line.push(
                        secondary("Open page", Message::OpenUrl(url.clone())).enabled(true),
                    );
                }
                body = body.push(container(line).padding(theme::MD).style(theme::row));
            }
        }

        if self.addons.rows.is_empty() {
            body = body.push(empty(
                "No addons installed.",
                "Find one under Browse Addons, or bring your collection over under Import.",
            ));
        }

        // Ordered and filtered before drawing, rather than in the order the
        // folder scan happened to return. Outdated first. That is the reason
        // to open this page, then by name, so a list of forty is something
        // you can read down instead of search through.
        let needle = self.addon_filter.trim().to_lowercase();
        let mut shown: Vec<&_> = self
            .addons
            .rows
            .iter()
            // A package's other folders belong to the addon above them;
            // showing BigWigs' eleven folders as eleven addons would be a lie.
            .filter(|addon| addon.child_of.is_none())
            .filter(|addon| {
                needle.is_empty()
                    || addon.title.to_lowercase().contains(&needle)
                    || addon.folder.to_lowercase().contains(&needle)
            })
            .collect();
        shown.sort_by_cached_key(|addon| {
            let outdated = self.update_status(addon).is_some_and(|s| s.has_update());
            (!outdated, addon.title.to_lowercase())
        });

        if !needle.is_empty() && shown.is_empty() && !self.addons.rows.is_empty() {
            body = body.push(empty(
                &format!("No installed addon matches “{}”.", self.addon_filter.trim()),
                "Clear the filter to see everything again.",
            ));
        }

        for addon in shown {
            let extra = self
                .addons
                .rows
                .iter()
                .filter(|r| r.child_of.as_deref() == Some(addon.folder.as_str()))
                .count();
            let status = self.update_status(addon);

            let mut heading = row![text(addon.title.clone()).size(theme::BODY), mono(addon.version.clone())]
                .spacing(theme::SM)
                .align_y(Alignment::Center);
            match addon.managed.as_ref() {
                // Said once, in the place an update would be announced: this
                // row is why "Update all" leaves something behind.
                Some(m) if m.ignored => {
                    heading = heading.push(pill("ignored".to_owned(), theme::MUTED));
                }
                // The version it is held at is already beside the title, so
                // the pill says the state and not the number twice.
                Some(m) if m.pinned.is_some() => {
                    heading = heading.push(pill("pinned".to_owned(), theme::MUTED));
                }
                // Only worth a word when it differs from what everything else
                // on the page is doing; with the default on, that is the
                // addon somebody decided to keep still.
                Some(m) if !m.auto_update.unwrap_or(self.config.addon_auto_update) => {
                    heading = heading.push(pill("manual".to_owned(), theme::MUTED));
                }
                _ => {}
            }
            if let Some(status) = status.filter(|s| s.has_update()) {
                // What is on offer, named the way the provider names it, a
                // version people recognise, not the opaque file id the
                // ledger trades in. A channel switch says so instead, because
                // "→ 1.4.26" beside an alpha looks like going forwards.
                heading = heading.push(match status.is_channel_switch() {
                    true => pill(
                        format!(
                            "{} build",
                            status
                                .available_build
                                .map(|c| c.as_str())
                                .unwrap_or("different"),
                        ),
                        theme::ACCENT,
                    ),
                    false => pill(
                        format!(
                            "→ {}",
                            status
                                .available_name
                                .clone()
                                .or_else(|| status.available.clone())
                                .unwrap_or_default()
                        ),
                        theme::UPDATE,
                    ),
                });
            }

            let mut line = row![column![
                heading,
                muted(
                    match (&addon.managed, extra) {
                        (Some(m), 0) => format!("{} · managed", m.provider),
                        (Some(m), n) => format!("{} · managed · +{n} folders", m.provider),
                        (None, _) => "installed by hand".to_owned(),
                    },
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS)
            .width(Length::Fill)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            // The three facts, in the columns the header names. A dash
            // rather than a blank for a fact we do not have: an empty cell
            // reads as a layout bug, where ", " reads as "not known", which
            // is what it is for an addon installed before this was recorded
            // or a provider that publishes no dates.
            let managed_row = addon.managed.as_ref();
            line = line.push(cell(
                status.and_then(|s| s.available_name.clone()).or_else(|| {
                    // Up to date: what is latest is what is installed, and
                    // saying so beats an empty column.
                    managed_row.map(|_| addon.version.clone())
                }),
            ));
            line = line.push(cell(
                status.and_then(|s| s.available_date.as_deref().map(short_day)),
            ));
            line = line.push(cell(
                managed_row.and_then(|m| m.installed_at.as_deref().map(short_day)),
            ));
            // Everything that acts on the row lives in a box of its own
            // width, right-aligned inside it. Without it the three columns
            // float: a row with an Update button pushes them left of a row
            // without one, and both sit left of the header that is supposed
            // to name them. The box is pushed for an addon installed by hand
            // too, which has no controls at all, otherwise its columns would
            // be the ones out of step.
            let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);
            if let Some(managed) = &addon.managed {
                // Which builds this addon accepts. Narrow and first, so the
                // buttons that act stay where the eye expects them whether or
                // not there is an update to show.
                // Sized like the buttons it sits beside: three controls on
                // one line that disagree about their height read as a
                // mistake rather than as a hierarchy. The width is fixed so
                // the buttons do not shift as the word changes between
                // Default, Stable, Beta and Alpha.
                actions = actions.push(self.picker(
                    format!("channel {} {}", managed.provider, managed.id),
                    RowChannel::ALL.to_vec(),
                    Some(RowChannel(managed.channel)),
                    "",
                    |choice| Message::AddonChannelFor(managed.provider.clone(), managed.id.clone(), choice),
                    Length::Fixed(120.0),
                    theme::LABEL,
                ));

                // One addon at a time. `install_one` fetches the newest file
                // and does not pin it, which is exactly what updating is, so
                // this is the same path "Update all" takes for every row.
                //
                // A pin is a statement about which version this addon is on,
                // so the row stops offering to move it, unlike Ignore, which
                // only means "not in bulk". Unpinning brings the button back.
                let pinned = managed.pinned.is_some();
                if let Some(status) = status.filter(|s| s.has_update() && !pinned) {
                    // "Update" would be a lie when the channel changed: going
                    // back to stable from an alpha installs an *older* build,
                    // which is right, but not what the word promises.
                    let label = match status.is_channel_switch() {
                        true => "Switch",
                        false => "Update",
                    };
                    actions = actions.push(
                        action_button(
                            label,
                            theme::UPDATE,
                            Message::InstallAddon(
                                managed.provider.clone(),
                                managed.id.clone(),
                            ),
                        )
                        .enabled(!self.addons_locked()),
                    );
                }

                // Two clicks, because removing deletes the addon's folders off
                // disk and there is no undo. The second button is the way out,
                // so the destructive one is never the only thing to press.
                let armed = self.addon_remove_armed.as_ref().is_some_and(|(p, id)| {
                    p == &managed.provider && id == &managed.id
                });
                // Remove and the Delete it turns into are one place for the
                // ring, which stays on it while the question is asked.
                let remove = format!("addon remove {} {}", managed.provider, managed.id);
                actions = match armed {
                    true => actions
                        .push(
                            action_button(
                                "Delete",
                                theme::ERROR,
                                Message::RemoveAddon(
                                    managed.provider.clone(),
                                    managed.id.clone(),
                                ),
                            )
                            .keyed(remove, !self.addons_locked()),
                        )
                        .push(
                            secondary("Keep", Message::CancelRemoveAddon).enabled(true),
                        ),
                    false => actions.push(
                        secondary(
                            "Remove",
                            Message::ArmRemoveAddon(
                                managed.provider.clone(),
                                managed.id.clone(),
                            ),
                        )
                        .keyed(remove, !self.addons_locked()),
                    ),
                };

                // Everything that is a setting rather than an action lives
                // behind this, so the row keeps its width as the settings
                // grow. Only one menu is open at a time, and the panel hangs
                // off the button itself rather than the row's corner.
                let key = (managed.provider.clone(), managed.id.clone());
                let open = self.addon_menu_open.as_ref() == Some(&key);
                let toggle = Message::AddonMenu(match open {
                    true => None,
                    false => Some(key.clone()),
                });
                actions = actions.push(popover::popover(
                    focus::ring(
                        focus::Target::Key(format!("addon menu {} {}", managed.provider, managed.id)),
                        focus::Press::Send(toggle.clone()),
                        button(
                            image(ellipsis_icon())
                                .width(Length::Fixed(MENU_GLYPH))
                                .height(Length::Fixed(MENU_GLYPH)),
                        )
                        .padding(theme::SM)
                        .style(theme::ghost)
                        .on_press(toggle),
                    ),
                    open.then(|| self.addon_row_menu(managed, &addon.version, &addon.title)),
                ));
            }
            line = line.push(
                container(actions)
                    .width(Length::Fixed(ADDON_ACTIONS))
                    .align_x(Alignment::End),
            );

            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }

        // Where all of this lands, kept quiet: "Open folder" in the toolbar is
        // the way there, and this is for the times you need the path itself.
        body = body.push(muted(self.addons.dir.display().to_string(), theme::CAPTION));
        body.into()
    }

    /// What the last update check said about one installed addon, if it is
    /// managed and the check covered it.
    fn update_status(
        &self,
        addon: &addonwork::Row,
    ) -> Option<&lightforge_core::addons::manage::UpdateStatus> {
        let managed = addon.managed.as_ref()?;
        self.updates
            .iter()
            .find(|u| u.id == managed.id && u.provider == managed.provider)
    }

    /// Bringing a collection over from WoWUp.
    ///
    /// Its own tab rather than a field above the list: it is something you do
    /// once, when you arrive, and it had the most prominent box on a page
    /// whose job is the addons you already have.
    fn addons_import(&self) -> Element<'_, Message> {
        let box_ = container(
            column![
                text("Import from WoWUp").size(theme::HEADING),
                muted(
                    "In WoWUp, export your addons and paste the token here. Everything it \
                     names is fetched from its provider and tracked by Lightforge from then \
                     on."
                        .to_owned(),
                    theme::LABEL,
                ),
                row![
                    clickable(
                        "import token",
                        text_input("Paste a WoWUp import token…", &self.token_input)
                            .on_input(Message::TokenChanged)
                            .on_submit(Message::ImportToken)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                    ),
                    action_button("Import", theme::ACCENT, Message::ImportToken)
                        .enabled(!self.addons_locked() && !self.token_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                // A WoWUp token names a file for every addon. Taking them is
                // reproducing someone's setup exactly, which is a pin and is
                // recorded as one; leaving them is taking their list of
                // addons and tracking each from there.
                clickable(
                    "import pinned",
                    tick(self.import_pinned)
                        .label("Install the exact versions this token names, and pin them")
                        .on_toggle(Message::ImportPinned)
                        .text_size(theme::tap_text(theme::BODY)),
                ),
            ]
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![box_].spacing(theme::SM);

        // What an import could not resolve belongs with the import that could
        // not resolve it, not scattered through the installed list.
        if let Some(report) =
            self.addon_report.as_ref().filter(|_| self.addon_report_tab == AddonTab::Import)
        {
            for item in &report.unavailable {
                let mut line = row![column![
                    text(item.name.clone()).size(theme::BODY),
                    muted(item.reason.clone(), theme::CAPTION),
                ]
                .spacing(theme::XS)]
                .spacing(theme::SM)
                .align_y(Alignment::Center);

                line = line.push(Space::new().width(Length::Fill));
                if let Some(url) = &item.website_url {
                    line = line.push(
                        secondary("Open page", Message::OpenUrl(url.clone())).enabled(true),
                    );
                }
                body = body.push(container(line).padding(theme::MD).style(theme::row));
            }
        }

        body.into()
    }

    /// Handing this collection to somewhere else.
    ///
    /// Beside Import rather than inside it: the two are opposite directions of
    /// the same idea, and a tab that does both is a tab you have to read before
    /// you can use.
    fn addons_export(&self) -> Element<'_, Message> {
        let managed = self.addons.managed;
        let box_ = container(
            column![
                row![
                    column![
                        text("Export").size(theme::HEADING),
                        muted(
                            match managed {
                                0 => "Nothing to export yet — only addons Lightforge \
                                      installed can be named in a token."
                                    .to_owned(),
                                1 => "A token naming the 1 addon Lightforge manages here, \
                                      to import on another machine or in WoWUp."
                                    .to_owned(),
                                n => format!(
                                    "A token naming the {n} addons Lightforge manages here, \
                                     to import on another machine or in WoWUp."
                                ),
                            },
                            theme::LABEL,
                        ),
                    ]
                    .spacing(theme::XS)
                    .width(Length::Fill),
                    action_button("Export", theme::ACCENT, Message::ExportAddons)
                        .enabled(!self.addons_locked() && managed > 0),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            ]
            .extend(match self.export_token.is_empty() {
                true => Vec::new(),
                false => vec![Element::from(
                    row![
                        // Editable so it can be selected and copied by hand;
                        // what is typed into it is never read back.
                        clickable(
                            "export token",
                            text_input("", &self.export_token)
                                .on_input(Message::ExportEdited)
                                .style(theme::field)
                                .size(theme::BODY)
                                .padding(theme::SM),
                        ),
                        secondary("Copy", Message::CopyExport).enabled(true),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )],
            })
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        column![box_].spacing(theme::SM).into()
    }

    fn addons_browse(&self) -> Element<'_, Message> {
        let search = container(
            column![
                row![
                    clickable(
                        "addon search",
                        text_input("Search addons, or paste a project page URL…", &self.search_input)
                            .on_input(Message::SearchChanged)
                            .on_submit(Message::RunSearch)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                    ),
                    action_button("Search", theme::ACCENT, Message::RunSearch)
                        .enabled(!self.searching && !self.search_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ]
            .extend(self.hint(
                "WoWInterface, Tukui and GitHub (type owner/repo). CurseForge needs a \
                 catalog in Settings. A pasted CurseForge, GitHub, WoWInterface or Tukui \
                 page is looked up at that provider — it installs the same way as a \
                 search hit, so a dead link says so rather than failing oddly.",
            ).map(Element::from))
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![search].spacing(theme::SM);

        // A provider that could not answer is said out loud: an empty list
        // otherwise reads as "nothing matched".
        for problem in &self.search.problems {
            body = body.push(text(format!("⚠ {problem}")).size(theme::CAPTION).color(theme::UPDATE));
        }

        if self.searching {
            body = body.push(muted("searching…".to_owned(), theme::BODY));
        } else if self.search.listings.is_empty() {
            body = body.push(match self.search.query.is_empty() {
                true => empty(
                    "Search for an addon.",
                    "The most-downloaded addons show here once the catalogue has been \
                     fetched. Results come from WoWInterface and Tukui; type owner/repo \
                     for GitHub, or paste an addon's page URL.",
                ),
                false => empty(
                    &format!("Nothing found for “{}”.", self.search.query),
                    "It may be a CurseForge addon, which needs a catalog configured.",
                ),
            });
        }

        // Said once, above the rows: a list nobody asked for has to explain
        // what it is, or it reads as the results of a search that went wrong.
        if self.search.query.is_empty() && !self.search.listings.is_empty() {
            body = body.push(muted("Most downloaded".to_owned(), theme::LABEL));
        }

        let installed_ids: Vec<(&str, &str)> = self
            .addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .map(|m| (m.provider.as_str(), m.id.as_str()))
            .collect();

        for listing in &self.search.listings {
            let provider = listing.provider.as_str().to_owned();
            let already = installed_ids
                .iter()
                .any(|(p, id)| *p == provider && *id == listing.id);

            let mut line = row![column![
                row![
                    text(listing.name.clone()).size(theme::BODY),
                    pill(provider.clone(), theme::ACCENT),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                muted(
                    format!(
                        "{}{}",
                        listing.author.as_deref().unwrap_or("unknown author"),
                        match listing.downloads {
                            0 => String::new(),
                            n => format!(" · {} downloads", downloads_of(n)),
                        }
                    ),
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            line = line.push(Space::new().width(Length::Fill));
            // "Details" where there is something to show and "Page" where
            // there is not: only CurseForge publishes a description and a
            // changelog, and the detail card offers the page anyway, so
            // carrying both buttons would just crowd the row.
            match provider.eq_ignore_ascii_case("curse") {
                true => {
                    let open = Message::ShowAddonDetail(
                        provider.clone(),
                        listing.id.clone(),
                        listing.name.clone(),
                    );
                    line = line.push(secondary("Details", open).enabled(true));
                }
                false => {
                    if let Some(url) = &listing.website_url {
                        line = line
                            .push(secondary("Page", Message::OpenUrl(url.clone())).enabled(true));
                    }
                }
            }
            line = line.push(match already {
                true => tag("Installed".to_owned(), theme::OK),
                false => action_button(
                    "Install",
                    theme::INSTALL,
                    Message::InstallAddon(provider.clone(), listing.id.clone()),
                )
                .enabled(!self.addons_locked()),
            });
            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }
        body.into()
    }

    /// The 32-bit runtime, when it is missing.
    ///
    /// Everything Lightforge does to build a prefix is 64-bit; Battle.net's
    /// installer is the first 32-bit binary in the sequence, so a machine
    /// without a 32-bit userspace looks perfectly healthy until the very last
    /// step and then dies in under a second. Saying so here, before anything
    /// is downloaded, is the whole point.
    ///
    /// The command is offered to copy rather than run. Installing system
    /// packages needs root, and a launcher invoking a package manager as root
    /// on a guess about someone's distribution is a trust escalation worth
    /// avoiding, the more so because the systems that most need help
    /// (read-only SteamOS, rpm-ostree, NixOS) are exactly the ones no such
    /// button could serve.
    fn runtime_row() -> Element<'static, Message> {
        let remedy = lightforge_core::runtime::remedy();

        let mut lines = column![
            text("32-bit runtime").size(theme::BODY),
            mono("missing — Battle.net's installer cannot run without it".to_owned()),
        ]
        .spacing(theme::XS);

        if let Some(command) = &remedy.command {
            lines = lines.push(muted(
                format!("on {}, run:", remedy.distro),
                theme::CAPTION,
            ));
            lines = lines.push(mono(command.clone()));
        }
        if let Some(note) = &remedy.note {
            lines = lines.push(muted(note.clone(), theme::CAPTION));
        }
        if remedy.command.is_none() && remedy.note.is_none() {
            lines = lines.push(muted(
                lightforge_core::runtime::ADVICE.to_owned(),
                theme::CAPTION,
            ));
        }

        // Only offer a copy when there is something to copy. A dead button
        // beside an immutable distro's instructions would be worse than none.
        let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);
        if let Some(command) = remedy.command.clone() {
            actions = actions.push(
                action_button(
                    "Copy command",
                    theme::ACCENT,
                    Message::CopyRuntimeCommand(command),
                )
                .enabled(true),
            );
        }
        actions = actions.push(secondary("Re-check", Message::RecheckRuntime).enabled(true));

        container(
            row![
                container(
                    container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
                        .style(theme::dot(theme::ERROR)),
                )
                .padding(theme::SM)
                .style(theme::badge(theme::ERROR)),
                lines.width(Length::Fill),
                actions,
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(theme::MD)
        .style(theme::row)
        .into()
    }

    /// First-run provisioning: what is missing, and one button to fix it.
    /// Proton, the prefix and Battle.net: what is there, and the button that
    /// provisions whatever isn't.
    fn setup_section(&self) -> Element<'_, Message> {
        self.setup_steps(false)
    }

    /// The same list inside the first-run wizard, which owns two of the things
    /// this draws for itself: where to install, and whether Battle.net is
    /// wanted. Drawing them twice on one slide is how the wizard ended up with
    /// two "Install to" fields and two ways to say no to Battle.net.
    fn setup_section_in_wizard(&self) -> Element<'_, Message> {
        self.setup_steps(true)
    }

    fn setup_steps(&self, in_wizard: bool) -> Element<'_, Message> {
        let mut body = column![]
            .extend(self.hint(
                "Proton, the Wine prefix and Battle.net. Each step is skipped if it is \
                 already done.",
            ).map(Element::from))
            .spacing(theme::SM);

        // Where all three steps below install to. Only offered while nothing
        // is installed: after that, moving it is a data migration rather than
        // a setting; see `relocate`. Once the prefix is built, the Wine
        // prefix step below says where it is, so nothing is lost by hiding it.
        if self.library_empty && !in_wizard {
            body = body.push(field_row(
                "Install to".to_owned(),
                row![
                    clickable(
                        "install location",
                        text_input("~/Games/Blizzard", &self.location_input)
                            .on_input(Message::LocationChanged)
                            .on_submit(Message::UseLocation)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                    ),
                    secondary("Use this location", Message::UseLocation).enabled(true),
                    secondary("Open folder", Message::OpenLibrary).enabled(true),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center)
                .into(),
            ));
            body = body.extend(self.hint(
                "Battle.net and the Wine prefix are installed here, and stay here whatever \
                 drives you add later. Games can go on any drive — see Storage.",
            ).map(Element::from));
        }

        let step = |done: bool, label: &'static str, detail: String| {
            let color = match done {
                true => theme::OK,
                false => theme::MUTED,
            };
            container(
                row![
                    container(
                        container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
                            .style(theme::dot(color)),
                    )
                    .padding(theme::SM)
                    .style(theme::badge(color)),
                    column![text(label).size(theme::BODY), mono(detail)].spacing(theme::XS),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(theme::MD)
            .style(theme::row)
        };

        match &self.setup {
            None => body = body.push(muted("checking what is installed…".to_owned(), theme::BODY)),
            Some(status) => {
                // A precondition rather than a step: nothing below can be
                // provisioned without it, and unlike the three steps it is not
                // something Lightforge can do on the user's behalf. It leads
                // the list for the same reason it blocks setup.
                if !status.thirty_two_bit {
                    body = body.push(Self::runtime_row());
                }
                body = body.push(step(
                    status.proton.is_some(),
                    "Proton-GE",
                    match &status.proton {
                        Some(build) => format!(
                            "{} — {}",
                            build.version,
                            lightforge_core::setup::describe_source(build)
                        ),
                        None => "not installed".to_owned(),
                    },
                ));
                body = body.push(step(
                    status.prefix.is_ready(),
                    "Wine prefix",
                    match status.prefix.is_ready() {
                        true => status.prefix.prefix.display().to_string(),
                        false => "not built, or the game link is missing".to_owned(),
                    },
                ));
                // Declined at setup is not the same as missing: the user was
                // told what it costs and said no, so the row offers it rather
                // than reporting it as unfinished.
                if !status.battle_net && !status.battle_net_wanted && !in_wizard {
                    body = body.push(
                        container(
                            row![
                                column![
                                    text("Battle.net").size(theme::BODY),
                                    mono("skipped — World of Warcraft only".to_owned()),
                                ]
                                .spacing(theme::XS)
                                .width(Length::Fill),
                                action_button(
                                    "Install it now",
                                    theme::ACCENT,
                                    Message::InstallBattleNet,
                                )
                                .enabled(!self.setup_running),
                            ]
                            .spacing(theme::MD)
                            .align_y(Alignment::Center),
                        )
                        .width(Length::Fill)
                        .padding(theme::MD)
                        .style(theme::row),
                    );
                } else {
                    body = body.push(step(
                        status.battle_net,
                        "Battle.net",
                        match (status.battle_net, status.battle_net_wanted) {
                            (true, _) => self.layout.battle_net_dir().display().to_string(),
                            (false, true) => "not installed in the prefix".to_owned(),
                            (false, false) => "skipped — World of Warcraft only".to_owned(),
                        },
                    ));
                }
            }
        }

        if self.setup_running {
            let mut log = column![].spacing(theme::XS);
            for entry in &self.setup_sample.log {
                log = log.push(muted(entry.clone(), theme::CAPTION));
            }
            log = log.push(match self.setup_sample.fraction() {
                Some(fraction) => Element::from(progress_bar(0.0..=1.0, fraction).girth(6)),
                // A download with no content-length still has to look alive.
                None => Element::from(mono(format!(
                    "{:.0} MB",
                    self.setup_sample.done as f64 / 1e6
                ))),
            });
            body = body.push(container(log).padding(theme::MD).style(theme::card));
        }

        let ready = self.setup.as_ref().is_some_and(|s| s.is_ready());
        let label = match (self.setup_running, ready) {
            (true, _) => "Working…",
            (false, true) => "Re-run setup",
            (false, false) => "Set up Lightforge",
        };
        let start = action_button(
            label,
            match ready {
                true => theme::PAUSE,
                false => theme::INSTALL,
            },
            Message::RunSetup,
        )
        .width(220.0)
        .enabled(!self.setup_running);
        body = body.push(
            row![start]
                // The same steps, walked through with the questions a first run
                // asks, region, language, drives, and the installs that were
                // here first. Kept here because this is where someone who
                // dismissed it comes looking for it; inside the wizard it would
                // be a button that reopens the thing you are looking at.
                .extend((!in_wizard).then(|| {
                    Element::from(
                        secondary("Run the setup wizard", Message::Wizard(wizard::Msg::Open))
                            .enabled(!self.setup_running),
                    )
                }))
                .spacing(theme::SM)
                .align_y(Alignment::Center),
        );

        body.into()
    }

    /// Every Proton build on the machine, and how to add or remove one.
    ///
    /// Installing lives in this card's own header rather than behind the
    /// update check in the card above, which is where it used to be: the
    /// button that revealed the picker and the picker it revealed were in
    /// two different sections, so the page told you to go and press
    /// something somewhere else.
    fn proton_builds(&self) -> Element<'_, Message> {
        let mut rows = column![].spacing(theme::SM);

        let installed = self.setup.as_ref().map(|s| s.installed.as_slice()).unwrap_or_default();
        if installed.is_empty() {
            rows = rows.push(muted("none found yet".to_owned(), theme::LABEL));
        }

        for build in installed {
            let pinned_by = self.config.pinned_to(&build.version);
            let managed = build.source == lightforge_core::proton::ProtonSource::Managed;

            let mut facts = row![
                mono(build.version.clone()),
                dot(),
                muted(lightforge_core::setup::describe_source(build).to_owned(), theme::CAPTION),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            // Absent until the measuring task answers, rather than a zero
            // that would read as an empty directory.
            if let Some(size) = self.proton_sizes.get(&build.path) {
                facts = facts.push(dot());
                facts = facts.push(muted(bytes_of(*size, 0), theme::CAPTION));
            }

            // Only managed builds are ours to delete, and only when nothing
            // points at them, the reason is shown instead of a dead button,
            // since "why can't I remove this" is the whole question here.
            //
            // The count, not the list. Spelled out it ran to two lines of
            // full product names, "pinned by the app default, Battle.net,
            // World of Warcraft, World of Warcraft Classic, World of
            // Warcraft: Forever, Beta", which is a paragraph where a row
            // wants a word. The names are still there on hover, because the
            // count alone does not tell you what to go and unpin.
            let action: Element<Message> = match (managed, pinned_by.is_empty()) {
                (false, _) => muted("not managed here".to_owned(), theme::CAPTION).into(),
                (true, false) => tooltip(
                    muted(
                        match pinned_by.len() {
                            1 => "in use by 1".to_owned(),
                            n => format!("in use by {n}"),
                        },
                        theme::CAPTION,
                    ),
                    container(text(pinned_by.join(", ")).size(theme::LABEL))
                        .padding([theme::XS, theme::SM])
                        .style(theme::floating),
                    tooltip::Position::Left,
                )
                .into(),
                (true, true) => secondary("Remove", Message::RemoveProton(build.version.clone()))
                    .enabled(!self.setup_running),
            };

            rows = rows.push(
                container(
                    row![facts, Space::new().width(Length::Fill), action]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                )
                .padding([theme::SM, theme::MD])
                .style(theme::row),
            );
        }

        // While a build is coming down, in this card. It used to throw the
        // user to Settings > Setup, because that was the only place a
        // progress bar existed, pressing Install and landing on a different
        // page reads as a misclick.
        if self.setup_running {
            let mut working = column![].spacing(theme::XS);
            for line in &self.setup_sample.log {
                working = working.push(muted(line.clone(), theme::CAPTION));
            }
            working = working.push(match self.setup_sample.fraction() {
                Some(fraction) => Element::from(progress_bar(0.0..=1.0, fraction).girth(6)),
                // No content-length: the bytes are the only honest figure.
                None => mono(bytes_of(self.setup_sample.done, 0)).into(),
            });
            rows = rows.push(container(working).padding(theme::MD).style(theme::card));
        }

        rows.into()
    }

    /// The control that adds a build, for the Builds card's header.
    ///
    /// The release list is fetched on the way into the page (see
    /// [`Lightforge::arrived`]), so by the time anyone reads this it is
    /// usually populated. The three states it can be in all say which one
    /// they are rather than showing an empty dropdown.
    fn proton_install(&self) -> Element<'_, Message> {
        if self.proton_available.is_empty() {
            return row![
                muted(
                    self.proton_notice.clone().unwrap_or_else(|| "checking…".to_owned()),
                    theme::CAPTION,
                ),
                secondary("Check again", Message::CheckProton).enabled(!self.setup_running),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into();
        }

        row![
            self.picker(
                "proton-install".to_owned(),
                self.proton_available
                    .iter()
                    .map(|v| ProtonChoice(Some(v.clone()), Unpinned::Newest))
                    .collect::<Vec<_>>(),
                self.proton_choice.clone().map(|v| ProtonChoice(Some(v), Unpinned::Newest)),
                "Install a version",
                Message::ChooseProtonVersion,
                Length::Fixed(220.0),
                theme::BODY,
            ),
            secondary("Install", Message::InstallProton)
                .enabled(self.proton_choice.is_some() && !self.setup_running),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center)
        .into()
    }

    /// Settings, laid out the way Battle.net lays out its own: a list of
    /// sections on the left and one section at a time on the right, as flat
    /// rows at one shared width rather than a stack of boxes that each size
    /// themselves.
    fn settings(&self) -> Element<'_, Message> {
        let mut nav = column![].spacing(theme::XS).width(Length::Fixed(SETTINGS_NAV_WIDTH));
        for tab in SettingsTab::ALL {
            let entry = button(text(tab.label()).size(theme::tap_text(theme::BODY)))
                .width(Length::Fill)
                .padding(theme::tap([theme::SM, theme::MD]));
            // The tab on show still takes the ring, or there would be no
            // way back into the list from the page beside it.
            nav = nav.push(self.ring(
                focus::Target::SettingsTab(tab),
                Some(Message::SettingsTab(tab)),
                match self.settings_tab == tab {
                    true => entry.style(theme::selected),
                    false => entry.style(theme::ghost).on_press(Message::SettingsTab(tab)),
                },
            ));
        }

        let (title, body) = match self.settings_tab {
            SettingsTab::Setup => ("Setup", self.setup_section()),
            SettingsTab::General => ("General", self.general_section()),
            SettingsTab::Proton => ("Proton", self.proton_section()),
            SettingsTab::BattleNet => ("Battle.net", self.battle_net_section()),
            SettingsTab::Downloads => ("Downloads", self.downloads_section()),
            SettingsTab::Addons => ("Addons", self.addons_section()),
            SettingsTab::Storage => ("Storage", self.storage_section()),
            SettingsTab::About => ("About", self.about_section()),
        };

        // The error leads rather than trails: a failed setup run is reported
        // here, and at the foot of a scrolled section it would go unseen.
        //
        // Every section takes the width it is given. The column used to be
        // capped at a form's measure, which left two thirds of the window
        // empty on a wide monitor and crammed every figure into the left of
        // it; the rows inside already right-align their controls, so the space
        // goes where it is useful rather than being thrown away.
        let mut content = column![].spacing(theme::MD).width(Length::Fill);
        if let Some(e) = &self.error {
            content =
                content.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
        }
        // Storage and Proton name each of their own groups, Drives and Add
        // a drive; Games and their Proton, and Builds, so an outer heading
        // above the first of them is a title for a title.
        content = content.push(match self.settings_tab {
            SettingsTab::Storage | SettingsTab::Proton => body,
            _ => section(title, body),
        });

        column![
            self.header(),
            row![
                focus::group("settings nav", nav),
                container(Space::new().width(Length::Fixed(1.0)))
                    .height(Length::Fill)
                    .style(theme::divider),
                focus::pane(
                    SETTINGS_PAGE,
                    scrollable(container(content).padding([0.0, theme::LG]))
                        .width(Length::Fill)
                        .height(Length::Fill),
                ),
            ]
            .spacing(theme::LG)
            .padding(theme::LG),
        ]
        .into()
    }

    /// Where one product stands on the build the in-game shop needs.
    fn shop_build_state(&self, code: &ProductCode) -> ShopBuild {
        if Kind::of(code) != Kind::Managed {
            return ShopBuild::NotWow;
        }
        match self.config.effective_proton(code) == Some(proton::SHOP_BUILD) {
            true => ShopBuild::Running,
            false => ShopBuild::Elsewhere,
        }
    }

    /// The build a product's row resolves to, pin or no pin.
    ///
    /// Judged the way that product actually starts: a WoW version under its
    /// own pin or the app default, a hand-off title under its pin or
    /// Battle.net's. Only the fallback differs, a pin wins on either path.
    fn row_build(&self, code: &ProductCode) -> Option<&proton::ProtonRelease> {
        let pin = match Kind::of(code) {
            Kind::Managed => self.config.effective_proton(code),
            Kind::HandOff => self.config.handoff_proton(code),
        };
        let installed = self.setup.as_ref().map(|s| s.installed.as_slice())?;
        proton::select(installed, pin)
    }

    /// How a product's build sits against the prefix every product shares.
    fn row_fit(&self, code: &ProductCode) -> proton::PrefixFit {
        let Some(build) = self.row_build(code) else {
            return proton::PrefixFit::Unknown;
        };
        proton::prefix_fit(
            &build.version,
            self.setup.as_ref().and_then(|s| s.prefix.built_by.as_deref()),
        )
    }

    /// The games this page has anything to say about: every configured
    /// product that is actually installed.
    ///
    /// Installed rather than configured, because the list is otherwise
    /// fourteen rows of which three can launch. Read from Battle.net's own
    /// `product.db`, which is the one place that knows about every install in
    /// the prefix, Lightforge's own and the client's alike.
    fn installed_products(&self) -> Vec<(ProductCode, String)> {
        self.config
            .products
            .iter()
            .filter(|p| self.installed_versions.contains_key(p.code.as_str()))
            .map(|p| (p.code.clone(), p.display_name.clone()))
            .collect()
    }

    /// Which Proton each game runs under, and the update check.
    ///
    /// Every installed game, not the one the Play page is showing: Settings is
    /// about the install as a whole, and a row that renamed itself as you
    /// switched tiles read as a bug, correctly, since nothing on the page
    /// said which game it meant.
    fn proton_section(&self) -> Element<'_, Message> {
        // Built per row rather than once: the first entry is "no pin of my
        // own", and what that falls through to is different on a title
        // Battle.net opens than on a game Lightforge launches itself.
        let builds = |unpinned: Unpinned| -> Vec<ProtonChoice> {
            std::iter::once(ProtonChoice(None, unpinned))
                .chain(
                    self.setup
                        .iter()
                        .flat_map(|s| s.installed.iter())
                        .map(|build| ProtonChoice(Some(build.version.clone()), unpinned)),
                )
                .collect()
        };

        let picker = |key: String,
                      unpinned: Unpinned,
                      current: Option<String>,
                      on_pick: Box<dyn Fn(ProtonChoice) -> Message>| {
            self.picker(
                key,
                builds(unpinned),
                Some(ProtonChoice(current, unpinned)),
                "",
                on_pick,
                Length::Fixed(260.0),
                theme::BODY,
            )
        };

        // A row of this card: the name takes the slack, then any signs, then
        // the control. Not [`field_row`], whose 160 pt label column left
        // about a hundred points for a tag and so allowed exactly one. The
        // name is the shortest thing on the line and the picker a fixed
        // width, so giving the *name* the slack is what frees the middle.
        fn line<'a>(
            label: String,
            tags: Vec<Element<'a, Message>>,
            control: Element<'a, Message>,
        ) -> Element<'a, Message> {
            row![text(label).size(theme::BODY).width(Length::Fill)]
                .extend(tags)
                .push(control)
                .spacing(theme::SM)
                .align_y(Alignment::Center)
                .into()
        }

        let games = self.installed_products();
        let mut pins = column![].spacing(theme::SM);

        // One row per installed game, in the order the config lists them,
        // the same order as the Play page's tiles, so the two read alike.
        for (code, label) in &games {
            // Every game, a title Battle.net launches included. Its pin is
            // honoured on the hand-off path too, so the control is not a lie,
            // the client is told to start *that* game, and that game is
            // what the pin is about. What it cannot do is give the title a
            // prefix of its own: there is one, shared, which is what the
            // downgrade sign is for.
            let pin = self
                .config
                .product(code)
                .and_then(|p| p.profile.proton_ge_version.clone());
            let for_code = code.clone();

            // Both signs now, where the old row had space for one and the
            // shop always won. They say different things, one names a build
            // to move to, the other warns about the prefix this one shares,
            // and a game can be in both states at once.
            let mut tags: Vec<Element<'_, Message>> = Vec::new();
            match self.shop_build_state(code) {
                ShopBuild::Running => tags.push(tag("In-game shop working".to_owned(), theme::OK)),
                ShopBuild::Elsewhere => {
                    tags.push(tag(format!("{} recommended", proton::SHOP_BUILD), theme::UPDATE))
                }
                ShopBuild::NotWow => {}
            }
            if self.row_fit(code).is_downgrade() {
                tags.push(tag("Older than this prefix".to_owned(), theme::UPDATE));
            }

            pins = pins.push(line(
                label.clone(),
                tags,
                picker(
                    format!("proton {}", code.as_str()),
                    match Kind::of(code) {
                        Kind::Managed => Unpinned::Default,
                        Kind::HandOff => Unpinned::BattleNet,
                    },
                    pin,
                    Box::new(move |choice| Message::PinProductProton(for_code.clone(), choice)),
                ),
            ));
        }

        // Nothing installed is worth saying: an empty gap above the rule
        // looks like a page that failed to load.
        if games.is_empty() {
            pins = pins.push(muted(
                "Games appear here once they are installed.".to_owned(),
                theme::LABEL,
            ));
        }

        // Under a rule, because they are not games. Everything above is a
        // thing you launch; these two are what the things above fall back to.
        pins = pins.push(
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
        );
        pins = pins.push(line(
            "Default".to_owned(),
            Vec::new(),
            picker(
                "proton default".to_owned(),
                Unpinned::Newest,
                self.config.default_proton.version().map(str::to_owned),
                Box::new(Message::PinDefaultProton),
            ),
        ));
        pins = pins.push(line(
            "Battle.net".to_owned(),
            // The prefix's own build, beside the pin that will next open it.
            // The one fact that makes the signs above legible.
            vec![match self.setup.as_ref().and_then(|s| s.prefix.built_by.as_deref()) {
                Some(built_by) => muted(format!("prefix built by {built_by}"), theme::CAPTION),
                None => muted("no prefix yet".to_owned(), theme::CAPTION),
            }
            .into()],
            picker(
                "proton battle.net".to_owned(),
                Unpinned::Default,
                self.config.battle_net_proton.version().map(str::to_owned),
                Box::new(Message::PinBattleNetProton),
            ),
        ));

        // One line for the whole list rather than one under each game: the
        // reason is the same every time it applies.
        let shop = games
            .iter()
            .any(|(code, _)| matches!(self.shop_build_state(code), ShopBuild::Elsewhere));

        // Said once, where the pin that causes it is set, and not behind the
        // hints toggle: this one has a cost to games the user is not looking at.
        let downgrade: Vec<String> = games
            .iter()
            .filter(|(code, _)| self.row_fit(code).is_downgrade())
            .map(|(_, label)| label.clone())
            .collect();

        let pins = pins
            .extend(self.hint(
                "A game with no pin of its own follows the default. Battle.net and Agent \
                 are a different workload from the game, so they pin separately — a game \
                 Battle.net starts for you follows the game's own pin where it has one, \
                 and Battle.net's where it does not.",
            ).map(Element::from))
            .extend((!downgrade.is_empty()).then(|| {
                text(format!(
                    "⚠ {} {} pinned to a build older than the {} that last upgraded the \
                     prefix. There is one prefix for everything here, and Proton only \
                     migrates it forward, so an older build may leave it in a state the \
                     newer one did not expect. You will be asked again at launch.",
                    downgrade.join(", "),
                    match downgrade.len() {
                        1 => "is",
                        _ => "are",
                    },
                    self.setup
                        .as_ref()
                        .and_then(|s| s.prefix.built_by.clone())
                        .unwrap_or_default(),
                ))
                .size(theme::LABEL)
                .color(theme::UPDATE)
            }).map(Element::from))
            .extend(
                shop.then(|| {
                    self.hint(format!(
                        "World of Warcraft's in-game shop and support pages only render \
                         under {}. Newer builds start the game's embedded browser and lose \
                         its GPU process, which is the grey box instead of a page. \
                         Everything else about the game is unaffected — install it under \
                         Builds below, then pin it here.",
                        proton::SHOP_BUILD,
                    ))
                })
                .flatten()
                .map(Element::from),
            );

        // A card each, because the page is two questions: which Proton does
        // each game run, and which Protons are on this machine. They used to
        // be one list with the second half tacked under the first.
        fn card<'a>(
            title: &'static str,
            header: Option<Element<'a, Message>>,
            body: Element<'a, Message>,
        ) -> Element<'a, Message> {
            let mut heading = row![text(title).size(theme::HEADING)]
                .spacing(theme::SM)
                .align_y(Alignment::Center);
            if let Some(control) = header {
                heading = heading.push(Space::new().width(Length::Fill)).push(control);
            }
            container(
                column![
                    heading,
                    container(Space::new().height(Length::Fixed(1.0)))
                        .width(Length::Fill)
                        .style(theme::divider),
                    body,
                ]
                .spacing(theme::MD),
            )
            .padding(theme::LG)
            .width(Length::Fill)
            .style(theme::card)
            .into()
        }

        column![
            card("Games and their Proton", None, pins.into()),
            card("Builds", Some(self.proton_install()), self.proton_builds()),
        ]
        .spacing(theme::MD)
        .into()
    }

    /// Speed caps, ours and Battle.net's, and pausing while playing.
    /// The Addons section: what every addon follows unless it says otherwise.
    fn addons_section(&self) -> Element<'_, Message> {
        let choices: Vec<ChannelChoice> = Channel::ALL.map(ChannelChoice).to_vec();

        column![field_row(
            "Default addon channel".to_owned(),
            self.picker(
                "addon channel".to_owned(),
                choices,
                Some(ChannelChoice(self.config.addon_channel)),
                "",
                Message::AddonChannelDefault,
                Length::Shrink,
                theme::LABEL,
            ),
        )]
        .extend(
            self.hint(
                "A channel is a floor, not a filter: allowing alpha builds still installs \
                 the newest release when that is newer. Only CurseForge and GitHub publish \
                 pre-release builds — other sources have one build, so the setting is \
                 inert there. An addon given its own channel on the Addons page keeps it.",
            )
            .map(Element::from),
        )
        .push(clickable(
            "addon auto update",
            tick(self.config.addon_auto_update)
                .label("Keep addons up to date automatically")
                .on_toggle(Message::AddonAutoUpdateDefault)
                .text_size(theme::tap_text(theme::BODY)),
        ))
        .extend(
            self.hint(
                "Runs once per game per session — when Lightforge starts, and again the \
                 first time you switch to another game — so addons are current before you \
                 play. A launch waits for it rather than writing into the AddOns folder \
                 the client is reading. An addon can opt out on its own, and an ignored \
                 addon never moves either way.",
            )
            .map(Element::from),
        )
        .spacing(theme::SM)
        .into()
    }

    /// What the launcher does around a launch: the settings that are about
    /// the app itself rather than about a product, a transfer or a prefix.
    fn general_section(&self) -> Element<'_, Message> {
        let toggle = |on: bool, label: &'static str, msg: fn(bool) -> Message| {
            clickable(label, tick(on).label(label).on_toggle(msg).text_size(theme::tap_text(theme::BODY)))
        };

        column![toggle(
            self.config.minimize_on_launch,
            "Minimize Lightforge when a game starts",
            Message::MinimizeOnLaunch,
        )]
        .extend(self.hint(
            "It stays minimized after the game exits — the window is in the taskbar, and a \
             launcher that raises itself over whatever you did next is worse than one that \
             waits.",
        ).map(Element::from))
        .push(toggle(
            self.config.battle_net_minimized,
            "Start Battle.net minimized when launching a game via Battle.net",
            Message::BattleNetMinimized,
        ))
        .extend(self.hint(
            "Only where Battle.net can sign itself in: without that it comes up as a normal \
             window whatever this says, because a sign-in nobody can see is a launch that \
             never happens. Opening Battle.net from the game panel always shows its window.",
        ).map(Element::from))
        .push(toggle(
            self.config.news_on_play,
            "Show News on the Play page",
            Message::NewsOnPlay,
        ))
        .extend(self.hint(
            "A News panel beside the key art, carrying the newest articles about whichever \
             game you are looking at. Only the World of Warcraft family has a news source \
             — the games Battle.net launches never show one.",
        ).map(Element::from))
        .push(toggle(
            !self.config.patch_notes_muted,
            "Show what's new after an update",
            Message::ShowReleaseNotes,
        ))
        .extend(self.hint(
            "Once per version, and only after an update. The notes are always on the \
             Lightforge Updates page whether or not this is on.",
        ).map(Element::from))
        .push(toggle(
            self.config.check_for_updates,
            "Tell me when a new Lightforge is released",
            Message::CheckLauncherUpdates,
        ))
        .extend(self.hint(
            "Asks GitHub once when the launcher starts, and says so if there is a newer \
             release than the one you are running. Nothing is downloaded or installed: \
             your package manager owns the program, and the notice is a link to the \
             releases page. DISCLAIMER.md lists this along with everything else \
             Lightforge contacts.",
        ).map(Element::from))
        .push(toggle(
            self.console(),
            "Use the optimized controller and touch experience",
            Message::ControllerLook,
        ))
        .extend(self.hint(
            "For a Steam Deck, a Steam Machine or a television: fullscreen, larger \
             controls, and a bar along the bottom saying what A and B do. Setup asks \
             on SteamOS; this changes the answer.",
        ).map(Element::from))
        .push(secondary("Add Lightforge to Steam", Message::AddToSteam).enabled(true))
        .extend(self.steam_shortcut.as_ref().map(|outcome| -> Element<'_, Message> {
            match outcome {
                Ok(done) => muted(done.clone(), theme::CAPTION).into(),
                Err(e) => text(e.clone()).size(theme::CAPTION).color(theme::ERROR).into(),
            }
        }))
        .extend(self.hint(
            "A shortcut in the Steam library, with Lightforge's own artwork, so it \
             starts from Game Mode like any game. Setup adds it on a Steam Deck.",
        ).map(Element::from))
        .spacing(theme::SM)
        .into()
    }

    /// How Battle.net and every game it launches are started.
    ///
    /// **One profile, not one per game, and that is the mechanism rather than
    /// a shortcut.** Lightforge only sets the environment of a Battle.net
    /// client it starts cold; a client already running takes the launch over
    /// its own IPC queue, where there is no new process to hand an
    /// environment to. Two games started from one client session therefore
    /// cannot have different environments, so a per-game control here would
    /// be a lie, `Config::battle_net_tweaks` and `SPEC.md` both say so.
    ///
    /// There is no client-settings half: a hand-off game has no `Config.wtf`.
    /// So this is the host tools, the runtime switches and the escape hatch,
    /// and no preset row, a preset decides the frames-versus-scenery trade,
    /// and none of these are a point on that axis.
    fn battle_net_section(&self) -> Element<'_, Message> {
        let tweaks = self.tweaks_of(Scope::BattleNet);
        let scope = Scope::BattleNet;

        // No heading of its own: `section()` puts "Battle.net" above this,
        // the way it does for General and Downloads. Storage and Proton name
        // their own groups and opt out of that; one group does not need to.
        let mut body = column![]
            .spacing(theme::SM)
            .extend(
                self.hint(
                    "These apply to the Battle.net client and therefore to every game it \
                     launches — Diablo, Hearthstone, Overwatch. They cannot be set per game, \
                     because Lightforge only gets to choose the environment when it starts \
                     the client itself: a client that is already open is handed the launch \
                     over its own message queue.",
                )
                .map(Element::from),
            );

        if self.tools.game_performance.is_some() {
            body = body.push(self.tweak_check(
                scope,
                "Hold the CPU on its performance profile",
                Tweak::GamePerformance,
                tweaks.wants_game_performance(),
            ));
        }
        if self.tools.gamemode.is_some() {
            body = body.push(self.tweak_check(
                scope,
                "Launch through GameMode",
                Tweak::GameMode,
                tweaks.gamemode,
            ));
        }
        if self.tools.mangohud.is_some() {
            body = body.push(self.tweak_check(
                scope,
                "Show the MangoHud overlay",
                Tweak::MangoHud,
                tweaks.mangohud,
            ));
        }
        body = body.push(self.tweak_check(
            scope,
            "Give the shader cache room",
            Tweak::BigShaderCache,
            tweaks.big_shader_cache,
        ));
        body = body.extend(self.runtime_rows(scope, &tweaks));

        let missing: Vec<&str> = [
            self.tools.game_performance.is_none().then_some("game-performance"),
            self.tools.gamemode.is_none().then_some("GameMode"),
            self.tools.mangohud.is_none().then_some("MangoHud"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            body = body.push(muted(
                format!("{} is not installed, so it is not offered.", missing.join(", ")),
                theme::CAPTION,
            ));
        }

        // The only way to tune a hand-off game beyond the switches above: it
        // has no client settings for Lightforge to write, so anything else
        // has to arrive as an environment variable.
        body = body.push(self.env_card(scope, &tweaks));
        body.spacing(theme::SM).into()
    }

    fn downloads_section(&self) -> Element<'_, Message> {
        let speed = |input: &str, placeholder: &str, on_input: fn(String) -> Message| {
            row![
                clickable(
                    format!("speed {placeholder}"),
                    text_input(placeholder, input)
                        .on_input(on_input)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                ),
                muted("MB/s".to_owned(), theme::LABEL),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into()
        };

        column![field_row(
            "Speed limit".to_owned(),
            speed(&self.limit_input, "unlimited", Message::DownloadLimitChanged),
        )]
        .extend(
            self.hint("Applies to game downloads. Proton builds and addons are not capped.").map(Element::from))
        .push(field_row(
            "Battle.net's limit".to_owned(),
            speed(&self.bnet_limit_input, "none", Message::BattleNetLimitChanged),
        ))
        .extend(self.hint(
            "Battle.net's own cap, for what it downloads. Read from its config and written \
             back; clearing the field removes the setting and restores its default.",
        ).map(Element::from))
        .push(clickable(
            "pause while playing",
            tick(self.config.pause_downloads_while_playing)
                .label("Pause downloads while a game is running")
                .on_toggle(Message::PauseWhilePlaying)
                .text_size(theme::tap_text(theme::BODY)),
        ))
        .extend(self.hint(
            "The game you launch always pauses its own download — it and the launcher \
             would be sharing one storage file. This covers the rest of the queue.",
        ).map(Element::from))
        .spacing(theme::SM)
        .into()
    }

    /// All the app shows when `config.toml` did not load.
    fn broken_config_screen<'a>(&'a self, error: &'a str) -> Element<'a, Message> {
        let body = column![
            text("Lightforge can't read its settings").size(theme::TITLE),
            muted(
                "Nothing has been changed. Fix the file, or move it aside to start over \
                 with defaults, then restart Lightforge. A copy was saved next to it as \
                 config.toml.broken."
                    .to_owned(),
                theme::BODY,
            ),
            container(mono(error.to_owned()))
                .padding(theme::MD)
                .width(Length::Fill)
                .style(theme::row),
            row![
                secondary("Open config folder", Message::OpenConfigFolder).enabled(true),
            ],
        ]
        .spacing(theme::MD)
        .max_width(MESSAGE_WIDTH);

        container(body).center(Length::Fill).padding(theme::XL).style(theme::scrim).into()
    }

    /// The install dialog's card.
    fn install_card<'a>(&'a self, dialog: &'a InstallDialog) -> Element<'a, Message> {
        let mut drives = column![].spacing(theme::SM);
        for (index, target) in dialog.targets.iter().enumerate() {
            let chosen = index == dialog.chosen;
            // Fixed, and wide enough for the longest thing that goes in it.
            // Left to shrink it was squeezed by the path beside it until
            // "1.37 TiB free" came out as three stacked words.
            let mut right = column![]
                .spacing(2)
                .width(Length::Fixed(110.0))
                .align_x(Alignment::End);
            right = match target.disk {
                Some(disk) => right.push(mono(format!("{} free", size_of(disk.free)))),
                None => right.push(muted("—".to_owned(), theme::LABEL)),
            };
            // Battle.net's own wording and colour for the same fact: a second
            // WoW version in a folder that already has one shares its data
            // store, so what is already downloaded is not fetched twice.
            if target.shares {
                right = right.push(text("Sharing data").size(theme::CAPTION).color(theme::OK));
            }

            let mut left = column![row![
                text(target.label.clone()).size(theme::BODY),
                Space::new().width(Length::Fixed(theme::SM)),
            ]
            .align_y(Alignment::Center)]
            .spacing(2);
            // `~/Games/...` rather than `/home/<name>/Games/...`: the home
            // prefix is the longest and least informative part of the path,
            // and this row is where the width is tightest.
            left = left.push(muted(home_relative(&target.install), theme::CAPTION));
            if let Some(reason) = target.state.reason(&target.label) {
                left = left.push(text(reason).size(theme::CAPTION).color(theme::ERROR));
            }

            let usable = target.state != library::State::Missing;
            drives = drives.push(tappable(
                format!("install target {index}"),
                usable.then_some(Message::InstallTarget(index)),
                button(
                    // The left column takes what the right one does not, so
                    // the path is the only thing that ever wraps.
                    row![left.width(Length::Fill), right]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([theme::SM, theme::MD])
                .style(match chosen {
                    true => theme::selected,
                    false => theme::card_button,
                }),
            ));
        }
        let location: Element<Message> = column![drives]
            .extend(
                self.hint(
                    "World of Warcraft is installed in a folder of its own on the drive you \
                     pick. Every WoW version you later put on the same drive shares its game \
                     data; one on another drive is a separate install.",
                )
                .map(Element::from),
            )
            .spacing(theme::XS)
            .into();

        let mut body = column![field_row("Install to".to_owned(), location)].spacing(theme::MD);

        let mut ready = false;
        match &dialog.stage {
            DialogStage::Calculating => {
                body = body.push(muted("Calculating size…".to_owned(), theme::LABEL));
            }
            DialogStage::Failed(e) => {
                body = body.push(
                    text(format!("⚠ Could not read this build: {e}"))
                        .size(theme::LABEL)
                        .color(theme::ERROR),
                );
            }
            DialogStage::Ready(offerings) => {
                let extra = offerings
                    .optional_bytes
                    .iter()
                    .find(|(tag, _)| tag == HIGH_RES)
                    .map(|(_, bytes)| *bytes)
                    // Every build names the tag; only some have textures
                    // behind it. Retail's adds nothing, and a "+0 MiB"
                    // switch is not a choice.
                    .filter(|bytes| *bytes > 0);
                if let Some(extra) = extra {
                    body = body.push(field_row(
                        "Optional".to_owned(),
                        row![
                            clickable(
                                "high res",
                                tick(dialog.high_res)
                                    .label(optional_label(HIGH_RES))
                                    .on_toggle(Message::InstallHighRes)
                                    .text_size(theme::tap_text(theme::BODY)),
                            ),
                            muted(format!("+{}", size_of(extra)), theme::LABEL),
                        ]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center)
                        .into(),
                    ));
                }

                let need = offerings.base_bytes
                    + if dialog.high_res { extra.unwrap_or(0) } else { 0 };
                // Exact: priced against the store, the same test the plan
                // applies, so shared files are already left out. Download
                // size, not disk size, the extracted install files (the
                // executables and the like) come on top, which is most of
                // why Battle.net's "required" reads a little higher.
                body = body.push(field_row(
                    "Download".to_owned(),
                    mono(format!("{} to download", size_of(need))).into(),
                ));
                match dialog.target().and_then(|t| t.disk) {
                    Some(disk) => {
                        body = body.push(field_row(
                            "Free space".to_owned(),
                            mono(size_of(disk.free)).into(),
                        ));
                        ready = disk.free > need;
                        if !ready {
                            body = body.push(
                                text("⚠ Not enough free space on that disk.")
                                    .size(theme::LABEL)
                                    .color(theme::ERROR),
                            );
                        }
                    }
                    None => ready = true,
                }
            }
        }

        // A pair, so one size: the same height and width, labels centred,
        // with only the fill telling the default from the way out.
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable("cancel", Some(Message::InstallCancel), choice("Cancel").style(theme::outlined)),
            tappable(
                "start install",
                ready.then_some(Message::InstallStart),
                choice("Start install").style(theme::filled(theme::INSTALL)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![
                section(format!("Install {}", dialog.label), body.into()),
                actions,
            ]
            .spacing(theme::LG),
        )
        .width(Length::Fixed(640.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The card asking whether to open the shared prefix with an older build.
    ///
    /// Two buttons and no third option on purpose. "Launch anyway" is the one
    /// the user came for, so it is not hidden behind an explanation; the
    /// explanation is there because the cost falls on games they are not
    /// looking at.
    fn downgrade_card<'a>(&'a self, dialog: &'a DowngradeDialog) -> Element<'a, Message> {
        let what = match dialog.what {
            Launch::Game => format!("Launching {}", dialog.label),
            Launch::BattleNet => format!("Starting {} through Battle.net", dialog.label),
            Launch::Client => "Opening Battle.net".to_owned(),
            Launch::Install => "Opening Battle.net's install flow".to_owned(),
        };

        let body = column![
            field_row("Would run".to_owned(), mono(dialog.pick.clone()).into()),
            field_row("Prefix built by".to_owned(), mono(dialog.built_by.clone()).into()),
            text(format!(
                "{what} under {} would open the Wine prefix with an older Proton than the \
                 {} that last upgraded it.",
                dialog.pick, dialog.built_by,
            ))
            .size(theme::LABEL)
            .color(theme::UPDATE),
            muted(
                "Every game here shares that one prefix, along with the Battle.net install \
                 and its login. Proton migrates a prefix forward and never back, so an older \
                 build can leave it in a state the newer one did not expect — which shows up \
                 as a game or the client failing to start, not as an error here. If this \
                 build is the one the game needs, go ahead; if it is not, pin it back under \
                 Settings › Proton."
                    .to_owned(),
                theme::LABEL,
            ),
        ]
        .spacing(theme::MD);

        let choice = |label: String| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(170.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable(
                "cancel",
                Some(Message::DowngradeCancel),
                choice("Cancel".to_owned()).style(theme::outlined),
            ),
            tappable(
                "launch anyway",
                Some(Message::DowngradeConfirm),
                choice("Launch anyway".to_owned()).style(theme::filled(theme::UPDATE)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section("Older Proton than this prefix".to_owned(), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The Game Settings popup. Backups are its first section; the game's
    /// other per-product settings are meant to join them here.
    fn game_settings_card<'a>(&'a self, settings: &'a GameSettings) -> Element<'a, Message> {
        let idle = !settings.working && !self.busy;
        let keep = self.config.backup_keep;

        let mut body = column![muted(
            format!(
                "Everything you set up in the game, every addon's SavedVariables, your key \
                 bindings and macros, lives in WTF/Account. Lightforge backs it up before an \
                 addon update and before the first launch on a new client build, and keeps \
                 the newest {keep}."
            ),
            theme::LABEL,
        )]
        .spacing(theme::MD);

        match settings.backups.as_deref() {
            None => body = body.push(muted("reading backups…".to_owned(), theme::BODY)),
            Some([]) => {
                body = body.push(empty(
                    "No backups yet.",
                    "One is taken automatically the next time it is needed.",
                ))
            }
            Some(backups) => {
                let mut list = column![].spacing(theme::XS);
                for backup in backups {
                    let facts = [
                        Some(backup.reason.label().to_owned()),
                        backup.build.clone(),
                        Some(size_of(backup.size)),
                    ];
                    let mut line = row![column![
                        text(backup.when()).size(theme::BODY),
                        muted(facts.into_iter().flatten().collect::<Vec<_>>().join(" · "), theme::CAPTION),
                    ]
                    .spacing(theme::XS)]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center)
                    .push(Space::new().width(Length::Fill));

                    line = match settings.confirm.as_ref() == Some(&backup.path) {
                        true => line
                            .push(muted("Replace the current settings?".to_owned(), theme::LABEL))
                            .push(secondary("Cancel", Message::CancelRestore).enabled(true))
                            .push(action_button("Restore", theme::UPDATE, Message::ConfirmRestore).enabled(idle)),
                        false => line.push(
                            secondary("Restore", Message::AskRestore(backup.path.clone())).enabled(idle),
                        ),
                    };
                    list = list.push(container(line).padding(theme::MD).style(theme::row));
                }
                // Five rows fit the popup; a longer `backup_keep` scrolls
                // rather than pushing the buttons off the card.
                let height = match backups.len() > 5 {
                    true => Length::Fixed(5.0 * 66.0),
                    false => Length::Shrink,
                };
                body = body.push(scrollable(list).height(height).style(theme::scrollbar));
                body = body.push(muted(
                    "Restoring backs up the current settings first, so it can be undone \
                     the same way. The game has to be closed."
                        .to_owned(),
                    theme::CAPTION,
                ));
            }
        }

        if let Some(note) = &settings.note {
            body = body.push(match note {
                Ok(line) => muted(line.clone(), theme::LABEL),
                Err(e) => text(e.clone()).size(theme::LABEL).color(theme::ERROR),
            });
        }

        let actions = row![
            secondary("Back up now", Message::BackupNow).enabled(idle),
            Space::new().width(Length::Fill),
            secondary("Close", Message::CloseGameSettings).enabled(true),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![
                column![
                    text("Game Settings").size(theme::TITLE),
                    muted(settings.label.clone(), theme::LABEL),
                ]
                .spacing(theme::XS),
                section("Settings backups", body.into()),
                actions,
            ]
            .spacing(theme::LG),
        )
        .width(Length::Fixed(620.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The uninstall confirmation's card.
    /// The builds one addon offers, for choosing an older one on purpose.
    ///
    /// Choosing here always pins. Installing an older build without pinning
    /// would be undone by the next pass within the minute, which is not a
    /// choice so much as a delay.
    fn version_card<'a>(&'a self, picker: &'a VersionPicker) -> Element<'a, Message> {
        let mut body = column![
            text(format!("Choose a build of {}", picker.name)).size(theme::TITLE),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(560.0));

        body = body.extend(
            self.hint(
                "Installing a build from here pins the addon to it, so nothing moves it \
                 again until you unpin. Only CurseForge publishes a build list.",
            )
            .map(Element::from),
        );

        match picker.builds.as_deref() {
            None => body = body.push(muted("asking the catalogue…".to_owned(), theme::BODY)),
            Some([]) => {
                body = body.push(empty(
                    match &picker.problem {
                        Some(e) => e.as_str(),
                        None => "This provider publishes no build list.",
                    },
                    "Only CurseForge offers older builds; the others ship one.",
                ));
            }
            Some(builds) => {
                let mut list = column![].spacing(theme::XS);
                for build in builds {
                    let installed = picker.installed.as_deref() == Some(build.file_id.as_str());
                    let mut head = row![text(
                        build
                            .display_name
                            .clone()
                            .or_else(|| build.file_name.clone())
                            .unwrap_or_else(|| build.file_id.clone()),
                    )
                    .size(theme::BODY)]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center);

                    // The release type in the provider's own words, because
                    // that is what the author labelled the build with.
                    if let Some(kind) = &build.release_type {
                        if kind != "release" {
                            head = head.push(pill(kind.clone(), theme::UPDATE));
                        }
                    }
                    if installed {
                        head = head.push(pill("installed".to_owned(), theme::MUTED));
                    }

                    let mut line = row![column![
                        head,
                        muted(
                            match (&build.file_date, build.file_length) {
                                (Some(date), Some(size)) =>
                                    format!("{} · {:.1} MB", day_of(date), size as f64 / 1e6),
                                (Some(date), None) => day_of(date),
                                (None, Some(size)) => format!("{:.1} MB", size as f64 / 1e6),
                                (None, None) => String::new(),
                            },
                            theme::CAPTION,
                        ),
                    ]
                    .spacing(theme::XS)]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center);

                    line = line.push(Space::new().width(Length::Fill));
                    line = line.push(
                        secondary(
                            "Pin to this",
                            Message::InstallVersion(
                                picker.provider.clone(),
                                picker.id.clone(),
                                build.file_id.clone(),
                            ),
                        )
                        .enabled(!self.addons_locked()),
                    );
                    list = list.push(container(line).padding(theme::MD).style(theme::row));
                }
                body = body.push(focus::region(
                    "addon versions",
                    scrollable(list).height(Length::Fixed(340.0)).style(theme::scrollbar),
                ));
            }
        }

        body = body.push(
            row![
                Space::new().width(Length::Fill),
                secondary("Cancel", Message::CancelVersionPicker).enabled(true),
            ]
            .spacing(theme::SM),
        );

        container(container(body).padding(theme::LG).style(theme::floating))
            .center(Length::Fill)
            .padding(theme::XL)
            .into()
    }

    /// One addon's project page: what it is, and what changed in the build
    /// you would get.
    ///
    /// The two halves are tabs rather than one long scroll because they
    /// answer different questions and a description can be thousands of words,
    /// the changelog would never be found under it.
    fn detail_card<'a>(&'a self, detail: &'a AddonDetail) -> Element<'a, Message> {
        let title = detail
            .loaded
            .as_ref()
            .map(|d| d.project.name.clone())
            .unwrap_or_else(|| detail.name.clone());
        let mut head = row![text(title).size(theme::TITLE)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

        if let Some(loaded) = &detail.loaded {
            if loaded.project.download_count > 0 {
                head = head.push(muted(
                    format!("{} downloads", downloads_of(loaded.project.download_count)),
                    theme::CAPTION,
                ));
            }
        }

        let mut body = column![head].spacing(theme::SM).width(Length::Fixed(620.0));

        // The author line sits under the name rather than beside it: addons
        // with four maintainers are common and the row would wrap badly.
        if let Some(loaded) = &detail.loaded {
            if !loaded.project.authors.is_empty() {
                body = body.push(muted(
                    format!("by {}", loaded.project.authors.join(", ")),
                    theme::CAPTION,
                ));
            }
            if let Some(summary) = &loaded.project.summary {
                body = body.push(text(summary.clone()).size(theme::BODY));
            }
        }

        match (&detail.loaded, &detail.problem) {
            (None, None) => {
                body = body.push(muted("asking the catalogue…".to_owned(), theme::BODY));
            }
            (_, Some(problem)) => {
                body = body.push(empty(
                    problem.as_str(),
                    "Only CurseForge publishes a description and a changelog.",
                ));
            }
            (Some(loaded), None) => {
                body = body.push(self.detail_tabs(detail, loaded));
                body = body.push(self.detail_body(detail, loaded));
            }
        }

        // Reading the changelog and then having to find the row again to act
        // on it is the whole reason this card is worth having, so the action
        // is here. Which action depends on whether it is already installed.
        let installed = self.addon_is_installed(&detail.provider, &detail.id);

        let mut footer = row![Space::new().width(Length::Fill)].spacing(theme::SM);
        if let Some(url) = detail.loaded.as_ref().and_then(|d| d.project.website_url.clone()) {
            footer = footer.push(secondary("Open the page", Message::OpenUrl(url)).enabled(true));
        }
        // The same offer the installed row's menu makes, in the panel someone
        // reads *before* installing, where a project's own repository is
        // often the thing they came to find.
        if let Some(url) = detail
            .loaded
            .as_ref()
            .and_then(|d| d.project.source_url.clone().or_else(|| d.project.author_url.clone()))
        {
            footer =
                footer.push(secondary("Support the author", Message::OpenUrl(url)).enabled(true));
        }
        if !installed {
            let install: Element<'static, Message> = action_button(
                "Install",
                theme::INSTALL,
                Message::InstallAddon(detail.provider.clone(), detail.id.clone()),
            )
            .enabled(!self.addons_locked());
            footer = footer.push(install);
        }
        footer = footer.push(secondary("Close", Message::CloseAddonDetail).enabled(true));
        body = body.push(footer);

        container(container(body).padding(theme::LG).style(theme::floating))
            .center(Length::Fill)
            .padding(theme::XL)
            .into()
    }

    /// The two tab buttons, each naming what it holds.
    ///
    /// A tab with nothing behind it is shown disabled rather than hidden:
    /// hiding it would move the other one, and "this addon has no changelog"
    /// is itself an answer worth being able to see.
    fn detail_tabs<'a>(
        &'a self,
        detail: &'a AddonDetail,
        loaded: &'a addonwork::Detail,
    ) -> Element<'a, Message> {
        let tab = |label: String, which: DetailTab, available: bool| {
            let selected = detail.tab == which;
            let key = format!("detail tab {label}");
            let tab = button(text(label).size(theme::BODY))
                .padding([theme::XS, theme::MD])
                .style(match selected {
                    true => theme::selected,
                    false => theme::ghost,
                });
            tappable(key, (available && !selected).then_some(Message::AddonDetailTab(which)), tab)
        };

        let changelog_label = match &loaded.changelog_build {
            Some(build) => format!("What changed in {build}"),
            None => "What changed".to_owned(),
        };
        row![
            tab(
                changelog_label,
                DetailTab::Changelog,
                loaded.changelog.as_ref().is_some_and(|d| !d.is_empty()),
            ),
            tab(
                "About".to_owned(),
                DetailTab::About,
                loaded.description.as_ref().is_some_and(|d| !d.is_empty()),
            ),
        ]
        .spacing(theme::XS)
        .into()
    }

    /// Whichever document the open tab names, scrolled.
    fn detail_body<'a>(
        &'a self,
        detail: &'a AddonDetail,
        loaded: &'a addonwork::Detail,
    ) -> Element<'a, Message> {
        let (document, missing) = match detail.tab {
            DetailTab::Changelog => (
                loaded.changelog.as_ref(),
                "This build ships no changelog — plenty of authors never write one.",
            ),
            DetailTab::About => (
                loaded.description.as_ref(),
                "This addon has no description beyond its summary.",
            ),
        };

        let Some(document) = document.filter(|d| !d.is_empty()) else {
            return empty(missing, "The CurseForge page has whatever else there is.");
        };

        let mut content = column![].spacing(theme::SM);
        for block in &document.blocks {
            content = content.push(rich_block(block));
        }
        if document.truncated {
            content = content.push(muted(
                "This is longer than Lightforge shows — the rest is on the page.".to_owned(),
                theme::CAPTION,
            ));
        }

        focus::region(
            "addon detail",
            scrollable(container(content).padding(theme::MD))
                .height(Length::Fixed(320.0))
                .style(theme::scrollbar),
        )
    }

    /// What changed in the version that has just started.
    ///
    /// A dialog rather than a page, because it is the one thing in the app
    /// that opens itself: it has to be read and closed, and every other
    /// screen stays reachable afterwards from the navigation. The same notes
    /// live on the Lightforge Updates page for anyone who wants them again,
    /// including after the checkbox has been ticked.
    fn patch_notes_card<'a>(&'a self, notes: &'a PatchNotes) -> Element<'a, Message> {
        // The notes are as long as the release was. This one is fifteen
        // wrapped paragraphs, and a card that grows with them runs off the
        // top and bottom of the window, taking the button with it.
        //
        // Only the notes scroll. The heading says what the card is and the
        // checkbox and button are how it is answered; both stay put, so the
        // dialog is dismissable at any length without scrolling to find the
        // way out. The cap plus the heading, the version line and the actions
        // come to about 630pt, inside the 820 the window is never smaller
        // than (`WINDOW_MIN`).
        //
        // `theme::scrollbar_shape` sets a spacing, which is what makes iced
        // reserve room for the bar beside the text rather than draw it over
        // the words, and only while it is actually needed, so a short
        // release is laid out exactly as before.
        const NOTES_MAX: f32 = 480.0;

        let body = column![
            muted(format!("Version {}", notes.release.version), theme::LABEL),
            container(focus::region(
                "notes",
                scrollable(release_notes(&notes.release))
                    .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                    .style(theme::scrollbar),
            ))
            .max_height(NOTES_MAX),
        ]
        .spacing(theme::SM);

        let actions = row![
            self.ring(
                focus::Target::Key("mute notes".to_owned()),
                Some(Message::MutePatchNotes(!notes.mute)),
                tick(notes.mute)
                    .label("Never show this again")
                    .on_toggle(Message::MutePatchNotes)
                    .text_size(theme::tap_text(theme::BODY)),
            ),
            Space::new().width(Length::Fill),
            self.ring(
                focus::Target::NotesDismiss,
                Some(Message::DismissPatchNotes),
                button(text("Got it").size(theme::LABEL).width(Length::Fill).center())
                    .width(Length::Fixed(150.0))
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .style(theme::filled(theme::ACCENT))
                    .on_press(Message::DismissPatchNotes),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section("What's new in Lightforge".to_owned(), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// A newer Lightforge exists. Says so, links to it, installs nothing.
    ///
    /// Deliberately small and deliberately passive: the binary belongs to
    /// whatever installed it, a distribution package most of the time, and a
    /// launcher that replaced itself behind one would be the wrong kind of
    /// helpful. The releases page is where the decision belongs.
    fn update_card<'a>(
        &'a self,
        available: &'a lightforge_core::update::Available,
    ) -> Element<'a, Message> {
        let body = column![
            text(format!("Lightforge {} is available.", available.latest)).size(theme::BODY),
            muted(
                format!(
                    "You are running {}. Nothing is installed automatically; \
                     the releases page has the downloads and the notes.",
                    available.current
                ),
                theme::LABEL,
            ),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(420.0));

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            self.ring(
                focus::Target::UpdateLater,
                Some(Message::DismissUpdate),
                choice("Not now").style(theme::outlined).on_press(Message::DismissUpdate),
            ),
            self.ring(
                focus::Target::UpdateOpen,
                Some(Message::OpenReleases),
                choice("See the release")
                    .style(theme::filled(theme::ACCENT))
                    .on_press(Message::OpenReleases),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section("Update available".to_owned(), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(420.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The confirmation for giving a download up.
    ///
    /// Two shapes, because an update and a first install are not the same
    /// question. An update's files are never removed: the game on disk plays
    /// right now, and what has already come down is a head start on the next
    /// attempt rather than waste.
    ///
    /// One line of consequence under the title and two buttons, deliberately.
    /// The earlier version offered a third way out, cancel but keep the files,
    /// and stated the figure it would free; both made the reader do work to
    /// answer a question they had already decided.
    fn cancel_card<'a>(&'a self, dialog: &'a CancelDialog) -> Element<'a, Message> {
        let updating = dialog.installed.is_some();
        let (title, hint, keep, give_up) = match &dialog.installed {
            Some(version) => (
                format!("Cancel the update to {}?", dialog.label),
                format!(
                    "{} stays on {version}, and what has already downloaded is \
                     kept for the next update.",
                    dialog.label
                ),
                "Keep downloading",
                "Cancel update",
            ),
            None => (
                format!("Cancel installing {}?", dialog.label),
                "Cancelling the download will remove all downloaded files on \
                 your drive."
                    .to_owned(),
                "Keep installing",
                "Cancel install",
            ),
        };

        let body = column![muted(hint, theme::BODY)]
            .spacing(theme::SM)
            .width(Length::Fixed(520.0));

        // One shape for both, so neither reads as the safe one by being
        // bigger. Only the fill says which is which.
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(170.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable("keep", Some(Message::CancelDownloadDismiss), choice(keep).style(theme::outlined)),
            tappable(
                "give up",
                Some(Message::CancelDownloadConfirm { delete: !updating }),
                choice(give_up).style(theme::filled(theme::ERROR)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(column![section(title, body.into()), actions].spacing(theme::LG))
            .width(Length::Fixed(520.0))
            .padding(theme::LG)
            .style(theme::dialog)
            .into()
    }

    fn uninstall_card<'a>(&'a self, dialog: &'a UninstallDialog) -> Element<'a, Message> {
        let mut body = column![].spacing(theme::MD);
        let mut ready = false;
        match &dialog.plan {
            None => {
                body = body.push(muted("Working out what to remove…".to_owned(), theme::LABEL));
            }
            Some(Err(e)) => {
                body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
            }
            Some(Ok(preview)) if preview.plan.is_empty() => {
                body = body.push(muted(
                    "Nothing to remove — it is not installed here.".to_owned(),
                    theme::LABEL,
                ));
            }
            Some(Ok(preview)) => {
                ready = true;
                let plan = &preview.plan;
                let reclaim = preview.reclaim.as_ref().and_then(|r| r.as_ref().ok());
                let deletes = column(
                    plan.flavor_dir
                        .iter()
                        .chain(plan.store.iter())
                        .map(|path| mono(path.display().to_string()).into()),
                )
                .spacing(theme::XS);
                body = body
                    .push(field_row("Deletes".to_owned(), deletes.into()))
                    .push(field_row(
                        "Frees".to_owned(),
                        mono(size_of(plan.bytes + reclaim.map_or(0, |r| r.freed))).into(),
                    ));

                let (note, color) = match plan.is_whole_install() {
                    true => (
                        "This is the last WoW version here, so the shared game data goes with \
                         it."
                            .to_owned(),
                        theme::ERROR,
                    ),
                    false => {
                        let others = plan
                            .remaining
                            .iter()
                            .map(|c| self.label_of(c))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let note = match &preview.reclaim {
                            // Only what nobody else uses goes; an archive
                            // it shares with them keeps its bytes as dead
                            // space until something rewrites it.
                            Some(Ok(r)) if r.stranded > 0 => format!(
                                "Game data it shares with {others} stays. About {} of its own \
                                 is scattered too thinly through their archives to be worth \
                                 rewriting and stays as unused space.",
                                size_of(r.stranded)
                            ),
                            Some(Ok(_)) => format!("Game data it shares with {others} stays."),
                            _ => format!(
                                "Could not check which game data only it uses, so the shared \
                                 store is left as it is for {others}."
                            ),
                        };
                        (note, theme::MUTED)
                    }
                };
                body = body.push(text(note).size(theme::LABEL).color(color));
                if let Some(r) = reclaim.filter(|r| r.moved > 0) {
                    let line = match dialog.working {
                        true => format!(
                            "Compacting: {} of {} copied.",
                            size_of(dialog.copied.load(std::sync::atomic::Ordering::Relaxed).min(r.moved)),
                            size_of(r.moved)
                        ),
                        false => format!(
                            "To free its space, {} of the game data they still use is copied into \
                             fresh archives first. That can take a few minutes.",
                            size_of(r.moved)
                        ),
                    };
                    body = body.push(muted(line, theme::LABEL));
                }
                body = body.push(muted(
                    "Its addons and settings (Interface, WTF) are deleted with it. Battle.net \
                     stops listing it too."
                        .to_owned(),
                    theme::LABEL,
                ));
            }
        }

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable(
                "cancel",
                (!dialog.working).then_some(Message::UninstallCancel),
                choice("Cancel").style(theme::outlined),
            ),
            tappable(
                "uninstall",
                (ready && !dialog.working).then_some(Message::UninstallConfirm),
                choice(if dialog.working { "Uninstalling…" } else { "Uninstall" })
                    .style(theme::filled(theme::ERROR)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section(format!("Uninstall {}", dialog.label), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// What a bug report in this category carries beyond the base bundle.
    ///
    /// Only what answers that category's first question: the addon list for
    /// an addon problem, the queue and the drives for a download problem, the
    /// settings for a settings problem. Read when the category is picked, so
    /// the card shows it before anything is sent, and scrubbed like the rest.
    fn report_attachment(&self, category: report::Category) -> String {
        use std::fmt::Write as _;
        use report::Category;

        let code = &self.selected.code;
        let mut out = String::new();
        match category {
            Category::Addons => {
                let dir = self.layout.addons_dir(code);
                let _ = writeln!(out, "\n--- addons: {} ---", dir.display());
                let _ = writeln!(out, "{:<16}{}", "flavor", lightforge_core::addons::flavor_of(code));
                let installed = match lightforge_core::addons::scan(&dir) {
                    Ok(installed) => installed,
                    Err(e) => {
                        let _ = writeln!(out, "the folder could not be read: {}", chain(&e));
                        Vec::new()
                    }
                };
                // Read-only: the Addons page prunes the ledger as it lists, a
                // report must not change what it is reporting on.
                let ledger = AppDirs::discover()
                    .ok()
                    .map(|dirs| manage::ledger_path(dirs.data_dir(), &dir, code))
                    .map(|path| lightforge_core::addons::Ledger::load(&path));
                let ledger = match ledger {
                    Some(Ok(ledger)) => ledger,
                    Some(Err(e)) => {
                        let _ = writeln!(out, "the ledger could not be read: {}", chain(&e));
                        lightforge_core::addons::Ledger::default()
                    }
                    None => lightforge_core::addons::Ledger::default(),
                };
                let rows = addonwork::rows_from(&installed, &ledger);
                let managed = rows.iter().filter(|r| r.managed.is_some() && r.child_of.is_none()).count();
                let _ = writeln!(out, "{:<16}{} folders, {managed} managed", "installed", rows.len());
                for row in rows.iter().filter(|r| r.child_of.is_none()) {
                    let owner = match &row.managed {
                        Some(m) => {
                            let mut owner = format!("{}:{}", m.provider, m.id);
                            if let Some(file) = &m.file_id {
                                let _ = write!(owner, " file {file}");
                            }
                            if let Some(channel) = m.channel {
                                let _ = write!(owner, " {}", channel.as_str());
                            }
                            if m.pinned.is_some() {
                                owner.push_str(" pinned");
                            }
                            if m.ignored {
                                owner.push_str(" ignored");
                            }
                            owner
                        }
                        None => "not managed".to_owned(),
                    };
                    let children = rows
                        .iter()
                        .filter(|r| r.child_of.as_deref() == Some(row.folder.as_str()))
                        .count();
                    let children = match children {
                        0 => String::new(),
                        n => format!(" (+{n} folders)"),
                    };
                    let _ = writeln!(out, "{}{children}  {}  {owner}", row.folder, row.version);
                }
            }
            Category::Downloader => {
                let _ = writeln!(out, "\n--- downloads ---");
                if self.queue.is_empty() {
                    let _ = writeln!(out, "nothing queued");
                }
                for download in &self.queue {
                    let stage = match &download.stage {
                        Stage::Queued => "queued".to_owned(),
                        Stage::Running { .. } => "running".to_owned(),
                        Stage::Paused(p) => {
                            format!("paused at {} of {}", size_of(p.downloaded_bytes), size_of(p.total_bytes))
                        }
                    };
                    let planned = download.job.total.map_or("not planned yet".to_owned(), size_of);
                    let _ = writeln!(out, "{} ({})  {stage}  job {planned}", download.label, download.code.as_str());
                }
                let _ = writeln!(out, "\n--- libraries ---");
                for library in &self.config.libraries {
                    let disk = library.disk().map_or("size unavailable".to_owned(), |d| {
                        format!("{} free of {}", size_of(d.free), size_of(d.total))
                    });
                    let _ = writeln!(
                        out,
                        "{} {}  {}  {:?}  {disk}",
                        library.id,
                        library.label,
                        library.games().display(),
                        library.state()
                    );
                }
            }
            Category::Settings => {
                // The contact is in the report already, where it belongs;
                // nothing else in the config is personal.
                let mut config = self.config.clone();
                config.contact = None;
                let _ = writeln!(out, "\n--- config.toml ---");
                match config.to_toml() {
                    Ok(toml) => out.push_str(&toml),
                    Err(e) => {
                        let _ = writeln!(out, "could not be rendered: {}", chain(&e));
                    }
                }
            }
            // The base bundle is already the game's own: build, Proton, DXVK,
            // the newest client log. Battle.net's log names the account, so
            // it stays on the machine; the launcher's own state is in the
            // recent activity.
            Category::Game | Category::BattleNet | Category::Launcher | Category::General => {}
        }
        lightforge_core::debug::scrub(&out)
    }

    /// The bug report and the feedback dialog, which are one card.
    // (`report_tags` sits below the impl, since it needs nothing from `self`.)
    ///
    /// They differ in three things: the prompt, whether the diagnostic bundle
    /// is shown, and whether it is sent. Everything else, the stages, the
    /// outcome line, the two buttons, is the same, and splitting them in two
    /// would mean maintaining that twice.
    fn report_card<'a>(&'a self, dialog: &'a ReportDialog) -> Element<'a, Message> {
        let (title, prompt) = match dialog.kind {
            ReportKind::Bug => ("Report a Bug", "What were you doing when this occurred?"),
            ReportKind::Feedback => {
                ("Send Feedback", "What would you like to see improved or changed?")
            }
        };
        // Editable right up until it is on its way: a report that failed, or
        // one the cooldown refused, is one somebody is about to change.
        let writing = !matches!(dialog.stage, ReportStage::Sending | ReportStage::Sent);

        let mut body = column![
            self.picker(
                "report category".to_owned(),
                report::Category::ALL.to_vec(),
                dialog.category,
                "What is it about?",
                Message::ReportCategory,
                Length::Fill,
                theme::BODY,
            ),
            // Five lines: enough that a paragraph looks invited rather than
            // tolerated, and the card still fits a laptop screen.
            // Pressing a field puts the typing in it: Steam's own keyboard
            // (Steam and X on a Deck) then writes there. Escape leaves the
            // field, and a second one closes the card.
            clickable(
                "report note",
                text_editor(&dialog.note)
                    .placeholder(prompt)
                    .on_action(Message::ReportNote)
                    .height(Length::Fixed(theme::BODY * 1.4 * 5.0 + theme::SM * 2.0))
                    .style(theme::editor)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
            // Required on a bug, and said so in the box itself: a bug nobody
            // can be asked about usually cannot be reproduced. Feedback can
            // stay anonymous, it rarely needs a reply.
            clickable(
                "report contact",
                text_input(
                    match dialog.kind {
                        ReportKind::Bug => "Email or Discord name, so I can reach you (required)",
                        ReportKind::Feedback => {
                            "Email or Discord name, so I can reach you (optional)"
                        }
                    },
                    &dialog.contact,
                )
                    .on_input_maybe(writing.then_some(Message::ReportContact))
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
        ]
        .spacing(theme::MD);

        body = match dialog.kind {
            // Shown in full, and shown before it is sent: the only way a
            // promise about what leaves the machine is checkable is if the
            // person can read the thing that leaves it.
            ReportKind::Bug => body
                .push(muted(
                    "This exact diagnostic data will be sent to my private server securely."
                        .to_owned(),
                    theme::LABEL,
                ))
                .push(
                    container(
                        scrollable(mono(dialog.debug.clone()))
                            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                            .height(Length::Fixed(200.0))
                            .width(Length::Fill),
                    )
                    .padding(theme::SM)
                    .style(theme::row),
                )
                .push(self.shot_row(dialog)),
            ReportKind::Feedback => body.push(muted(
                "This will be sent directly to me securely.".to_owned(),
                theme::LABEL,
            )),
        };

        if let Some((note, color)) = match dialog.stage {
            ReportStage::Sent => Some(("Sent — thank you.".to_owned(), theme::OK)),
            ReportStage::Failed => Some((
                "It could not be sent. Try again, or copy it and send it on Discord \
                 instead — closing this loses it."
                    .to_owned(),
                theme::ERROR,
            )),
            ReportStage::TooSoon => Some((
                "That exact report just went out. Add what changed, or close this.".to_owned(),
                theme::PLAY,
            )),
            _ => None,
        } {
            body = body.push(text(note).size(theme::LABEL).color(color));
        }

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        // Once it has arrived there is nothing left to cancel, so the card
        // offers one button that says what it does.
        let actions = match dialog.stage {
            ReportStage::Sent => row![
                Space::new().width(Length::Fill),
                tappable(
                    "close",
                    Some(Message::ReportCancel),
                    choice("Close").style(theme::filled(theme::ACCENT)),
                ),
            ],
            _ => row![
                Space::new().width(Length::Fill),
                // Only once there is something to lose. The screenshot is not
                // in it, the clipboard carries text, which the Discord
                // route solves on its own, since an image is pasted there.
                match dialog.stage {
                    ReportStage::Failed => tappable(
                        "copy report",
                        Some(Message::ReportCopy),
                        choice("Copy report").style(theme::outlined),
                    ),
                    _ => Element::from(Space::new().width(Length::Shrink)),
                },
                tappable("cancel", Some(Message::ReportCancel), choice("Cancel").style(theme::outlined)),
                tappable(
                    "send",
                    // Nothing is sent without a description, a category, and
                    // for a bug a way back: each one missing is an issue
                    // nobody can act on.
                    (writing && dialog.stage != ReportStage::TooSoon && dialog.can_send())
                        .then_some(Message::ReportSend),
                    choice(match dialog.stage {
                        ReportStage::Sending => "Sending…",
                        ReportStage::Failed => "Try again",
                        ReportStage::TooSoon => "Just sent",
                        _ => match dialog.kind {
                            ReportKind::Bug => "Send Bug Report",
                            ReportKind::Feedback => "Send Feedback",
                        },
                    })
                    .style(theme::filled(theme::ACCENT)),
                ),
            ],
        }
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(column![section(title, body.into()), actions].spacing(theme::LG))
            .width(Length::Fixed(560.0))
            .padding(theme::LG)
            .style(theme::dialog)
            .into()
    }

    /// The screenshot line of the bug report card.
    ///
    /// A button rather than a drop target: iced has no file dialog of its own,
    /// so this opens the desktop's through the XDG portal, which is also the
    /// only one that works when the launcher is sandboxed.
    fn shot_row<'a>(&self, dialog: &'a ReportDialog) -> Element<'a, Message> {
        let choose = |label: &'static str| {
            tappable(
                "screenshot",
                Some(Message::ReportShotPick),
                button(text(label).size(theme::LABEL))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::outlined),
            )
        };

        let Some(shot) = &dialog.shot else {
            return row![
                choose("Add a screenshot…"),
                muted("Optional, and it is sent with the report.".to_owned(), theme::LABEL),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into();
        };

        let (width, height) = shot.size;
        row![
            container(
                image(shot.preview.clone())
                    .height(Length::Fixed(72.0))
                    .content_fit(ContentFit::Contain),
            )
            .padding(theme::XS)
            .style(theme::row),
            column![
                text(shot.name.clone()).size(theme::LABEL),
                muted(format!("{width}×{height}, {}", size_of(shot.bytes.len() as u64)), theme::CAPTION),
            ]
            .spacing(theme::XS)
            .width(Length::Fill),
            choose("Replace…"),
            tappable(
                "remove screenshot",
                Some(Message::ReportShotCleared),
                button(text("Remove").size(theme::LABEL))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::outlined),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center)
        .into()
    }

    /// A product's name as the picker shows it, or its code.
    fn label_of(&self, code: &ProductCode) -> String {
        self.choices
            .iter()
            .find(|c| &c.code == code)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| code.as_str().to_owned())
    }

    /// Where the library lives, how full its disk is, and what is using it.
    fn storage_section(&self) -> Element<'_, Message> {
        let report = self.storage.as_ref();

        // ---- One card per drive.
        let mut drives = column![].spacing(theme::MD);
        match report {
            None => drives = drives.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                for library in &report.libraries {
                    drives = drives.push(self.drive_card(library));
                }
                drives = drives.extend(self.hint("The default drive is where a game goes unless you pick another one, and \
                     the folder Battle.net proposes for its own titles. It is only that: \
                     Battle.net itself and the Wine prefix stay where they were installed, \
                     above. Removing a drive takes it off this list and deletes nothing — \
                     add it again and Lightforge finds the games on it.").map(Element::from));
            }
        }

        // ---- Partitions that could become one.
        let mut add = column![].spacing(theme::SM);
        let offered: Vec<(usize, &library::Partition)> = self
            .partitions
            .iter()
            .enumerate()
            .filter(|(_, partition)| !partition.in_use)
            .collect();
        match offered.is_empty() {
            true => {
                add = add.push(muted(
                    "No other mounted partition Lightforge could write to.".to_owned(),
                    theme::LABEL,
                ))
            }
            false => {
                for (index, partition) in offered {
                    let free = match partition.disk {
                        Some(disk) => format!(
                            "{} free of {}",
                            size_of(disk.free),
                            size_of(disk.total)
                        ),
                        None => "size unknown".to_owned(),
                    };
                    add = add.push(
                        row![
                            container(
                                column![
                                    row![
                                        text(partition.label.clone()).size(theme::BODY),
                                        muted(partition.fstype.clone(), theme::CAPTION),
                                    ]
                                    .spacing(theme::SM)
                                    .align_y(Alignment::Center),
                                    muted(
                                        format!(
                                            "{} at {}",
                                            partition.device.display(),
                                            partition.point.display()
                                        ),
                                        theme::CAPTION,
                                    ),
                                ]
                                .spacing(2)
                            )
                            .width(Length::Fill),
                            muted(free, theme::LABEL),
                            container(secondary("Add", Message::AddDrive(index)).enabled(true))
                                .width(Length::Fixed(ACTION_COLUMN))
                                .align_x(Alignment::End),
                        ]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                    );
                }
            }
        }
        add = add.push(
            row![
                Space::new().width(Length::Fill),
                secondary("Rescan drives", Message::RefreshDrives).enabled(true),
            ]
            .align_y(Alignment::Center),
        );
        add = add.extend(self.hint("Lightforge creates a \"Lightforge\" folder on the partition and installs \
             games under it. Only mounted partitions you can write to are listed — mount the \
             drive the way you normally do and rescan. A partition is added, not a disk: a \
             drive with two partitions can hold two drives here.").map(Element::from));

        // ---- Lightforge's own directories, which follow XDG rather than the
        // drives, so they are listed apart rather than attributed to one.
        let mut usage = column![].spacing(theme::SM);
        match report {
            None => usage = usage.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                for item in &report.app {
                    let clearing = self.cache_notice.as_deref() == Some("clearing…");
                    let action = item.clearable.then(|| {
                        secondary("Clear", Message::ClearCache)
                            .enabled(item.bytes > 0 && !clearing)
                    });
                    usage = usage.push(usage_row(
                        column![
                            text(item.label.clone()).size(theme::BODY),
                            muted(item.path.display().to_string(), theme::CAPTION),
                        ]
                        .spacing(2)
                        .into(),
                        item.bytes,
                        action,
                    ));
                }
                usage = usage.extend(self.hint("Clearing the cache removes downloaded art, installers and lists; each is \
                     fetched again when needed. Shader caches are kept — rebuilding them is \
                     what makes a game stutter.").map(Element::from));
                if let Some(notice) = &self.cache_notice {
                    usage = usage.push(muted(notice.clone(), theme::LABEL));
                }
                if self.storage_measuring {
                    usage = usage.push(muted("updating…".to_owned(), theme::CAPTION));
                }
            }
        }

        column![
            section("Drives", drives.into()),
            Space::new().height(Length::Fixed(theme::MD)),
            section("Add a drive", add.into()),
            Space::new().height(Length::Fixed(theme::MD)),
            section("Lightforge's own files", usage.into()),
        ]
        .spacing(theme::SM)
        .into()
    }

    /// One drive's card: how full it is, what Lightforge put there, and the
    /// three things you can do to it.
    fn drive_card<'a>(&'a self, library: &'a storage::LibraryUsage) -> Element<'a, Message> {
        let id = library.id.clone();
        let mut card = column![].spacing(theme::SM);

        let mut title = row![text(library.label.clone()).size(theme::HEADING)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        if library.is_default {
            title = title.push(
                container(text("Default").size(theme::CAPTION).color(theme::ACCENT))
                    .padding([2.0, theme::SM])
                    .style(theme::tag(theme::ACCENT)),
            );
        }
        if let Some(reason) = library.state.reason(&library.label) {
            title = title.push(
                container(text(reason).size(theme::CAPTION).color(theme::ERROR))
                    .padding([2.0, theme::SM])
                    .style(theme::tag(theme::ERROR)),
            );
        }
        title = title.push(Space::new().width(Length::Fill));
        title = title.push(mono(format!(
            "{} → games   {} → wow",
            library.letters.0.to_uppercase(),
            library.letters.1.to_uppercase(),
        )));
        card = card.push(title);
        card = card.push(muted(library.path.display().to_string(), theme::CAPTION));

        match library.disk {
            Some(disk) => {
                let used = disk.total.saturating_sub(disk.free);
                let fraction = match disk.total {
                    0 => 0.0,
                    total => used as f32 / total as f32,
                };
                card = card
                    .push(progress_bar(0.0..=1.0, fraction).girth(6))
                    .push(muted(
                        format!(
                            "{} free of {} — {} is Lightforge's",
                            size_of(disk.free),
                            size_of(disk.total),
                            size_of(library.total()),
                        ),
                        theme::CAPTION,
                    ));
            }
            // A drive that is not here is not measured at all: walking its
            // mount point would report whatever filesystem is *under* it as
            // if the library's games were on it.
            None => {
                card = card.push(muted(
                    "Nothing is measured while the drive is away. The games on it are \
                     untouched, and come back with it."
                        .to_owned(),
                    theme::CAPTION,
                ))
            }
        }
        // Only where it is actually true: a library can be unclaimed and still
        // hold games, which is every install from before multi-drive storage,
        // and the drive holding the prefix always has Battle.net's own titles
        // on it.
        if library.state == library::State::New && library.total() == 0 {
            card = card.push(muted(
                "Nothing installed here yet. The folder is created with the first game.".to_owned(),
                theme::CAPTION,
            ));
        }

        for item in &library.usage {
            // Nothing is a fact better told by absence than by a row of
            // zeroes with a dead button beside it.
            if item.bytes == 0 {
                continue;
            }
            let notice = self
                .drive_notice
                .as_ref()
                .filter(|(which, _)| *which == library.id)
                .map(|(_, note)| note.clone());
            let clearing = notice.as_deref() == Some("clearing…");
            let action = item.clearable.then(|| {
                secondary("Clear", Message::ClearDrive(id.clone()))
                    .enabled(item.bytes > 0 && !clearing)
            });
            card = card.push(usage_row(
                text(item.label.clone()).size(theme::BODY).into(),
                item.bytes,
                action,
            ));
        }
        if let Some((which, note)) = &self.drive_notice {
            if *which == library.id {
                card = card.push(muted(note.clone(), theme::LABEL));
            }
        }
        card = card.extend(self.unused_rows(library));

        let mut actions = row![Space::new().width(Length::Fill)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        // `is_present` rather than `is_ready`: a drive with nothing of ours on
        // it yet can still be opened and can still be made the default, it is
        // set up by the first install, not before. Only a drive that is *away*
        // can do neither.
        actions = actions.push(
            secondary("Open folder", Message::OpenDrive(id.clone()))
                .enabled(library.state.is_present()),
        );
        actions = actions.push(
            secondary("Make default", Message::SetDefaultDrive(id.clone()))
                .enabled(!library.is_default && library.state.is_present()),
        );
        // The default drive cannot be removed while another exists, make a
        // different one the default first, and the last one cannot be
        // removed at all, since every path resolves through it.
        actions = actions.push(
            secondary("Remove", Message::RemoveDrive(id.clone()))
                .enabled(!library.is_default && self.config.libraries.len() > 1),
        );
        card = card.push(actions);

        container(card).padding(theme::MD).width(Length::Fill).style(theme::row).into()
    }

    /// A drive card's "Unused game data" row and the line under it.
    fn unused_rows<'a>(&'a self, library: &'a storage::LibraryUsage) -> Vec<Element<'a, Message>> {
        let label = || text("Unused game data").size(theme::BODY).into();
        let Some(state) = self.unused.get(&library.id) else {
            return Vec::new();
        };
        match state {
            UnusedData::Checking => vec![muted("checking for unused game data…".to_owned(), theme::CAPTION).into()],
            // Nothing worth a button: told by absence, like an empty cache.
            UnusedData::Ready(plan) if plan.freeable < UNUSED_WORTH_SHOWING => Vec::new(),
            UnusedData::Ready(plan) => {
                let busy = self
                    .config
                    .libraries
                    .iter()
                    .find(|l| l.id == library.id)
                    .is_some_and(|l| self.downloading_into(&self.layout.wow_install_in(l)));
                let held = self.store_writer().is_some() || self.launching.is_some();
                let action =
                    secondary("Free up", Message::FreeUnused(library.id.clone())).enabled(!busy && !held);
                let note = match busy {
                    true => "Finish or cancel the download on this drive first.".to_owned(),
                    false if plan.moved > 0 => format!(
                        "Left behind by patches and versions that are gone. {} of game data \
                         still in use is copied first. The game and Battle.net have to be closed.",
                        size_of(plan.moved)
                    ),
                    false => "Left behind by patches and versions that are gone. The game and \
                              Battle.net have to be closed."
                        .to_owned(),
                };
                vec![
                    usage_row(label(), plan.freeable, Some(action)),
                    muted(note, theme::CAPTION).into(),
                ]
            }
            UnusedData::Freeing { copied, total } => {
                let done = copied.load(std::sync::atomic::Ordering::Relaxed).min(*total);
                vec![muted(
                    format!("freeing unused game data… {} of {} copied", size_of(done), size_of(*total)),
                    theme::LABEL,
                )
                .into()]
            }
            UnusedData::Said(note) => vec![muted(note.clone(), theme::LABEL).into()],
        }
    }

    /// The version, and where its configuration lives.
    fn about_section(&self) -> Element<'_, Message> {
        let mut body = column![field_row(
            "Version".to_owned(),
            mono(env!("CARGO_PKG_VERSION").to_owned()).into(),
        )]
        .spacing(theme::SM);
        if let Some(dirs) = &self.dirs {
            body = body.push(field_row(
                "Config file".to_owned(),
                mono(dirs.config_file().display().to_string()).into(),
            ));
        }
        body.into()
    }
}

/// How wide a centred message card gets, the broken-config screen. A measure
/// rather than the full window: this one is prose, and prose set across a wide
/// monitor is unreadable.
const MESSAGE_WIDTH: f32 = 720.0;

/// Width of the size column on the Storage page.
///
/// Fixed so every figure right-aligns to the same edge. Left to shrink, a row
/// with a Clear button would push its size left of the rows without one, and a
/// column of numbers that does not line up is the thing that makes a table
/// look unfinished.
const SIZE_COLUMN: f32 = 96.0;

/// Width of the action slot beside a size, kept even where there is no button.
const ACTION_COLUMN: f32 = 76.0;

/// One "name, size, maybe a button" line, as the Storage page draws them.
fn usage_row<'a>(
    name: Element<'a, Message>,
    bytes: u64,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        container(name).width(Length::Fill),
        container(mono(size_of(bytes)).align_x(Alignment::End)).width(Length::Fixed(SIZE_COLUMN)),
        container(action.unwrap_or_else(|| Space::new().width(Length::Shrink).into()))
            .width(Length::Fixed(ACTION_COLUMN))
            .align_x(Alignment::End),
    ]
    .spacing(theme::MD)
    .align_y(Alignment::Center)
    .into()
}

/// The optional downloads Lightforge offers to switch on.
///
/// This lists what is *shown*, not what is gated: every optional tag a build
/// carries stays off unless opted in, named here or not, so a tag Blizzard
/// adds tomorrow is still excluded, hardcoding the gate is what once put a
/// 37 GiB texture pack in every plan. `Alternate` and `Talebound` exist in
/// the manifests but are not something to offer a player a switch for.
const OFFERED_OPTIONAL: &[&str] = &[HIGH_RES];

/// The high-resolution texture pack's manifest tag.
const HIGH_RES: &str = "HighRes";

/// The tags in `tags` that [`OFFERED_OPTIONAL`] names.
fn offered_only(tags: &[String]) -> Vec<String> {
    tags.iter().filter(|tag| OFFERED_OPTIONAL.contains(&tag.as_str())).cloned().collect()
}

/// A human label for an optional-download tag.
///
/// The manifest's own name is the fallback, for anything added to
/// [`OFFERED_OPTIONAL`] before it gets a phrase.
fn optional_label(tag: &str) -> String {
    match tag {
        "HighRes" => "High-resolution textures".to_owned(),
        other => other.to_owned(),
    }
}

/// Secondary text at a given size.
/// `~/Games/Blizzard` rather than `/home/ann/Games/Blizzard`.
///
/// Only the home prefix, and only when it is one: a library on
/// `/run/media/ann/ssd` is left exactly as it is, because every part of that
/// path tells the user which drive they are looking at. Unlike
/// `core::debug::scrub`, the account name is not hidden; this is the user's
/// own screen and their own path.
fn home_relative(path: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn muted<'a>(content: String, size: f32) -> iced::widget::Text<'a> {
    text(content).size(size).color(theme::MUTED)
}

/// Centre a line of text on its *letters* rather than on its line box.
///
/// A line box is ascent plus descent plus leading, so centring it puts the
/// empty descent space, the room for a `g` or a `y`: below the letters and
/// lifts them above the middle. On a word that has no descenders at all, like
/// `GAMES` or `Battle.net`, every bit of that space is empty and the word
/// reads as sitting high beside something that fills its box, such as a tile.
///
/// Collapsing the line height to the glyphs removes the leading, and the
/// remaining half-descent is taken by a pixel of padding above. Both are of
/// the text's own size, so this holds at any size rather than being a nudge
/// tuned to one.
fn optically_centred<'a>(text: iced::widget::Text<'a>) -> Element<'a, Message> {
    container(text.line_height(iced::widget::text::LineHeight::Relative(1.0)))
        // Top only. Padding on both sides would grow the box symmetrically and
        // move the letters nowhere.
        .padding(iced::Padding { top: OPTICAL_NUDGE, right: 0.0, bottom: 0.0, left: 0.0 })
        .into()
}

/// Shortest query worth sending to the providers while typing.
const SEARCH_MIN: usize = 2;

/// How long the search box waits for the next keystroke before going out.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

/// How far down a descenderless line is pushed to sit on the optical middle.
/// Roughly a quarter of the font's descent at label sizes; see
/// [`optically_centred`].
const OPTICAL_NUDGE: f32 = 2.0;

/// The key that reveals the hints, with Control, Alt and Shift held.
///
/// Not a setting and not in any menu: the prose it brings back is written for
/// whoever is building Lightforge, and a user who found it by accident would
/// be reading notes meant for someone else. It lasts as long as the window,
/// so nothing on disk records that it was ever pressed.
const HINTS_CHORD: &str = "h";

/// Whether a key press is the chord that reveals the hints.
///
/// All three modifiers, deliberately: Control and Shift alone are one slip
/// away from shortcuts people use, and the prose behind this is not something
/// a user should meet by fumbling a paste.
fn hints_chord(key: &iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> bool {
    matches!(key.as_ref(), iced::keyboard::Key::Character(HINTS_CHORD))
        && modifiers.control()
        && modifiers.alt()
        && modifiers.shift()
}

/// Height of the game band's own row.
///
/// Pinned rather than left to the tiles: the band fills its cross axis, and a
/// widget that does that is handed the height of its siblings, the buttons,
/// which are shorter than a tile. The padding is [`theme::SM`] rather than
/// `XS` now that the row is the band's alone, so the tiles sit in it instead
/// of filling it edge to edge: [`theme::LG`] of air, split above and below.
fn strip_height() -> f32 {
    theme::tile() + theme::LG
}

/// Width of a band button, pinned so the band's arithmetic is exact rather
/// than an estimate of how wide a glyph renders.
const BAND_BUTTON: f32 = 48.0;

/// How far a held tile has to travel before it is a drag rather than a click
/// with a shaky hand.
const DRAG_SLOP: f32 = 6.0;

/// How many frames whatever a key press opens or closes is given to be drawn
/// before the ring stops waiting to follow it.
const FOCUS_ENTER_FRAMES: u8 = 10;

/// The scrolled page beside the Settings list, as a focus region.
const SETTINGS_PAGE: &str = "settings";

/// How far an arrow with nothing to move to scrolls the page instead.
const FOCUS_SCROLL: f32 = 160.0;

/// Ask again in a frame's time whether a menu the keyboard opened is drawn.
fn focus_settle() -> Task<Message> {
    // Made inside the future: a timer needs the runtime it will run on.
    Task::perform(async { tokio::time::sleep(Duration::from_millis(16)).await }, |()| {
        Message::FocusSettle
    })
}

/// A tile held down in the band.
///
/// `origin` is unknown at the press, which carries no position, and is taken
/// from the first movement after it. `last` is where the cursor was on the
/// previous movement, so a move can tell which way it is heading.
struct TileDrag {
    line: ProductLine,
    origin: Option<f32>,
    last: f32,
    moving: bool,
}

/// Battle.net's own mark on the button at the end of the band.
///
/// Larger than [`BAND_GLYPH`], which sizes marks that sit *inside* a band
/// control: this one sits beside a word at the end of the row of 48pt tiles,
/// and a 16pt mark there reads as a favicon rather than the last entry in a
/// row of games. Half a tile is the size that makes the button a peer of what
/// it stands next to without becoming a tile itself.
const BNET_GLYPH: f32 = BAND_BUTTON / 2.0;

/// Mark in front of one of the three links under the action panel's status
/// lines. A shade over the 12pt word it introduces, so the pair sits on one
/// optical line rather than the mark hanging below the text.
const LINK_GLYPH: f32 = 14.0;

/// Mark beside a label in the app menu, and the social squares' mark.
const MENU_GLYPH: f32 = 16.0;

/// The arrow on a tab that opens a browser. Smaller than [`MENU_GLYPH`]
/// because this one is trimmed to its own ink, the way the band's marks are:
/// it is read beside a word rather than down a column, so the box is the mark
/// and the number is the size it actually draws. 8 sits just under the cap
/// height of [`TAB_LABEL`] bold: a mark on a word, not a second word.
const OUTBOUND_GLYPH: f32 = 8.0;

/// How tall the wordmark is drawn in the top bar. Above [`theme::TITLE`]:
/// the mark is artwork rather than a line of type, and the hammer between the
/// two words carries detail that a headline's cap height loses.
const WORDMARK_HEIGHT: f32 = 28.0;

/// The navigation tabs' type. Deliberately small beside a wordmark of
/// [`WORDMARK_HEIGHT`]: the mark is the row's voice and the tabs are its
/// index, and setting them anywhere near the mark's size gives the row two
/// things shouting at once. The air around them, not their size, is what
/// keeps them readable; see [`TAB_GAP`].
const TAB_LABEL: f32 = theme::LABEL;

/// The air between two tabs in the navigation row.
///
/// All of it lives here rather than in the tabs' own padding, and that is the
/// point. A tab's padding is what the selected pill is drawn around, so air
/// spent there is air you see on three sides of "Play" and nowhere beside
/// "News", which is why the gap to "Battle.net Shop" used to look wider than
/// the gap back to "Play" although both measured the same. Keeping the tabs'
/// padding at the minimum a pill needs leaves the two gaps within a few
/// pixels of each other.
const TAB_GAP: f32 = theme::LG;

/// Extra air on top of the row's own spacing where one group of controls in
/// the navigation row ends and the next begins, wordmark, then the arrows,
/// then the pages. It only has to say "different kind of thing"; the groups
/// are already told apart by what they look like, so a gap wide enough to
/// read as a break is wider than the job needs.
const GROUP_GAP: f32 = theme::XS;

/// The wordmark's width over its height. The picture is trimmed to its own
/// ink, so the ratio is fixed by the art: scaling by it is what keeps the
/// hammer round instead of squeezed.
const WORDMARK_ASPECT: f32 = 563.0 / 72.0;

/// The wordmark as drawn, from its height and the artwork's aspect.
const WORDMARK_WIDTH: f32 = WORDMARK_HEIGHT * WORDMARK_ASPECT;

/// The chevron beside the wordmark, the app menu's own arrow.
const APP_MENU_CHEVRON: f32 = BAND_GLYPH * 0.75;

/// Width of the Settings sidebar.
///
/// Derived rather than picked: it is whatever puts the rule beside it directly
/// under the chevron on the wordmark above. That chevron is the only vertical
/// mark in the header, so a rule a few pixels off it reads as a mistake where
/// one exactly under it reads as intent.
///
/// ```text
/// chevron centre = LG (header padding) + SM (button padding)
///                + wordmark + SM (gap) + half the chevron
/// rule           = LG (row padding)    + this + LG (row spacing)
/// ```
///
/// The leading `LG`s cancel, which leaves the rest.
const SETTINGS_NAV_WIDTH: f32 =
    theme::SM + WORDMARK_WIDTH + theme::SM + APP_MENU_CHEVRON / 2.0 - theme::LG;

/// Air between that label and its arrow.
///
/// Tighter than [`theme::XS`] on purpose. Every tab in the navigation row is
/// separated by the same [`TAB_GAP`], but only this one carries anything after
/// its label, and that padding is *inside* the button, so air there widens
/// the space between "News" and "Battle.net Shop" without the row's spacing
/// changing at all. Keeping the arrow against its word is what makes the two
/// gaps match by eye as well as in the arithmetic.
const OUTBOUND_GAP: f32 = 2.0;

/// How far a control that has nowhere to go is faded: visibly present, plainly
/// not available. One value so every dimmed thing dims by the same amount.
const DIMMED: f32 = 0.35;

/// How many status lines a bug report carries.
///
/// Twenty is about a minute of a busy launcher and three screens of a quiet
/// one, enough to show what led up to the problem, short enough that nobody
/// has to scroll the card to read what they are sending.
const HISTORY: usize = 20;

/// Where the app menu's links point.
///
/// `None` is a destination that does not exist yet, and the entry draws itself
/// dimmed and unclickable rather than opening a browser on nothing. Fill one
/// in and that entry comes alive, no other change is needed.
/// Blizzard's own shop, which redirects to the viewer's region on its own, so
/// one address serves every account without Lightforge guessing at a locale.
///
/// One outbound mark as a square button, Discord and X.
///
/// Shared by the logo menu and the first run's last slide, under the same rule
/// as the menu's entries: no destination yet means no press and a dimmed face,
/// so the square says what is coming without pretending it is here.
fn link_square(
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

const SHOP_URL: &str = "https://shop.battle.net";

/// Asks a running Steam to show its on-screen keyboard.
const STEAM_KEYBOARD: &str = "steam://open/keyboard";
/// And to put it away: the pair SDL uses, found in Steam's own libSDL3.
const STEAM_KEYBOARD_CLOSE: &str = "steam://close/keyboard";

/// Whether Steam started this launcher, which it says by setting
/// `SteamGameId` for everything it launches, a non-Steam shortcut included.
/// Only then is there a Steam to ask for its keyboard: anywhere else
/// [`STEAM_KEYBOARD`] is just a URL, and opening it starts Steam. Measured
/// 2026-10-17, A on a text field in a launcher started from the desktop
/// brought up the real Steam client with every library on the machine.
fn launched_by_steam() -> bool {
    std::env::var_os("SteamGameId").is_some_and(|id| !id.is_empty())
}

/// The A and B on the console's button hints.
const HINT_GLYPH: f32 = 22.0;

/// Where the News page's headlines come from, and where its button goes.
const WOWHEAD_URL: &str = "https://www.wowhead.com";

const DISCORD_URL: Option<&str> = Some("https://discord.gg/vtTgmUV9Bq");
const X_URL: Option<&str> = Some("https://x.com/simplyKennybtw");
/// Where someone can say thanks. Offered, never asked for: these sit beside
/// the places to get help rather than in front of anything.
const KOFI_URL: Option<&str> = Some("https://ko-fi.com/simplykenny");
const PAYPAL_URL: Option<&str> = Some("https://paypal.me/simplykenny");

/// The handful of facts worth filtering a pile of issues by.
///
/// Every one of them is already in the rendered bundle the card shows, so this
/// sends nothing new, it only puts them where GlitchTip can group on them,
/// which reading a text blob per issue does not allow.
fn report_tags(info: &DebugInfo) -> Vec<(String, String)> {
    let mut tags = vec![
        ("lightforge".to_owned(), info.app_version.clone()),
        ("product".to_owned(), info.product.clone()),
        ("state".to_owned(), info.action_state.clone()),
        ("os".to_owned(), info.os.clone()),
        ("kernel".to_owned(), info.kernel.clone()),
    ];
    for (key, value) in [
        ("version", &info.build_version),
        ("proton", &info.proton),
        ("session", &info.session),
        ("gpu", &info.graphics),
        ("gpu_driver", &info.gpu_driver),
    ] {
        if let Some(value) = value {
            tags.push((key.to_owned(), value.clone()));
        }
    }
    tags
}

/// A product's mark in the Add a game list.
///
/// Smaller than the band's [`theme::TILE`]: there the mark *is* the control,
/// here it labels a row whose height belongs to two lines of text, and a mark
/// taller than those two lines would set the row's height instead of fitting
/// inside it.
const BROWSE_MARK: f32 = theme::TILE * 0.6;

/// How big a mark on a band button is drawn: the box it sits in is
/// [`BAND_BUTTON`] less its padding, and a mark wider than that would push the
/// button out of the size the band's arithmetic assumes.
const BAND_GLYPH: f32 = BAND_BUTTON - theme::MD * 2.0;

/// Add a game, and the overflow button beside it: one mark in a fixed box, so
/// the two match and [`BAND_BUTTON`] stays true.
fn band_button<'a>(
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

/// Which tiles the band shows, and which go behind the overflow button.
///
/// Pure, so the rules can be tested without a window: `widths` is what each
/// tile needs, `room` what the band has for tiles, and the answer is indices
/// into `widths`: shown first, hidden second, both in the order given.
///
/// Nothing is hidden while everything fits. Once something must be, the
/// overflow button costs room too, and the selected line is never what
/// disappears: it is the game the page is showing, so a band that dropped it
/// would stop saying where you are.
fn divide_strip(widths: &[f32], selected: usize, room: f32) -> (Vec<usize>, Vec<usize>) {
    let gap = theme::SM;
    let span = |ids: &[usize]| -> f32 {
        ids.iter().map(|id| widths[*id]).sum::<f32>()
            + gap * ids.len().saturating_sub(1) as f32
    };

    let all: Vec<usize> = (0..widths.len()).collect();
    if all.is_empty() || span(&all) <= room {
        return (all, Vec::new());
    }

    let budget = room - BAND_BUTTON - gap;
    let mut shown: Vec<usize> = Vec::new();
    for id in 0..widths.len() {
        let mut next = shown.clone();
        next.push(id);
        if span(&next) > budget {
            break;
        }
        shown = next;
    }

    // The selected line takes the place of the last tile that fit, and of the
    // one before that if it still does not fit. A band showing only the
    // selected line is the floor: something is always lit.
    if !shown.contains(&selected) {
        shown.pop();
        while !shown.is_empty() {
            let mut with_selected = shown.clone();
            with_selected.push(selected);
            if span(&with_selected) <= budget {
                break;
            }
            shown.pop();
        }
        shown.push(selected);
    }

    let hidden = all.iter().copied().filter(|id| !shown.contains(id)).collect();
    (shown, hidden)
}

/// Machine text, versions, paths, ids. Monospaced so a version number cannot
/// be mistaken for prose, and so columns line up without a table.
fn mono<'a>(content: String) -> iced::widget::Text<'a> {
    text(content)
        .size(theme::CAPTION)
        .font(theme::MONO)
        .color(theme::MUTED)
}

/// A status pill.
/// Draw one block of an addon's description or changelog.
///
/// The proxy has already reduced the HTML to these few shapes (see
/// `lightforge_core::addons::richtext`), so this is the whole renderer. An
/// unrecognised kind draws as a paragraph on purpose: the contract is that
/// the proxy can add one without a client release.
fn rich_block(block: &richtext::Block) -> Element<'_, Message> {
    use richtext::Kind;

    match &block.kind {
        Kind::Heading => {
            // Two sizes, not six. A changelog's h1 and h3 mean "version" and
            // "section", and honouring all six levels on a 620px card just
            // makes the deep ones indistinguishable from body text.
            let size = match block.level {
                0..=2 => theme::HEADING,
                _ => theme::BODY,
            };
            container(text(block.text.clone()).size(size)).padding([theme::XS, 0.0]).into()
        }

        Kind::Bullet => row![
            // The marker is its own text rather than part of the string so
            // that a wrapped bullet hangs under its text and not under the
            // dot.
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

        // Deliberately not drawn. Showing them means fetching and decoding
        // remote images into handles, which is the News page's whole image
        // cache over again for a card that is open for ten seconds; the
        // description's words carry the meaning and the page has the pictures.
        Kind::Image => Space::new().into(),

        Kind::Para | Kind::Other(_) => rich_paragraph(block),
    }
}

/// A block's text with its links clickable.
///
/// The link offsets come from the proxy precisely so this can cut the string
/// without searching it, a search picks the wrong occurrence as soon as one
/// block links the same words twice, which changelogs do constantly.
fn rich_paragraph(block: &richtext::Block) -> Element<'_, Message> {
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
/// Padded exactly like [`action_button`] and [`secondary`], for the same
/// reason those two match each other: side by side, a control that disagrees
/// about its height reads as a mistake.
fn tag<'a>(content: String, color: iced::Color) -> Element<'a, Message> {
    // Never wrapped. A tag is a mark at the size of the control beside it, and
    // a two-line one makes its whole row taller than every other row on the
    // page. Keep the words few enough to fit instead.
    container(
        text(content)
            .size(theme::LABEL)
            .wrapping(iced::widget::text::Wrapping::None),
    )
        .padding([theme::SM, theme::MD])
        .style(theme::tag(color))
        .into()
}

fn pill<'a>(content: String, color: iced::Color) -> Element<'a, Message> {
    container(text(content).size(theme::CAPTION))
        .padding([2.0, theme::SM])
        .style(theme::badge(color))
        .into()
}

/// An empty state: what happened, and what to do about it.
fn empty<'a>(headline: &str, hint: &str) -> Element<'a, Message> {
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
struct Action<'a> {
    inner: button::Button<'a, Message>,
    message: Message,
}

impl<'a> Action<'a> {
    /// The button, pressable or not, and a place for the selection ring
    /// named by what it sends, which is unique wherever it is drawn.
    fn enabled(self, enabled: bool) -> Element<'a, Message> {
        let key = format!("{:?}", self.message);
        self.keyed(key, enabled)
    }

    /// [`Action::enabled`] under a name of its own, for a button whose
    /// message changes when pressed, Add turning into Remove: named by what
    /// it sends, the ring would lose it the moment Enter landed.
    fn keyed(self, key: String, enabled: bool) -> Element<'a, Message> {
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

    fn width(mut self, width: f32) -> Self {
        self.inner = self.inner.width(Length::Fixed(width));
        self
    }
}

fn action_button(label: &str, color: iced::Color, message: Message) -> Action<'static> {
    Action {
        inner: button(text(label.to_owned()).size(theme::tap_text(theme::LABEL)))
            .style(theme::filled(color))
            .padding(theme::tap([theme::SM, theme::MD])),
        message,
    }
}

/// The quieter button: same size as [`action_button`], different colour.
///
/// Padded identically on purpose. These two sit side by side all over the app,
/// Page next to Install, Keep next to Delete, and a pair of buttons that
/// disagree about their height reads as a mistake rather than as a hierarchy.
/// What separates them is colour, which is the thing that carries meaning.
fn secondary(label: &str, message: Message) -> Action<'static> {
    Action {
        inner: button(text(label.to_owned()).size(theme::tap_text(theme::LABEL)))
            .style(theme::outlined)
            .padding(theme::tap([theme::SM, theme::MD])),
        message,
    }
}

/// A heading over a hairline, then its body, Settings' unit of grouping.
fn section<'a>(title: impl Into<String>, body: Element<'a, Message>) -> Element<'a, Message> {
    column![
        text(title.into()).size(theme::HEADING),
        container(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(theme::divider),
        body,
    ]
    .spacing(theme::MD)
    .into()
}

/// A fixed-width label beside its control, so every control in a section
/// starts on the same line.
/// Width of each fact column on an addon row. One number, so the header and
/// the rows cannot drift apart.
const ADDON_COLUMN: f32 = 92.0;

/// Width reserved for a row's controls, so the columns beside them line up
/// with each other and with the header.
///
/// Measured against the widest a row gets: the channel picker (a fixed 120),
/// an Update or Switch button, the two-button armed Remove (Delete beside
/// Keep, which is wider than Remove alone), and the menu glyph, with
/// [`theme::SM`] between each.
const ADDON_ACTIONS: f32 = 380.0;

/// One fact in an addon row's column, or a dash where there is none.
fn cell<'a>(value: Option<String>) -> Element<'a, Message> {
    match value {
        Some(text) => muted(text, theme::CAPTION),
        None => muted("—".to_owned(), theme::CAPTION),
    }
    .width(Length::Fixed(ADDON_COLUMN))
    .into()
}

/// A column heading over those cells.
fn column_head<'a>(label: &str) -> Element<'a, Message> {
    muted(label.to_owned(), theme::CAPTION)
        .width(Length::Fixed(ADDON_COLUMN))
        .into()
}

/// `2026-10-04T20:10:34.237Z` -> `4 Oct`.
///
/// Day and month only: the year is almost always this one, and a column wide
/// enough for a full timestamp is a column that costs the addon's name.
fn short_day(timestamp: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = timestamp.split('T').next().unwrap_or(timestamp);
    let mut parts = date.split('-');
    let (Some(_year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        // Not a shape we know. Showing it raw is more use than hiding it.
        return timestamp.to_owned();
    };
    match (month.parse::<usize>(), day.parse::<u32>()) {
        (Ok(m), Ok(d)) if (1..=12).contains(&m) => format!("{d} {}", MONTHS[m - 1]),
        _ => timestamp.to_owned(),
    }
}

/// The day out of an RFC 3339 timestamp, which is all a build list needs.
///
/// Not parsed into a date type: the string is the provider's, the only part
/// anyone reads is the day, and a malformed one should show as itself rather
/// than disappear behind an error.
fn day_of(timestamp: &str) -> String {
    timestamp.split('T').next().unwrap_or(timestamp).to_owned()
}

/// A channel as a `pick_list` option; the widget needs `Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChannelChoice(Channel);

impl std::fmt::Display for ChannelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.label())
    }
}

/// One addon's channel: its own, or whatever the default happens to be.
///
/// "Default" is a real option rather than a blank, because following the
/// default and choosing stable look identical until the default changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowChannel(Option<Channel>);

impl RowChannel {
    const ALL: [Self; 4] = [
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

fn field_row<'a>(label: String, control: Element<'a, Message>) -> Element<'a, Message> {
    row![muted(label, theme::LABEL).width(Length::Fixed(160.0)), control]
        .spacing(theme::MD)
        .align_y(Alignment::Center)
        .into()
}

/// A checkbox, with a bigger box and a little more air beside it on a
/// console, where a thumb is what presses it.
fn tick<'a>(on: bool) -> iced::widget::Checkbox<'a, Message> {
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
fn clickable<'a>(key: impl Into<String>, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    focus::ring(focus::Target::Key(key.into()), focus::Press::Click, content)
}

/// A button inside a [`clickable`]-style ring, drawn on the button's edge.
///
/// A button's ring sends its message itself rather than click it: iced's
/// button ignores a press once the event carrying it is captured, and the
/// made-up click rides whatever event comes next, which on a page with text
/// fields often is. Measured 2026-10-17: the bug report's Cancel and Send
/// and the environment variables' Remove never pressed.
fn tappable<'a>(
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
fn dot<'a>() -> Element<'a, Message> {
    text("·").size(theme::CAPTION).color(theme::MUTED).into()
}

/// Place a transfer sample within its job; see [`Job`].
fn job_progress(job: Job, sample: &TransferState, bytes_per_second: u64) -> state::Progress {
    let stored = sample.progress.bytes_stored;
    let (downloaded, total) = match (job.total, sample.remaining) {
        // Whatever the job planned that this run no longer has to fetch is
        // already done.
        (Some(total), Some(remaining)) => {
            ((total.saturating_sub(remaining) + stored).min(total), total)
        }
        // Resumed and still planning.
        (Some(total), None) => (job.reached.min(total), total),
        // A first run before its plan: nothing to measure against yet.
        (None, _) => (stored, sample.progress.bytes_total),
    };
    state::Progress { downloaded_bytes: downloaded, total_bytes: total, bytes_per_second }
}

/// Progress carried by the two transfer states, if this is one of them.
/// A repeating 0→1 sweep for work whose length is unknown.
///
/// iced's progress bar has no indeterminate mode, and a moving segment would
/// need a widget of its own; a fill that restarts is the older idiom for the
/// same thing and reads correctly, something is happening, nobody knows for
/// how long. Driven straight off the clock rather than off a counter in the
/// model because the 100 ms tick already redraws the window whenever a
/// transfer is live, which is exactly when this is on screen.
/// The splash's spinner: a ring with a bright arc travelling round it.
///
/// A canvas rather than a widget, because iced has neither a spinner nor an
/// indeterminate progress bar and an arc is two paths. Both the angle and the
/// arc's length come straight off the clock, like [`sweep`] and for the same
/// reason: the 100 ms tick already redraws the window while the splash is up.
///
/// The arc grows and shrinks as it turns rather than running at a fixed
/// length. A constant arc at a constant speed reads as a rigid object going
/// round; one that breathes reads as work being done, which is the only thing
/// this is here to say.
struct Spinner;

impl Spinner {
    /// How long one full turn takes.
    const PERIOD: f32 = 1.5;
    /// How thick the ring is drawn.
    const STROKE: f32 = 3.0;
    /// The shortest and longest the travelling arc gets, in turns.
    const SHORTEST: f32 = 0.08;
    const LONGEST: f32 = 0.42;

    fn phase() -> f32 {
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

        // The track it runs on, so the ring is a ring even where the arc is
        // not, without it the arc reads as a stray mark rather than as
        // something going round something.
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

/// One release's notes as a bulleted list.
///
/// Shared by the popup and the Lightforge Updates page so the two cannot
/// drift. The bullet is a character rather than a drawn mark: these are lines
/// of prose, not controls.
fn release_notes(release: &lightforge_core::releases::Release) -> Element<'static, Message> {
    // Prose, so a little more leading than a label gets: a note that wraps
    // should not read as two notes run together.
    let leading = iced::widget::text::LineHeight::Relative(1.45);
    column(release.notes.iter().map(|note| {
        row![
            text("·").size(theme::BODY).color(theme::MUTED).line_height(leading),
            text(note.clone()).size(theme::BODY).line_height(leading).width(Length::Fill),
        ]
        .spacing(theme::SM)
        .into()
    }))
    .spacing(theme::SM)
    .into()
}

fn sweep() -> f32 {
    const PERIOD: f32 = 1.4;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    (elapsed % PERIOD) / PERIOD
}

fn progress_of(action: &ActionState) -> Option<state::Progress> {
    match action {
        ActionState::Downloading(p) | ActionState::Paused(p) => Some(*p),
        _ => None,
    }
}

/// Transfer rate as a human should read it: an average with a real time
/// constant, published at a pace the eye can follow.
///
/// A raw delta between two samples swings wildly when a large file lands in one
/// tick, so the rate is averaged. Measuring often keeps the ETA honest, but a
/// number that changes three times a second reads as flicker however well it is
/// averaged, so what the view shows is held between refreshes.
#[derive(Debug, Default)]
struct Rate {
    last_bytes: u64,
    last_at: Option<Instant>,
    /// The average, moved by every sample.
    average: f64,
    /// What the view shows: the average as it stood at `shown_at`.
    per_second: u64,
    shown_at: Option<Instant>,
}

impl Rate {
    /// How long the average takes to cover about two thirds of a step change.
    const TAU: f64 = 2.0;
    /// How long a shown value stands before the average replaces it.
    const HOLD: Duration = Duration::from_secs(1);
    /// Shorter windows are mostly chunk-arrival noise, not a rate.
    const MIN_WINDOW: f64 = 0.25;

    fn sample(&mut self, bytes: u64) {
        let now = Instant::now();
        let Some(previous) = self.last_at else {
            self.last_at = Some(now);
            self.last_bytes = bytes;
            return;
        };
        let elapsed = now.duration_since(previous).as_secs_f64();
        if elapsed < Self::MIN_WINDOW {
            return;
        }

        let instant = bytes.saturating_sub(self.last_bytes) as f64 / elapsed;
        // Weighting by the time that passed rather than by the tick keeps the
        // smoothing the same whether ticks arrive on time, late or in a burst.
        self.average = match self.shown_at {
            None => instant,
            Some(_) => {
                let alpha = 1.0 - (-elapsed / Self::TAU).exp();
                self.average + (instant - self.average) * alpha
            }
        };
        self.last_bytes = bytes;
        self.last_at = Some(now);

        // The first real sample is shown at once -- an empty readout while the
        // average warms up would look stalled.
        if self.shown_at.is_none_or(|at| now.duration_since(at) >= Self::HOLD) {
            self.per_second = self.average as u64;
            self.shown_at = Some(now);
        }
    }
}

/// A download count, short enough to sit in a caption beside the author.
///
/// One decimal, dropped when it would read ".0", so popular addons are "98.4M"
/// and "142M" rather than nine digits the eye has to count. Short scale,
/// matching every other addon manager; small counts are left exact, because
/// "847" is already shorter than any abbreviation of it.
fn downloads_of(n: u64) -> String {
    let scaled = |n: u64, unit: f64, suffix: &str| {
        let value = n as f64 / unit;
        // The rounded value decides how many decimals to print, or 99,950,000
        // renders as "100.0M", a decimal place that is only ever ".0". The
        // raw value is what gets printed, because rounding twice turns
        // 999,499,999 into "1000M".
        let rounded = (value * 10.0).round() / 10.0;
        match rounded.fract() < 0.05 || rounded >= 100.0 {
            true => format!("{value:.0}{suffix}"),
            false => format!("{rounded:.1}{suffix}"),
        }
    };
    // The thresholds promote half a unit early, where the smaller unit would
    // round to four digits: 999,999,999 is "1B", never "1000M".
    match n {
        n if n >= 999_500_000 => scaled(n, 1e9, "B"),
        n if n >= 999_500 => scaled(n, 1e6, "M"),
        n if n >= 1_000 => scaled(n, 1e3, "K"),
        n => n.to_string(),
    }
}

fn bytes_of(done: u64, total: u64) -> String {
    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    match total {
        0 => format!("{:.2} GiB", gib(done)),
        _ => format!("{:.2} of {:.2} GiB", gib(done), gib(total)),
    }
}

/// Describe a config that would not load, keeping a copy of it aside.
///
/// The whole error chain, not just the top: "could not parse config.toml" is
/// useless without the line and column underneath it.
fn broken_config(error: &lightforge_core::config::ConfigError, path: &Path) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(&format!("\n{cause}"));
        source = cause.source();
    }
    let backup = path.with_extension("toml.broken");
    if std::fs::copy(path, &backup).is_err() {
        message.push_str("\n(could not save a copy to config.toml.broken)");
    }
    format!("{}\n\n{message}", path.display())
}

/// A typed folder as a full path, `~/` expanded.
fn parse_path(typed: &str) -> Result<PathBuf, String> {
    let typed = typed.trim();
    let path = match typed.strip_prefix("~/") {
        Some(rest) => {
            PathBuf::from(std::env::var_os("HOME").ok_or("cannot find your home directory")?)
                .join(rest)
        }
        None => PathBuf::from(typed),
    };
    match path.is_absolute() {
        true => Ok(path),
        false => Err(format!("{typed} is not a full path — start it with / or ~/")),
    }
}

/// A size in the unit it reads best in: a 16 MB cache is not "0.02 GiB".
fn size_of(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    match b {
        b if b >= MIB * 1024.0 * 1024.0 => format!("{:.2} TiB", b / (MIB * 1024.0 * 1024.0)),
        b if b >= MIB * 1024.0 => format!("{:.1} GiB", b / (MIB * 1024.0)),
        // A screenshot is the only thing here smaller than a megabyte, and
        // "0 MiB" beside a thumbnail reads as a broken file.
        b if b >= MIB => format!("{:.0} MiB", b / MIB),
        b => format!("{:.0} KiB", b / 1024.0),
    }
}

fn rate_of(per_second: u64) -> String {
    let mb = per_second as f64 / 1e6;
    match per_second {
        0 => "—".to_owned(),
        // Tenths tell a slow link apart from a stalled one; at speed they are
        // only jitter, and a digit that moves every second draws the eye.
        _ if mb >= 10.0 => format!("{mb:.0} MB/s"),
        _ => format!("{mb:.1} MB/s"),
    }
}

fn eta_of(progress: state::Progress) -> String {
    match progress.eta() {
        None => "ETA —".to_owned(),
        Some(eta) => {
            let secs = eta.as_secs();
            match secs {
                0..=59 => format!("ETA {secs}s"),
                60..=3599 => format!("ETA {}m", secs / 60),
                _ => format!("ETA {}h {}m", secs / 3600, (secs % 3600) / 60),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::servers_open;

    #[test]
    fn the_servers_caption_is_built_from_blizzards_date() {
        assert_eq!(servers_open("Available October 19, 2026"), "Servers will be available on October 19");
        // Anything else is Blizzard's words, untouched.
        assert_eq!(servers_open("Coming soon"), "Coming soon");
    }

    use super::*;

    fn chord(text: &str, modifiers: iced::keyboard::Modifiers) -> bool {
        hints_chord(&iced::keyboard::Key::Character(text.into()), modifiers)
    }

    #[test]
    fn a_date_column_is_a_day_and_a_month_or_the_string_itself() {
        // What the proxy sends, straight from CurseForge.
        assert_eq!(short_day("2026-10-04T20:10:34.237Z"), "4 Oct");
        // What our own ledger stamps.
        assert_eq!(short_day("2026-01-09"), "9 Jan");
        assert_eq!(short_day("2026-12-31"), "31 Dec");
        // A shape we do not know shows as itself. A column that silently
        // blanks is a column that looks broken, and the raw value at least
        // says what the provider sent.
        assert_eq!(short_day("whenever"), "whenever");
        assert_eq!(short_day("2026-13-01"), "2026-13-01", "no such month");
    }

    #[test]
    fn the_handoff_button_only_promises_what_is_known() {
        let act = |family: bool, installed: bool| match (family, installed) {
            (false, _) => HandoffAct::OpenClient,
            (true, true) => HandoffAct::Play,
            (true, false) => HandoffAct::Install,
        };

        assert_eq!(act(true, true), HandoffAct::Play);
        assert_eq!(act(true, false), HandoffAct::Install);
        // No family means no tile can be named, installed or not: the client
        // resolves an unknown one to Warcraft III, so neither Play nor
        // Install would be true.
        assert_eq!(act(false, true), HandoffAct::OpenClient);
        assert_eq!(act(false, false), HandoffAct::OpenClient);

        // Label, colour and message agree, which is the reason for the type.
        assert_eq!(HandoffAct::Play.label(), "Play via Battle.net");
        assert_eq!(HandoffAct::Install.label(), "Install via Battle.net");
        assert_eq!(HandoffAct::OpenClient.label(), "Open Battle.net");
        assert_eq!(HandoffAct::Play.colour(), Some(theme::ACCENT));
        assert_eq!(
            HandoffAct::Install.colour(),
            Some(theme::PAUSE),
            "blue is reserved for a game that can actually be played"
        );
        assert_eq!(
            HandoffAct::OpenClient.colour(),
            None,
            "opening the client is not an act on this game, so it draws outlined"
        );
        assert!(matches!(HandoffAct::Install.message(), Message::InstallViaBattleNet));
    }

    #[test]
    fn a_download_count_is_short_without_lying() {
        assert_eq!(downloads_of(0), "0");
        assert_eq!(downloads_of(847), "847", "small counts stay exact");
        assert_eq!(downloads_of(999), "999");
        assert_eq!(downloads_of(1_000), "1K", "a whole value drops the decimal");
        assert_eq!(downloads_of(24_138), "24.1K");
        assert_eq!(downloads_of(98_400_000), "98.4M");
        assert_eq!(downloads_of(142_000_000), "142M", "no decimal past a hundred");
        assert_eq!(downloads_of(300_000_000), "300M");
        assert_eq!(downloads_of(1_500_000_000), "1.5B");
        // Rounding up a magnitude must not leave a ".0" behind.
        assert_eq!(downloads_of(99_950_000), "100M");
        assert_eq!(downloads_of(999_999_999), "1B", "promoted, not four digits");
        assert_eq!(downloads_of(999_499_999), "999M", "and not promoted early");
        assert_eq!(downloads_of(999_499), "999K");
        assert_eq!(downloads_of(999_500), "1M");
    }

    #[test]
    fn the_hints_need_all_three_modifiers() {
        use iced::keyboard::Modifiers;
        let all = Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT;
        assert!(chord("h", all));
        assert!(!chord("h", Modifiers::CTRL | Modifiers::SHIFT), "Ctrl+Shift+H is a near miss");
        assert!(!chord("h", Modifiers::CTRL), "and plain Ctrl+H is someone else's shortcut");
        assert!(!chord("h", Modifiers::default()), "typing an h is just typing");
        assert!(!chord("g", all), "another letter is not the chord");
    }

    /// A band wide enough for four 54pt tiles and their gaps.
    fn room_for(tiles: usize) -> f32 {
        54.0 * tiles as f32 + theme::SM * tiles.saturating_sub(1) as f32
    }

    #[test]
    fn a_band_with_room_to_spare_hides_nothing() {
        let widths = [54.0; 3];
        let (shown, hidden) = divide_strip(&widths, 0, room_for(3));
        assert_eq!(shown, vec![0, 1, 2]);
        assert!(hidden.is_empty(), "nothing overflows when everything fits");
    }

    #[test]
    fn the_overflow_button_is_paid_for_out_of_the_band() {
        // Room for four tiles exactly, five to place: the fifth does not fit,
        // and making space for the button costs a fourth tile as well.
        let widths = [54.0; 5];
        let (shown, hidden) = divide_strip(&widths, 0, room_for(4));
        assert_eq!(shown, vec![0, 1, 2]);
        assert_eq!(hidden, vec![3, 4]);
    }

    #[test]
    fn the_selected_game_is_never_the_one_that_disappears() {
        let widths = [54.0; 5];
        let (shown, hidden) = divide_strip(&widths, 4, room_for(4));
        assert!(shown.contains(&4), "the game on screen stays in the band");
        assert_eq!(shown.last(), Some(&4), "it takes the place nearest the menu");
        assert!(!hidden.contains(&4));
        assert_eq!(hidden, vec![2, 3], "the tiles it displaced are the ones behind the menu");
    }

    #[test]
    fn a_band_with_almost_no_room_still_shows_the_selected_game() {
        let widths = [54.0; 4];
        let (shown, hidden) = divide_strip(&widths, 2, 10.0);
        assert_eq!(shown, vec![2], "something is always lit");
        assert_eq!(hidden, vec![0, 1, 3]);
    }

    #[test]
    fn a_wide_name_tile_costs_more_of_the_band_than_an_icon() {
        // Not a layout assertion, just that the estimate scales with the
        // name, since that is what keeps a long label from being cut off.
        let widths = [54.0, 200.0, 54.0];
        let (shown, hidden) = divide_strip(&widths, 0, room_for(4));
        assert_eq!(shown, vec![0], "the wide tile does not fit beside the first");
        assert_eq!(hidden, vec![1, 2]);
    }

    fn planned(remaining: u64, stored: u64) -> TransferState {
        TransferState {
            remaining: Some(remaining),
            progress: lightforge_core::tact::download::Progress {
                bytes_stored: stored,
                ..Default::default()
            },
            ..TransferState::default()
        }
    }

    #[test]
    fn an_update_measures_what_it_fetches_not_the_install() {
        // The symptom this guards: a 10 GiB update to a 90 GiB install opened
        // at "80 of 90 GiB", counting the untouched install as progress.
        let gib = 1024u64 * 1024 * 1024;
        let job = Job { total: Some(10 * gib), reached: 0 };

        let start = job_progress(job, &planned(10 * gib, 0), 0);
        assert_eq!((start.downloaded_bytes, start.total_bytes), (0, 10 * gib));

        let later = job_progress(job, &planned(10 * gib, 4 * gib), 0);
        assert_eq!(later.downloaded_bytes, 4 * gib);
    }

    #[test]
    fn a_resumed_run_continues_its_job_rather_than_restarting() {
        // Pause 3 GiB into a 10 GiB job: the next run plans only the 7 GiB
        // still missing, and the bar must carry on from 3, not from 0.
        let gib = 1024u64 * 1024 * 1024;
        let paused = Download {
            code: ProductCode::Retail,
            label: String::new(),
            stage: Stage::Paused(state::Progress {
                downloaded_bytes: 3 * gib,
                total_bytes: 10 * gib,
                bytes_per_second: 0,
            }),
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job { total: Some(10 * gib), reached: 0 },
        };
        let job = paused.resumed();

        // Still planning: hold where the pause left it.
        let planning = job_progress(job, &TransferState::default(), 0);
        assert_eq!((planning.downloaded_bytes, planning.total_bytes), (3 * gib, 10 * gib));

        // Planned: a run byte moves the job readout by one byte.
        let running = job_progress(job, &planned(7 * gib, gib), 0);
        assert_eq!((running.downloaded_bytes, running.total_bytes), (4 * gib, 10 * gib));
    }

    #[test]
    fn progress_stays_run_relative_until_the_plan_is_known() {
        let sample = TransferState {
            progress: lightforge_core::tact::download::Progress {
                bytes_stored: 500,
                bytes_total: 1000,
                ..Default::default()
            },
            ..TransferState::default()
        };
        let p = job_progress(Job::default(), &sample, 0);
        assert_eq!((p.downloaded_bytes, p.total_bytes), (500, 1000));
    }

    /// A game whose drive is away is held, whatever the last refresh thought
    /// of it, and held in a way that cannot turn into a download. Resolving
    /// a missing drive by re-fetching 150 GB onto the system disk is the one
    /// outcome this whole mechanism exists to prevent.
    #[test]
    fn a_game_on_an_absent_drive_is_held_rather_than_played_or_downloaded() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();

        // An install that is complete and current, so core would say Play.
        app.local = Local {
            build_key: Some("abc".to_owned()),
            version: Some("11.2.7.65000".to_owned()),
            executable: Some(PathBuf::from("/nowhere/Wow.exe")),
            store_files: 12,
        };
        assert!(matches!(app.action(), ActionState::Play));

        // The same product, on a library that was set up once and is not here.
        let mut away =
            library::Library::on_partition(std::path::Path::new("/nowhere"), "nvme2", None);
        away.claimed = true;
        let id = away.id.clone();
        app.config.libraries = vec![away];
        if let Some(product) = app.config.product_mut(&code) {
            product.profile.library = Some(id);
        }
        app.check_drives();

        match app.action() {
            ActionState::Offline { drive } => assert_eq!(drive, "nvme2"),
            other => panic!("expected the drive to hold the game, got {other:?}"),
        }
        // And the download path refuses on its own, because the queue is
        // restored and resumed without anyone pressing the button.
        let _ = app.enqueue();
        assert!(app.queue.is_empty(), "a download was queued onto a drive that is not there");
        assert_eq!(app.error.as_deref(), Some("nvme2 is not mounted"));
    }

    /// Dragging a tile moves its whole line, branches and all, and only in
    /// the direction the cursor is heading; a press and release without
    /// travel is still a click.
    #[test]
    fn dragging_a_tile_reorders_the_band() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();

        let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
        app.choices = vec![
            retail.clone(),
            Choice { code: ProductCode::Ptr, label: "PTR".to_owned() },
            Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
            Choice { code: ProductCode::EraPtr, label: "Era PTR".to_owned() },
            Choice { code: ProductCode::Classic, label: "Classic".to_owned() },
        ];
        app.config.products = app
            .choices
            .iter()
            .map(|c| Product::new(c.code.clone(), c.label.clone()))
            .collect();
        app.selected = retail;

        let classic = app.catalog.line_of(&ProductCode::Classic);
        let era = app.catalog.line_of(&ProductCode::ClassicEra);
        let retail_line = app.catalog.line_of(&ProductCode::Retail);

        // Held over Classic, nudged inside the slop, then dragged left over
        // Era (heading right: no move) and on over Retail (heading left).
        let _ = app.update(Message::TileEntered(classic.clone()));
        let _ = app.update(Message::TileGrabbed(classic.clone()));
        let _ = app.update(Message::TileDragged(300.0));
        let _ = app.update(Message::TileDragged(297.0));
        assert!(!app.tile_drag.as_ref().unwrap().moving, "a twitch started a drag");
        let _ = app.update(Message::TileEntered(era.clone()));
        let _ = app.update(Message::TileDragged(305.0));
        assert_eq!(app.lines().last(), Some(&classic), "moved against the direction of travel");
        let _ = app.update(Message::TileDragged(250.0));
        let _ = app.update(Message::TileEntered(retail_line.clone()));
        let _ = app.update(Message::TileDragged(200.0));
        let _ = app.update(Message::TileDropped);

        let order: Vec<&ProductCode> = app.config.products.iter().map(|p| &p.code).collect();
        assert_eq!(order, vec![
            &ProductCode::Classic,
            &ProductCode::Retail,
            &ProductCode::Ptr,
            &ProductCode::ClassicEra,
            &ProductCode::EraPtr,
        ]);
        assert_eq!(app.lines(), vec![classic.clone(), retail_line, era.clone()]);
        assert!(app.tile_drag.is_none());

        // A click: press and release on Era with no travel selects it.
        let _ = app.update(Message::TileEntered(era.clone()));
        let _ = app.update(Message::TileGrabbed(era.clone()));
        let _ = app.update(Message::TileDropped);
        assert_eq!(app.catalog.line_of(&app.selected.code), era);
    }

    /// The mouse moving takes the ring away, and the next key brings it back
    /// where it was rather than to the start of the page: Steam's keyboard
    /// moves the pointer as it closes, and the ring went to the logo.
    #[test]
    fn the_ring_comes_back_where_the_mouse_took_it() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.splash = None;
        let at = |x, y| iced::Rectangle { x, y, width: 80.0, height: 30.0 };
        let key = |name: &str| focus::Target::Key(name.to_owned());
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), focus::Press::Send(Message::MenuClick), 0);
        focus::put(key("frame cap"), at(400.0, 600.0), focus::Press::Click, 0);
        app.focus = Some(key("frame cap"));

        let _ = app.update(Message::FocusHidden);
        assert_eq!(app.focus, None);
        let _ = app.update(Message::FocusStep(focus::Step::Down));
        assert_eq!(app.focus, Some(key("frame cap")));

        // Gone from the page by then: the start of the page, as before.
        let _ = app.update(Message::FocusHidden);
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), focus::Press::Send(Message::MenuClick), 0);
        let _ = app.update(Message::FocusStep(focus::Step::Down));
        assert_eq!(app.focus, Some(focus::Target::AppMenu));
    }

    /// The Deck's controls arrive as keys: the D-pad as arrows, A as Enter,
    /// B as Escape. They have to reach a game and back out of a dialog with
    /// no mouse at all.
    /// Buttons that are gone once pressed. Install inside the addon details:
    /// the ring moves on inside the dialog, and closing the dialog still takes
    /// it back to the Details button that opened it. Keep on an addon row:
    /// the ring lands on the Remove beside it, not at the top of the page.
    #[test]
    fn a_button_gone_once_pressed_leaves_the_ring_nearby() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.splash = None;
        let at = |x, y| iced::Rectangle { x, y, width: 80.0, height: 30.0 };
        let key = |name: &str| focus::Target::Key(name.to_owned());
        let send = || focus::Press::Send(Message::MenuClick);
        let settle = |app: &mut Lightforge| {
            app.focus_entering = FOCUS_ENTER_FRAMES;
            let _ = app.update(Message::FocusSettle);
        };

        // The dialog opens over the page; the ring goes into it.
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
        focus::put(key("details"), at(600.0, 300.0), send(), 0);
        focus::put(key("install"), at(500.0, 500.0), send(), 1);
        focus::put(key("close"), at(600.0, 500.0), send(), 1);
        app.focus = Some(key("details"));
        settle(&mut app);
        assert_eq!(app.focus, Some(key("install")));

        // Install pressed and gone: on to Close, still in the dialog.
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
        focus::put(key("details"), at(600.0, 300.0), send(), 0);
        focus::put(key("close"), at(600.0, 500.0), send(), 1);
        settle(&mut app);
        assert_eq!(app.focus, Some(key("close")));

        // The dialog closed: back on Details.
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
        focus::put(key("details"), at(600.0, 300.0), send(), 0);
        settle(&mut app);
        assert_eq!(app.focus, Some(key("details")));
        // Keep pressed on a row, and gone with the question: the ring lands
        // on the Remove that comes back beside it, not at the top of the page.
        app.focus_return = None;
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
        focus::put(key("delete"), at(600.0, 400.0), send(), 0);
        focus::put(key("keep"), at(690.0, 400.0), send(), 0);
        app.focus = Some(key("keep"));
        let _ = app.update(Message::FocusPress);
        focus::reset();
        focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
        focus::put(key("remove"), at(600.0, 400.0), send(), 0);
        settle(&mut app);
        assert_eq!(app.focus, Some(key("remove")));
    }

    #[test]
    fn the_keyboard_alone_picks_a_game_and_closes_the_release_notes() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        app.wizard = None;
        app.splash = None;
        app.update_notice = None;
        app.screen = Screen::Dashboard;

        let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
        app.choices = vec![
            retail.clone(),
            Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
        ];
        app.config.products = app
            .choices
            .iter()
            .map(|c| Product::new(c.code.clone(), c.label.clone()))
            .collect();
        app.selected = retail;
        let retail_line = app.catalog.line_of(&ProductCode::Retail);
        let era = app.catalog.line_of(&ProductCode::ClassicEra);

        // What a frame would have recorded: where each control was drawn,
        // and in which layer.
        let at = |x: f32, y: f32| iced::Rectangle { x, y, width: 56.0, height: 40.0 };
        let send = focus::Press::Send;
        let page = || {
            focus::reset();
            focus::put(focus::Target::AppMenu, at(40.0, 10.0), send(Message::ToggleAppMenu), 0);
            focus::put(
                focus::Target::Tile(retail_line.clone()),
                at(110.0, 60.0),
                send(Message::LineSelected(retail_line.clone())),
                0,
            );
            focus::put(focus::Target::Tile(era.clone()), at(180.0, 60.0), send(Message::LineSelected(era.clone())), 0);
            focus::put(focus::Target::Primary, at(980.0, 660.0), send(Message::Primary), 0);
        };

        // Release notes up, over the page: the first key lands on Got it,
        // the page beneath is out of reach, and B closes them.
        page();
        focus::put(focus::Target::NotesDismiss, at(700.0, 500.0), send(Message::DismissPatchNotes), 1);
        app.patch_notes = Some(PatchNotes {
            release: lightforge_core::releases::Release { version: "9.9.9".to_owned(), notes: vec![] },
            mute: false,
        });
        let _ = app.update(Message::FocusStep(focus::Step::Down));
        assert_eq!(app.focus, Some(focus::Target::NotesDismiss));
        let _ = app.update(Message::FocusStep(focus::Step::Up));
        assert_eq!(app.focus, Some(focus::Target::NotesDismiss), "the ring left the dialog");
        let _ = app.update(Message::FocusBack);
        assert!(app.patch_notes.is_none(), "Escape left the notes up");

        // Gone with the dialog, the ring lands on PLAY once the page is drawn.
        page();
        let _ = app.update(Message::FocusSettle);
        assert_eq!(app.focus, Some(focus::Target::Primary));

        // Up from the first game is the wordmark above it.
        app.focus = Some(focus::Target::Tile(retail_line.clone()));
        let _ = app.update(Message::FocusStep(focus::Step::Up));
        assert_eq!(app.focus, Some(focus::Target::AppMenu));

        // Enter on the wordmark opens its menu, the ring moves into it once
        // it is drawn, and Escape brings it back to the wordmark.
        let _ = app.update(Message::FocusPress);
        assert!(app.app_menu_open);
        focus::put(focus::Target::Menu("Settings"), at(40.0, 50.0), send(Message::Show(Screen::Settings)), 1);
        let _ = app.update(Message::FocusSettle);
        assert_eq!(app.focus, Some(focus::Target::Menu("Settings")));
        let _ = app.update(Message::FocusBack);
        assert!(!app.app_menu_open);
        page();
        let _ = app.update(Message::FocusSettle);
        assert_eq!(app.focus, Some(focus::Target::AppMenu));

        app.focus = Some(focus::Target::Tile(retail_line));
        let _ = app.update(Message::FocusStep(focus::Step::Right));
        let _ = app.update(Message::FocusPress);
        assert_eq!(app.catalog.line_of(&app.selected.code), era);

        // The mouse moving takes the ring away.
        let _ = app.update(Message::FocusHidden);
        assert!(app.focus.is_none());
    }

    /// A tile is a line. Removing Classic Era with its PTR tracked used to
    /// leave the PTR behind, so the game stayed in the band under a branch
    /// name, the bug Gabriel reported as "it will still be here but only the
    /// ptr".
    #[test]
    fn removing_a_game_takes_its_ptr_and_beta_with_it() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();

        let keep = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
        app.choices = vec![
            keep.clone(),
            Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
            Choice { code: ProductCode::EraPtr, label: "Era PTR".to_owned() },
        ];
        app.config.products = app
            .choices
            .iter()
            .map(|c| Product::new(c.code.clone(), c.label.clone()))
            .collect();
        app.selected = keep;
        app.error = None;

        let _ = app.update(Message::RemoveProduct(ProductCode::ClassicEra));

        let left: Vec<&ProductCode> = app.choices.iter().map(|c| &c.code).collect();
        assert_eq!(left, vec![&ProductCode::Retail], "the PTR stayed behind");
        assert!(
            app.config.products.iter().all(|p| p.code == ProductCode::Retail),
            "the config still tracks a branch of the removed line",
        );
        // Only the save complains, and only because this harness has no
        // config directory, the removal itself was not refused.
        assert_ne!(app.error.as_deref(), Some("that is the only game left — add another first"));
    }

    /// The header cannot be emptied, and a line is now the unit, so the last
    /// *line* is held even when it has branches that would keep `choices`
    /// from going to zero.
    #[test]
    fn the_last_line_cannot_be_removed_even_with_its_branches_tracked() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();

        app.choices = vec![
            Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
            Choice { code: ProductCode::EraPtr, label: "Era PTR".to_owned() },
        ];
        app.config.products = app
            .choices
            .iter()
            .map(|c| Product::new(c.code.clone(), c.label.clone()))
            .collect();
        app.selected = app.choices[0].clone();
        app.error = None;

        let _ = app.update(Message::RemoveProduct(ProductCode::ClassicEra));

        assert_eq!(app.choices.len(), 2, "the only line was removed");
        assert_eq!(app.error.as_deref(), Some("that is the only game left — add another first"));
    }

    /// A game opened from Add a game is shown, not tracked, and survives the
    /// band being rebuilt; the first action on it puts it in the band.
    #[test]
    fn a_game_opened_from_add_a_game_joins_the_band_only_when_acted_on() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();

        let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
        app.choices = vec![retail.clone()];
        app.config.products = vec![Product::new(ProductCode::Retail, "Retail")];
        app.selected = retail;

        let _ = app.update(Message::OpenProduct(ProductCode::ClassicEra));
        assert_eq!(app.selected.code, ProductCode::ClassicEra);
        assert_eq!(app.choices.len(), 1, "looking at a game added it to the band");
        assert!(app.config.product(&ProductCode::ClassicEra).is_none());

        // Adoption and the wizard rebuild the band; the page stays put.
        app.sync_choices();
        assert_eq!(app.selected.code, ProductCode::ClassicEra, "the preview was dropped");

        app.track_selected();
        assert!(app.choices.iter().any(|c| c.code == ProductCode::ClassicEra));
        assert!(app.config.product(&ProductCode::ClassicEra).is_some());
        app.track_selected();
        assert_eq!(app.choices.len(), 2, "tracked twice");
    }

    /// Remove from a tile's menu takes the line out of the band and closes
    /// the menu; on the last tile it is held.
    #[test]
    fn a_tiles_menu_removes_its_line_and_closes() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();

        let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
        app.choices = vec![
            retail.clone(),
            Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
        ];
        app.config.products = app
            .choices
            .iter()
            .map(|c| Product::new(c.code.clone(), c.label.clone()))
            .collect();
        app.selected = retail;

        let _ = app.update(Message::TileMenu(ProductLine::ClassicEra));
        assert_eq!(app.tile_menu, Some(ProductLine::ClassicEra));
        assert!(app.panel_open());
        // A second right-click on the same tile closes it again.
        let _ = app.update(Message::TileMenu(ProductLine::ClassicEra));
        assert!(app.tile_menu.is_none());

        let _ = app.update(Message::TileMenu(ProductLine::ClassicEra));
        let _ = app.update(Message::RemoveProduct(ProductLine::ClassicEra.head()));
        assert_eq!(app.choices.len(), 1);
        assert!(app.tile_menu.is_none(), "the menu outlived its tile");
    }

    /// A bug report goes nowhere without a category and a way back; feedback
    /// only needs the category.
    #[test]
    fn a_bug_report_needs_a_category_and_a_contact() {
        let dialog = |kind, category, contact: &str| ReportDialog {
            kind,
            note: iced::widget::text_editor::Content::with_text("it broke"),
            category,
            contact: contact.to_owned(),
            tags: Vec::new(),
            shot: None,
            base: String::new(),
            debug: String::new(),
            history: Vec::new(),
            stage: ReportStage::Writing,
        };
        let addons = Some(report::Category::Addons);
        assert!(!dialog(ReportKind::Bug, None, "me@example.org").can_send());
        assert!(!dialog(ReportKind::Bug, addons, "  ").can_send());
        assert!(dialog(ReportKind::Bug, addons, "someone#1234").can_send());
        assert!(dialog(ReportKind::Feedback, addons, "").can_send());
        assert!(!dialog(ReportKind::Feedback, None, "").can_send());
    }

    /// The popup opens once per version and then not again, and every way of
    /// saying "not now" is honoured. Each `return` in `open_patch_notes` is a
    /// case that should see nothing.
    #[test]
    fn the_release_notes_open_once_after_an_update_and_never_uninvited() {
        let version = env!("CARGO_PKG_VERSION");
        let fresh = || {
            let (mut app, _) = Lightforge::new();
            app.dirs = None;
            app.queue.clear();
            app.config.setup_complete = true;
            app.config.patch_notes_muted = false;
            app.config.patch_notes_seen = Some("0.0.1".to_owned());
            app
        };

        // Updated from an older version: this is the one start that shows them.
        let mut app = fresh();
        app.open_patch_notes();
        assert_eq!(
            app.patch_notes.as_ref().map(|n| n.release.version.clone()),
            Some(version.to_owned()),
        );

        // Dismissed, and the version recorded, so a second start is quiet.
        let _ = app.update(Message::DismissPatchNotes);
        assert!(app.patch_notes.is_none());
        assert_eq!(app.config.patch_notes_seen.as_deref(), Some(version));
        assert!(!app.config.patch_notes_muted, "closing it is not muting it");
        app.open_patch_notes();
        assert!(app.patch_notes.is_none(), "shown twice for one version");

        // The checkbox means every future release, not just this one.
        let mut app = fresh();
        app.open_patch_notes();
        let _ = app.update(Message::MutePatchNotes(true));
        let _ = app.update(Message::DismissPatchNotes);
        assert!(app.config.patch_notes_muted);
        app.config.patch_notes_seen = Some("0.0.1".to_owned());
        app.open_patch_notes();
        assert!(app.patch_notes.is_none(), "muted and still opened");

        // A machine still in the wizard has a first run to get through, and
        // notes about a build nothing has been done with yet say nothing.
        let mut app = fresh();
        app.config.setup_complete = false;
        app.open_patch_notes();
        assert!(app.patch_notes.is_none());
    }

    #[test]
    fn a_resume_during_the_wind_down_is_not_swallowed() {
        let (mut app, _) = Lightforge::new();
        // Nothing read from or written to the real queue file.
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        // Paused, then resumed before the paused run reported back: the entry
        // is already a fresh, queued one and the line is still winding down.
        app.queue.push(Download {
            code: code.clone(),
            label: String::new(),
            stage: Stage::Queued,
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job::default(),
        });
        app.winding_down = Some(code.clone());

        let stopped = Complete { complete: false, cancelled: true, files: 0, version: None };
        let _ = app.update(Message::Downloaded(code, Ok(stopped)));

        assert!(app.winding_down.is_none());
        assert!(matches!(app.queue[0].stage, Stage::Running { .. }), "the resume must start");
        assert_eq!(app.status, "downloading…");
    }

    #[test]
    fn a_library_under_home_is_shown_short_and_one_elsewhere_is_not() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("a home directory"));
        assert_eq!(
            home_relative(&home.join("Games/Blizzard/games/World of Warcraft")),
            "~/Games/Blizzard/games/World of Warcraft"
        );
        // Every part of a path on another drive says which drive it is.
        let external = PathBuf::from("/run/media/someone/ssd/games");
        assert_eq!(home_relative(&external), "/run/media/someone/ssd/games");
    }

    /// A queued or paused entry has no run to wind down, so its files can go
    /// at once. A running one must not: the downloader still has the store
    /// open, and deleting it under its own writer is the thing this defers.
    #[test]
    fn deleting_a_cancelled_install_waits_for_the_run_to_stop() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        app.queue.push(Download {
            code: code.clone(),
            label: "Retail".to_owned(),
            stage: Stage::Queued,
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job::default(),
        });

        app.cancel_dialog = Some(CancelDialog {
            code: code.clone(),
            label: "Retail".to_owned(),
            installed: None,
        });
        let _ = app.update(Message::CancelDownloadConfirm { delete: true });

        assert!(app.queue.is_empty(), "the entry goes either way");
        assert!(
            app.delete_after_cancel.is_empty(),
            "a queued entry had no run open, so nothing is deferred"
        );
        assert_eq!(app.discarding, Some(code), "removed at once");
    }

    fn running(app: &mut Lightforge, code: &ProductCode) {
        let (_tx, rx) = tokio::sync::watch::channel(TransferState::default());
        app.queue.push(Download {
            code: code.clone(),
            label: "Retail".to_owned(),
            stage: Stage::Running { rx, cancel: Cancel::new() },
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job::default(),
        });
    }

    fn stopped(cancelled: bool) -> Result<Complete, String> {
        Ok(Complete { complete: false, cancelled, files: 0, version: None })
    }

    #[test]
    fn a_running_download_is_removed_only_once_its_run_has_stopped() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        running(&mut app, &code);
        app.cancel_dialog =
            Some(CancelDialog { code: code.clone(), label: "Retail".to_owned(), installed: None });

        let _ = app.update(Message::CancelDownloadConfirm { delete: true });
        assert_eq!(app.delete_after_cancel, std::slice::from_ref(&code));
        assert_eq!(app.discarding, None, "the run is still winding down");

        let _ = app.update(Message::Downloaded(code.clone(), stopped(true)));
        assert!(app.delete_after_cancel.is_empty());
        assert_eq!(app.discarding, Some(code));
    }

    #[test]
    fn a_cancel_confirmed_after_the_install_finished_deletes_nothing() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        app.cancel_dialog =
            Some(CancelDialog { code: code.clone(), label: "Retail".to_owned(), installed: None });

        let _ = app.update(Message::CancelDownloadConfirm { delete: true });
        assert!(app.delete_after_cancel.is_empty());
        assert_eq!(app.discarding, None);
    }

    #[test]
    fn scan_and_repair_waits_for_a_running_download() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        running(&mut app, &code);

        let _ = app.update(Message::ScanAndRepair);
        assert!(!app.scanning, "repair rewrites the indices the download is writing");
        assert!(app.error.is_some());
    }

    #[test]
    fn queueing_a_product_that_is_downloading_leaves_its_run_alone() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        running(&mut app, &code);

        let _ = app.enqueue_for(code.clone());
        assert_eq!(app.queue.len(), 1);
        assert!(matches!(app.queue[0].stage, Stage::Running { .. }));
    }

    #[test]
    fn a_launch_waits_for_its_paused_download_to_come_to_rest() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        running(&mut app, &code);
        app.busy = true;
        app.launching = Some(code.clone());

        // Pre-flight tripped the token and returned; the run has not stopped.
        let _ = app.update(Message::PreflightDone(Launch::Game, Ok(())));
        assert_eq!(app.launch_when_quiet, Some(Launch::Game));

        let _ = app.update(Message::Downloaded(code.clone(), stopped(true)));
        assert_eq!(app.launch_when_quiet, None, "started once the run stopped");
        assert!(app.status.starts_with("running"), "{}", app.status);
        assert_eq!(app.launching, Some(code), "still this product's session");
    }

    #[test]
    fn a_download_a_launch_paused_is_restarted_when_the_game_exits() {
        use lightforge_core::preflight::DownloadGuard;

        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        app.config.pause_downloads_while_playing = false;
        let code = app.selected.code.clone();
        running(&mut app, &code);
        app.downloads.register(code.clone(), Cancel::new());
        app.busy = true;
        app.launching = Some(code.clone());

        // Pre-flight pauses it, and the run reports back as cancelled.
        let mut guard = app.downloads.clone();
        assert!(guard.pause(&code).unwrap());
        let _ = app.update(Message::Downloaded(code.clone(), stopped(true)));
        assert!(matches!(app.queue[0].stage, Stage::Paused(_)));

        let _ = app.update(Message::Launched(Ok("exited".to_owned())));
        assert!(
            !matches!(app.queue[0].stage, Stage::Paused(_)),
            "queued again, or already running"
        );
    }

    #[test]
    fn closing_waits_for_a_repair_to_finish() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        app.repairing = Some(app.selected.code.clone());

        let _ = app.update(Message::CloseRequested(iced::window::Id::unique()));
        assert!(app.closing.is_some());
    }

    #[test]
    fn cancelling_an_update_never_deletes_anything() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        app.queue.push(Download {
            code: code.clone(),
            label: "Retail".to_owned(),
            stage: Stage::Queued,
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job::default(),
        });
        app.cancel_dialog = Some(CancelDialog {
            code: code.clone(),
            label: "Retail".to_owned(),
            // Installed, so this is an update and the game plays right now.
            installed: Some("1.15.9.70003".to_owned()),
        });

        let _ = app.update(Message::CancelDownloadConfirm { delete: false });

        assert!(app.queue.is_empty());
        assert!(app.delete_after_cancel.is_empty(), "an update's files are never removed");
        assert_eq!(app.status, "cancelled");
    }

    /// Asking about a product with nothing in the queue must not open a dialog
    /// offering to delete its install.
    #[test]
    fn there_is_nothing_to_cancel_when_nothing_is_downloading() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();

        let _ = app.update(Message::AskCancelDownload(code));

        assert!(app.cancel_dialog.is_none());
    }

    #[test]
    fn a_download_running_at_close_is_saved_as_running() {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        let code = app.selected.code.clone();
        app.queue.push(Download {
            code: code.clone(),
            label: String::new(),
            stage: Stage::Queued,
            sample: TransferState::default(),
            rate: Rate::default(),
            job: Job { total: Some(12 << 30), reached: 0 },
        });
        let _ = app.pump();
        let running = app.queue_snapshot();
        assert_eq!(running.entries[0].standing, Standing::Running);
        app.saved_queue = running.clone();

        // Closing pauses the transfer to checkpoint it; that pause must not
        // be what the next start restores.
        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert!(matches!(app.queue[0].stage, Stage::Paused(_)));
        assert_eq!(app.saved_queue, running);
    }

    #[test]
    fn a_config_that_will_not_load_is_described_kept_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("lightforge-badcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dirs = AppDirs::with_dirs(dir.join("config"), dir.join("cache"), dir.join("data"));
        std::fs::create_dir_all(dirs.config_dir()).unwrap();
        let broken = "default_proton = \"GE-Proton10-34\"\nprefix_user = [oops\n";
        std::fs::write(dirs.config_file(), broken).unwrap();

        let error = Config::load_or_default(&dirs.config_file()).unwrap_err();
        let message = broken_config(&error, &dirs.config_file());
        assert!(message.contains("line 2"), "names where: {message}");
        let backup = dirs.config_file().with_extension("toml.broken");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), broken);

        let (mut app, _) = Lightforge::new();
        app.dirs = Some(dirs.clone());
        app.queue.clear();
        app.config_error = Some(message);
        app.config.download_limit_mbps = Some(5);
        app.save_config();
        app.persist_queue();
        assert_eq!(std::fs::read_to_string(dirs.config_file()).unwrap(), broken);
        assert!(!dirs.queue_file().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_offered_optional_content_is_shown_or_fetched() {
        let tags = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        // What the Forever beta's manifest carries.
        assert_eq!(offered_only(&tags(&["Alternate", "HighRes", "Talebound"])), tags(&["HighRes"]));
        // A stale opt-in from before the list existed is dropped, not fetched.
        assert!(offered_only(&tags(&["Talebound"])).is_empty());
    }

    #[test]
    fn the_rate_meter_smooths_rather_than_tracking_one_tick() {
        let mut rate = Rate::default();
        rate.sample(0);
        // First real sample seeds the average outright.
        std::thread::sleep(Duration::from_millis(300));
        rate.sample(30_000_000);
        let first = rate.per_second;
        assert!(first > 0);

        // A tick with nothing stored must not drop the readout to zero: the
        // average gives up a fraction of the way, not all of it.
        std::thread::sleep(Duration::from_millis(300));
        rate.sample(30_000_000);
        assert!(rate.average > 0.0 && rate.average < first as f64);
    }

    #[test]
    fn the_shown_rate_is_held_a_second_however_often_it_is_sampled() {
        let mut rate = Rate::default();
        rate.sample(0);
        std::thread::sleep(Duration::from_millis(300));
        rate.sample(30_000_000);
        let shown = rate.per_second;

        // Three more samples inside the hold: the average follows them, the
        // number on screen does not move.
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(260));
            rate.sample(30_000_000);
        }
        assert_eq!(rate.per_second, shown);
        assert!(rate.average < shown as f64);

        // Past the hold, the next sample publishes where the average got to.
        std::thread::sleep(Duration::from_millis(300));
        rate.sample(30_000_000);
        assert_eq!(rate.per_second, rate.average as u64);
        assert!(rate.per_second < shown);
    }

    #[test]
    fn the_rate_readout_drops_the_decimal_once_tenths_are_only_jitter() {
        assert_eq!(rate_of(0), "—");
        assert_eq!(rate_of(6_400_000), "6.4 MB/s");
        assert_eq!(rate_of(9_900_000), "9.9 MB/s");
        assert_eq!(rate_of(31_400_000), "31 MB/s");
    }

    #[test]
    fn eta_is_rendered_in_the_largest_useful_unit() {
        let p = |done: u64, total: u64, rate: u64| state::Progress {
            downloaded_bytes: done,
            total_bytes: total,
            bytes_per_second: rate,
        };
        assert_eq!(eta_of(p(0, 30, 1)), "ETA 30s");
        assert_eq!(eta_of(p(0, 600, 1)), "ETA 10m");
        assert_eq!(eta_of(p(0, 7200, 1)), "ETA 2h 0m");
        // Stalled: no rate means no honest estimate.
        assert_eq!(eta_of(p(0, 100, 0)), "ETA —");
    }

    #[test]
    fn byte_readouts_survive_an_unknown_total() {
        assert_eq!(bytes_of(1024 * 1024 * 1024, 0), "1.00 GiB");
        assert!(bytes_of(0, 1024 * 1024 * 1024).ends_with("of 1.00 GiB"));
    }
}
