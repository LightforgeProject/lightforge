//! A product's artwork, decoded once and held in memory.
//!
//! Handing iced a file path looks free but isn't: the tiny-skia renderer keeps
//! a decoded image only while it is being drawn, and trims everything else at
//! the end of each frame. Leave the dashboard and the background is dropped;
//! come back and the whole JPEG is decoded again on the render thread before
//! the page can paint. With 3840×1600 art that was the pause on returning to
//! Play.
//!
//! So the pixels live here instead, as RGBA behind a stable handle. The
//! renderer still trims its own copy, but rebuilding it from ours is a copy,
//! not a decode.
//!
//! Every picture the GUI draws is decoded here, and only here. iced is built
//! without image codecs, so a handle holding a path or encoded bytes would not
//! draw at all; what this module cannot read becomes [`placeholder`] instead.

use std::path::Path;
use std::sync::OnceLock;

use iced::widget::image::Handle;
use image::imageops::FilterType;
use lightforge_core::assets::Branding;

/// Widest a background is kept. Enough to fill a full-screen 1440p window;
/// art is drawn behind a scrim, so what a 4K window loses is not visible.
/// Past this width every frame pays to scale pixels it throws away.
const BACKGROUND_MAX_WIDTH: u32 = 2560;

/// Widest a product mark is kept: twice [`crate::theme::TILE`].
///
/// Marks are published at 256px so they stay useful if the UI ever draws them
/// larger, but the band draws them at 40pt. Handing the renderer 256px and
/// letting it reduce that far samples four texels per pixel with no mipmaps,
/// which is the crunchy, aliased look that reads as "pixelated".
///
/// Twice the tile rather than exactly the tile: a 2x display then maps one
/// texel to one device pixel, and a 1x display gets a 2:1 reduction, which is
/// the one ratio bilinear filtering handles cleanly. Measured against 64, 96
/// and 128 at the drawn size, the differences past this point are not visible,
/// what remains is the artwork, not the scaling.
const MARK_MAX_WIDTH: u32 = (crate::theme::TILE * 2.0) as u32;

/// Widest a logo is kept. It is drawn at most a couple of hundred pixels tall.
const LOGO_MAX_WIDTH: u32 = 1024;

/// How many times its width a scaled picture may be tall.
///
/// Nothing the GUI draws is portrait past this, and without a bound on height
/// a 320x60000 file would sail through a width-only cap at full size.
const MAX_TALL: u32 = 2;

/// What a picture that could not be decoded draws as: one transparent pixel.
///
/// One handle for the whole session rather than one per failure, so a feed
/// with a broken picture in it does not mint a fresh texture on every pass.
pub fn placeholder() -> Handle {
    static EMPTY: OnceLock<Handle> = OnceLock::new();
    EMPTY.get_or_init(|| Handle::from_rgba(1, 1, vec![0, 0, 0, 0])).clone()
}

/// [`Branding`] with its images ready to draw.
#[derive(Debug, Clone)]
pub struct Art {
    pub branding: Branding,
    pub logo: Handle,
    /// The trimmed wordmark's width over its height.
    ///
    /// Carried because the dashboard sizes the logo by *area* rather than by
    /// a bounding box; see [`crate::theme::logo_size`]. `1.0` for a logo
    /// that could not be measured, which is a square box and no worse than
    /// any other guess.
    pub logo_aspect: f32,
    /// How much of the trimmed wordmark is the mark, and how much is glow.
    ///
    /// Some logos are drawn with a wide soft halo, Retail's is a third of
    /// its own bounding box, and a halo is not something the eye measures a
    /// logo by. Sizing those by their outer bounds makes the mark inside
    /// come out visibly smaller than everything else, which is exactly what
    /// Retail looked like. This is the fraction of the bounding box the solid
    /// mark covers, and the dashboard divides by it so that it is the *mark*
    /// that gets the standard area, with the glow spilling past.
    ///
    /// `1.0` for a logo with no soft edge at all, which is most of them.
    pub logo_ink: f32,
    pub background: Handle,
}

impl Art {
    /// Decode both images. Blocking and CPU-bound, call it off the UI thread.
    pub fn decode(branding: Branding) -> Self {
        let (logo, logo_aspect, logo_ink) = logo(&branding.logo.path);
        let background = load(&branding.background.path, BACKGROUND_MAX_WIDTH);
        Self {
            branding,
            logo,
            logo_aspect,
            logo_ink,
            background,
        }
    }
}

/// A product mark, decoded and scaled for the size it is actually drawn at.
///
/// Unlike [`load`]'s backgrounds this is a large reduction, 256px down to
/// 128, so it is worth the sharper filter. The image is tiny and this runs
/// once per product per session.
pub fn mark(path: &Path) -> Handle {
    try_mark(path).unwrap_or_else(placeholder)
}

/// [`mark`], or `None` for a file that does not decode, for a caller that
/// would rather draw nothing at all than an empty box.
pub fn try_mark(path: &Path) -> Option<Handle> {
    try_scale(path, MARK_MAX_WIDTH, FilterType::Lanczos3)
}

/// Decode a bundled image that is already at the size it is drawn.
///
/// Bundled art goes through here rather than `Handle::from_bytes` for the
/// reason at the top of this file: a handle holding compressed bytes is
/// decoded by the renderer, which keeps the result only while it is drawing
/// and throws it away at the end of the frame. A 2560px JPEG re-decoded on
/// every return to the page is exactly the pause this module exists to
/// remove. Blocking and CPU-bound: a large picture belongs off the UI thread,
/// a bundled icon of a few hundred bytes does not.
pub fn decode_bundled(bytes: &'static [u8]) -> Handle {
    match image::load_from_memory(bytes) {
        Ok(decoded) => {
            let rgba = decoded.into_rgba8();
            Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
        }
        Err(e) => {
            crate::trace::mark(format_args!("bundled image failed to decode: {e}"));
            placeholder()
        }
    }
}

/// A product wordmark, cropped to the pixels that are actually drawn.
///
/// Blizzard puts every logo on the same 704x440 canvas, but the marks inside
/// it differ wildly (StarCraft's is 127px tall, Retail's 440). Fitting the
/// canvas left them at different sizes; fitting the cropped mark makes them
/// read as one size. A logo with no transparent border, or one that is all
/// transparent, comes back unchanged.
///
/// Returns the image, its aspect, and its [`Art::logo_ink`] fraction.
fn logo(path: &Path) -> (Handle, f32, f32) {
    let decoded = match image::open(path) {
        Ok(decoded) => decoded,
        Err(e) => {
            crate::trace::mark(format_args!("decode failed for {}: {e}", path.display()));
            return (placeholder(), 1.0, 1.0);
        }
    };

    let mut rgba = decoded.into_rgba8();
    if let Some((x, y, w, h)) = ink_bounds(&rgba, 0) {
        if (w, h) != (rgba.width(), rgba.height()) {
            rgba = image::imageops::crop(&mut rgba, x, y, w, h).to_image();
        }
    }

    // Measured on the cropped image, so both rectangles are in the same frame
    // and the fraction is simply one area over the other.
    let ink = match ink_bounds(&rgba, SOLID_ALPHA) {
        Some((_, _, w, h)) => {
            let full = f64::from(rgba.width()) * f64::from(rgba.height());
            match full > 0.0 {
                true => (f64::from(w) * f64::from(h) / full) as f32,
                false => 1.0,
            }
        }
        None => 1.0,
    };

    let rgba = match rgba.width() > LOGO_MAX_WIDTH {
        true => {
            let height = (u64::from(rgba.height()) * u64::from(LOGO_MAX_WIDTH)
                / u64::from(rgba.width())) as u32;
            image::imageops::resize(
                &rgba,
                LOGO_MAX_WIDTH,
                height.max(1),
                FilterType::Triangle,
            )
        }
        false => rgba,
    };

    let aspect = match rgba.height() > 0 {
        true => rgba.width() as f32 / rgba.height() as f32,
        false => 1.0,
    };
    (Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw()), aspect, ink)
}

/// Where a logo's glow stops being glow and starts being the mark.
///
/// An eighth of full opacity. Above it the pixel is something you would point
/// at; below it, on these logos, it is the outer half of a halo. The exact
/// number matters less than it sounds, Retail's bounds move 25px between an
/// eighth and two thirds, and every other logo in the catalogue moves less
/// than 5.
const SOLID_ALPHA: u8 = 32;

/// The smallest rectangle holding every pixel more opaque than `alpha`, as
/// `(x, y, width, height)`.
///
/// `None` when there is no such pixel, an all-transparent image, or one whose
/// every pixel is fainter than the threshold, neither of which has anything to
/// measure.
///
/// At `0` this is "not fully transparent", which is the crop: a logo's own
/// soft edge is still artwork and must not be cut off. At [`SOLID_ALPHA`] it
/// is the mark within that artwork, which is what the page is sized by.
fn ink_bounds(rgba: &image::RgbaImage, alpha: u8) -> Option<(u32, u32, u32, u32)> {
    let (mut left, mut top) = (u32::MAX, u32::MAX);
    let (mut right, mut bottom) = (0_u32, 0_u32);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel.0[3] <= alpha {
            continue;
        }
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    match left == u32::MAX {
        true => None,
        false => Some((left, top, right - left + 1, bottom - top + 1)),
    }
}

/// Decode and downscale one image. An image this can't read is drawn as
/// [`placeholder`].
fn load(path: &Path, max_width: u32) -> Handle {
    // Triangle rather than Lanczos: a one-off downscale by less than 2x,
    // where the sharper filter costs several times as long for no difference
    // behind a scrim.
    scale(path, max_width, FilterType::Triangle)
}

/// Decode a file, asking a JPEG to come out no larger than it needs to be.
///
/// Decoding at full size once cost 665 MB across a page of pictures for 22 MB
/// of thumbnails. JPEG can emit at 1/2, 1/4 or 1/8 scale almost free, so `scale`
/// asks for the smallest that still covers `max_width` and the resize below
/// finishes the job. JPEG is recognised by its magic bytes (cache files are
/// named by URL hash); PNG has no such trick and is decoded whole. The box is
/// `max_width` by [`MAX_TALL`] times that, under the same 512 MiB allocation
/// limit `Reader` gives the generic path.
fn decode(path: &Path, max_width: u32) -> image::ImageResult<image::DynamicImage> {
    use image::ImageDecoder;
    use std::io::Read;

    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 3];
    let is_jpeg = file.read_exact(&mut magic).is_ok() && magic == [0xFF, 0xD8, 0xFF];
    file.seek_relative(-(magic.len() as i64))?;

    if !is_jpeg {
        return image::io::Reader::new(file).with_guessed_format()?.decode();
    }

    let mut decoder = image::codecs::jpeg::JpegDecoder::new(file)?;
    // The decoder keeps the aspect ratio and stops at the first scale that
    // covers *either* side, which is the side the fit below is bound by.
    let width = u16::try_from(max_width).unwrap_or(u16::MAX);
    let height = u16::try_from(max_width.saturating_mul(MAX_TALL)).unwrap_or(u16::MAX);
    let _ = decoder.scale(width, height);
    // After the scale, so the reservation is for the buffer actually decoded.
    let mut limits = image::io::Limits::default();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    image::DynamicImage::from_decoder(decoder)
}

fn scale(path: &Path, max_width: u32, filter: FilterType) -> Handle {
    try_scale(path, max_width, filter).unwrap_or_else(placeholder)
}

fn try_scale(path: &Path, max_width: u32, filter: FilterType) -> Option<Handle> {
    let decoded = match decode(path, max_width) {
        Ok(decoded) => decoded,
        Err(e) => {
            crate::trace::mark(format_args!("decode failed for {}: {e}", path.display()));
            return None;
        }
    };

    let max_height = max_width.saturating_mul(MAX_TALL);
    let (w, h) = (decoded.width(), decoded.height());
    let decoded = if w > max_width || h > max_height {
        // Bound by whichever side is further over, in integers so a 1px
        // rounding never pushes the other side past its own limit.
        let (width, height) = match u64::from(w) * u64::from(max_height)
            >= u64::from(h) * u64::from(max_width)
        {
            true => (max_width, (u64::from(h) * u64::from(max_width) / u64::from(w)) as u32),
            false => ((u64::from(w) * u64::from(max_height) / u64::from(h)) as u32, max_height),
        };
        decoded.resize_exact(width.max(1), height.max(1), filter)
    } else {
        decoded
    };

    let rgba = decoded.into_rgba8();
    Some(Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_size(handle: &Handle) -> Option<(u32, u32)> {
        match handle {
            Handle::Rgba { width, height, .. } => Some((*width, *height)),
            _ => None,
        }
    }

    fn write_jpeg(name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("lightforge-art-{}-{name}.jpg", std::process::id()));
        image::RgbImage::new(width, height).save(&path).unwrap();
        path
    }

    #[test]
    fn wide_art_is_capped_keeping_its_aspect() {
        let path = write_jpeg("wide", 3840, 1600);
        let handle = load(&path, BACKGROUND_MAX_WIDTH);
        let _ = std::fs::remove_file(&path);
        assert_eq!(rgba_size(&handle), Some((2560, 1066)));
    }

    /// The point of the JPEG path: the buffer that comes out of the decoder
    /// is already close to the size wanted, so the 33 MB full-resolution one
    /// is never allocated. A thumbnail is 320 wide, and 3840/8 = 480 is the
    /// smallest eighth that still covers it.
    #[test]
    fn a_large_jpeg_is_decoded_small_rather_than_decoded_and_shrunk() {
        let path = write_jpeg("dct", 3840, 2160);
        let decoded = decode(&path, 320).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            decoded.width() < 3840,
            "decoded at full size ({}x{}) — the DCT scale was not applied",
            decoded.width(),
            decoded.height(),
        );
        assert!(
            decoded.width() >= 320,
            "decoded smaller than asked for ({}), which would soften the result",
            decoded.width(),
        );
        // Still the same picture, so the aspect has to survive.
        assert_eq!(decoded.width() * 2160 / 3840, decoded.height());
    }

    /// And the result is the same size it always was: the decoder gets it
    /// close, the resize lands it exactly.
    #[test]
    fn a_scaled_decode_still_ends_at_the_width_asked_for() {
        let path = write_jpeg("thumb", 2560, 1440);
        let width = 320;
        let handle = scale(&path, width, FilterType::Lanczos3);
        let _ = std::fs::remove_file(&path);
        assert_eq!(rgba_size(&handle), Some((width, width * 1440 / 2560)));
    }

    /// A PNG has no such trick and must still come through whole.
    #[test]
    fn a_png_is_decoded_and_resized_as_before() {
        let path = std::env::temp_dir()
            .join(format!("lightforge-art-{}-plain.png", std::process::id()));
        image::RgbaImage::new(1024, 512).save(&path).unwrap();
        let handle = scale(&path, 256, FilterType::Triangle);
        let _ = std::fs::remove_file(&path);
        assert_eq!(rgba_size(&handle), Some((256, 128)));
    }

    #[test]
    fn narrow_art_is_left_alone() {
        let path = write_jpeg("narrow", 1920, 1080);
        let handle = load(&path, BACKGROUND_MAX_WIDTH);
        let _ = std::fs::remove_file(&path);
        assert_eq!(rgba_size(&handle), Some((1920, 1080)));
    }

    /// A wordmark on a canvas of empty space comes back as the wordmark.
    #[test]
    fn a_logos_transparent_border_is_cropped_away() {
        let mut canvas = image::RgbaImage::new(704, 440);
        for y in 150..280 {
            for x in 40..664 {
                canvas.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let path = std::env::temp_dir()
            .join(format!("lightforge-art-{}-padded.png", std::process::id()));
        canvas.save(&path).unwrap();
        let (handle, _, _) = logo(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(rgba_size(&handle), Some((624, 130)));
    }

    /// Nothing to crop to is not a reason to crop to nothing.
    #[test]
    fn an_entirely_transparent_logo_is_left_whole() {
        let path = std::env::temp_dir()
            .join(format!("lightforge-art-{}-empty.png", std::process::id()));
        image::RgbaImage::new(64, 32).save(&path).unwrap();
        let (handle, _, _) = logo(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(rgba_size(&handle), Some((64, 32)));
    }

    /// A mark inside a halo is measured by the mark, not by the halo.
    #[test]
    fn a_soft_halo_does_not_count_toward_a_logos_size() {
        let mut canvas = image::RgbaImage::new(200, 100);
        // A faint wash over the whole canvas, with a solid block in the middle.
        for pixel in canvas.pixels_mut() {
            *pixel = image::Rgba([255, 255, 255, 8]);
        }
        for y in 25..75 {
            for x in 50..150 {
                canvas.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let path =
            std::env::temp_dir().join(format!("lightforge-art-{}-halo.png", std::process::id()));
        canvas.save(&path).unwrap();
        let (handle, aspect, ink) = logo(&path);
        let _ = std::fs::remove_file(&path);
        // The wash reaches the edges, so nothing is cropped and the aspect is
        // the canvas's, but only a quarter of it is the mark.
        assert_eq!(rgba_size(&handle), Some((200, 100)));
        assert!((aspect - 2.0).abs() < 0.01, "{aspect}");
        assert!((ink - 0.25).abs() < 0.01, "{ink}");
    }

    /// iced has no codecs of its own, so a path handed on would draw nothing
    /// anyway; what comes back is the one shared placeholder.
    #[test]
    fn unreadable_art_becomes_the_placeholder() {
        let path = std::env::temp_dir().join("lightforge-art-missing.jpg");
        let empty = placeholder();
        assert_eq!(load(&path, BACKGROUND_MAX_WIDTH).id(), empty.id());
        assert_eq!(logo(&path).0.id(), empty.id());
        assert!(try_mark(&path).is_none());

        let garbage = std::env::temp_dir()
            .join(format!("lightforge-art-{}-garbage.png", std::process::id()));
        std::fs::write(&garbage, b"\x89PNG not really").unwrap();
        let handle = scale(&garbage, 320, FilterType::Lanczos3);
        let _ = std::fs::remove_file(&garbage);
        assert_eq!(handle.id(), empty.id());
    }

    /// A tall JPEG is bounded by its height, in the decoder and in the fit.
    #[test]
    fn a_tall_jpeg_is_capped_by_its_height() {
        let path = write_jpeg("tall", 320, 8000);
        let decoded = decode(&path, 320).unwrap();
        let handle = scale(&path, 320, FilterType::Triangle);
        let _ = std::fs::remove_file(&path);
        assert!(decoded.height() < 8000, "decoded at full height ({})", decoded.height());
        assert_eq!(rgba_size(&handle), Some((25, 640)));
    }

    /// The generic path's allocation limit holds on the JPEG path too.
    #[test]
    fn a_jpeg_over_the_allocation_limit_is_refused() {
        // 40000x40000 RGB is 4.8 GB; even at an eighth it is 75 MB, so ask
        // for the full size and let only the limit stand in the way.
        let path = write_jpeg("huge-header", 8, 8);
        let mut bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        // Rewrite the frame header's height and width (SOF0, big-endian).
        let sof = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        bytes[sof + 5..sof + 7].copy_from_slice(&40000_u16.to_be_bytes());
        bytes[sof + 7..sof + 9].copy_from_slice(&40000_u16.to_be_bytes());
        let path = std::env::temp_dir()
            .join(format!("lightforge-art-{}-huge.jpg", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let result = decode(&path, u32::from(u16::MAX));
        let _ = std::fs::remove_file(&path);
        assert!(
            matches!(result, Err(image::ImageError::Limits(_))),
            "{:?}",
            result.map(|d| (d.width(), d.height())),
        );
    }
}

// ------------------------------------------------------- report screenshots --

/// Size and quality steps, tried in order until one fits [`SHOT_MAX_BYTES`].
///
/// The last pair is the floor and is sent whatever it weighs: a heavy
/// screenshot is better than none, and nothing anybody drags in here will get
/// past 720px at quality 45 anyway.
const SHOT_STEPS: [(u32, u8); 4] = [(1280, 75), (1280, 55), (960, 55), (720, 45)];

/// What an embedded screenshot may weigh before it is squeezed again.
///
/// It is embedded rather than attached because **GlitchTip throws envelope
/// attachments away**: `attachment` is in its hard-coded `IgnoredItemType`
/// list, so an attached image is parsed and dropped with no error. So the
/// image rides inside the event as base64, and the event has to stay a size
/// the server will accept: 400 KiB of JPEG is about 533 KiB of base64.
const SHOT_MAX_BYTES: usize = 400 * 1024;

/// A screenshot somebody chose for a bug report.
#[derive(Debug, Clone)]
pub struct Shot {
    /// The file's own name, which is what the report calls it.
    pub name: String,
    /// Encoded bytes exactly as they will be sent.
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
    pub size: (u32, u32),
    /// The same image, for the thumbnail in the dialog.
    pub preview: Handle,
}

/// Read an image from disk and re-encode it small enough to travel.
///
/// Fails rather than guesses: a file that is not an image at all is the common
/// case here, because the picker will hand over whatever was selected.
pub fn load_shot(path: &Path) -> Result<Shot, String> {
    let decoded = image::open(path).map_err(|e| format!("not an image: {e}"))?;
    let original = decoded.into_rgb8();

    let mut best: Option<(Vec<u8>, image::RgbImage)> = None;
    for (edge, quality) in SHOT_STEPS {
        let longest = original.width().max(original.height());
        let scaled = match longest > edge {
            true => {
                let scale = f64::from(edge) / f64::from(longest);
                let width = ((f64::from(original.width()) * scale) as u32).max(1);
                let height = ((f64::from(original.height()) * scale) as u32).max(1);
                image::imageops::resize(&original, width, height, FilterType::Triangle)
            }
            false => original.clone(),
        };

        // JPEG throughout: alpha is meaningless in a screenshot, and PNG of a
        // photographed monitor is several times the size for no gain.
        let mut out = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&scaled)
            .map_err(|e| format!("could not encode it: {e}"))?;
        let bytes = out.into_inner();

        let fits = bytes.len() <= SHOT_MAX_BYTES;
        best = Some((bytes, scaled));
        if fits {
            break;
        }
    }

    let (bytes, scaled) = best.expect("SHOT_STEPS is not empty");
    let size = (scaled.width(), scaled.height());
    // From the pixels already in hand rather than the JPEG: iced has no
    // decoder of its own, and these are the same pixels the encoder was given.
    let rgba = image::DynamicImage::ImageRgb8(scaled).into_rgba8();
    let preview = Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "screenshot".to_owned());

    Ok(Shot { name, preview, bytes, media_type: "image/jpeg", size })
}
