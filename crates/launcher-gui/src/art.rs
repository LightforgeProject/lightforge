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

use std::path::Path;

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
/// and 128 at the drawn size, the differences past this point are not visible
/// — what remains is the artwork, not the scaling.
const MARK_MAX_WIDTH: u32 = (crate::theme::TILE * 2.0) as u32;

/// Widest a logo is kept. It is drawn at most a couple of hundred pixels tall.
const LOGO_MAX_WIDTH: u32 = 1024;

/// [`Branding`] with its images ready to draw.
#[derive(Debug, Clone)]
pub struct Art {
    pub branding: Branding,
    pub logo: Handle,
    /// The trimmed wordmark's width over its height.
    ///
    /// Carried because the dashboard sizes the logo by *area* rather than by
    /// a bounding box — see [`crate::theme::logo_size`]. `1.0` for a logo
    /// that could not be measured, which is a square box and no worse than
    /// any other guess.
    pub logo_aspect: f32,
    /// How much of the trimmed wordmark is the mark, and how much is glow.
    ///
    /// Some logos are drawn with a wide soft halo — Retail's is a third of
    /// its own bounding box — and a halo is not something the eye measures a
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
    /// Decode both images. Blocking and CPU-bound — call it off the UI thread.
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
/// Unlike [`load`]'s backgrounds this is a large reduction — 256px down to
/// 128 — so it is worth the sharper filter. The image is tiny and this runs
/// once per product per session.
pub fn mark(path: &Path) -> Handle {
    scale(path, MARK_MAX_WIDTH, FilterType::Lanczos3)
}

/// A headline's picture, at the size the News card draws it.
///
/// A large reduction — Wowhead publishes these around 800px wide and the card
/// shows them at [`crate::theme::NEWS_THUMB_WIDTH`] — so it takes the sharper
/// filter for the same reason [`mark`] does: handing the renderer eight times
/// the pixels and letting it reduce them is what reads as "pixelated".
///
/// Twice the drawn width, so a 2x display maps one texel to one device pixel
/// and a 1x display gets the clean 2:1 reduction.
pub fn thumbnail(path: &Path) -> Handle {
    scale(path, (crate::theme::NEWS_THUMB_WIDTH * 2.0) as u32, FilterType::Lanczos3)
}

/// Decode a bundled image that is already at the size it is drawn.
///
/// Bundled art goes through here rather than `Handle::from_bytes` for the
/// reason at the top of this file: a handle holding compressed bytes is
/// decoded by the renderer, which keeps the result only while it is drawing
/// and throws it away at the end of the frame. A 2560px JPEG re-decoded on
/// every return to the page is exactly the pause this module exists to
/// remove. Blocking and CPU-bound — call it off the UI thread.
pub fn decode_bundled(bytes: &'static [u8]) -> Handle {
    match image::load_from_memory(bytes) {
        Ok(decoded) => {
            let rgba = decoded.into_rgba8();
            Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
        }
        Err(e) => {
            crate::trace::mark(format_args!("bundled image failed to decode: {e}"));
            Handle::from_bytes(bytes)
        }
    }
}

/// A product wordmark, cropped to the pixels that are actually drawn.
///
/// Blizzard publishes every logo on the same 704x440 canvas, but what sits on
/// that canvas is not the same size from one game to the next: StarCraft's
/// wordmark is 127px tall inside it, retail WoW's is the full 440. Fitting
/// the *canvas* into a box therefore makes them all the same rectangle of
/// mostly-nothing and leaves the visible marks at wildly different sizes —
/// which is what the dashboard did, and what a per-product height hint in the
/// manifest was being hand-tuned to paper over.
///
/// So the transparent border comes off here, before the UI ever sees the
/// image. What reaches [`crate::theme::LOGO_BOX`] is the mark itself, and
/// fitting *that* is what makes two logos read as one size.
///
/// A logo with no transparent border is returned unchanged; one that is
/// entirely transparent is left alone rather than cropped to nothing.
///
/// Returns the image, its aspect, and its [`Art::logo_ink`] fraction.
fn logo(path: &Path) -> (Handle, f32, f32) {
    let decoded = match image::open(path) {
        Ok(decoded) => decoded,
        Err(e) => {
            crate::trace::mark(format_args!("decode failed for {}: {e}", path.display()));
            return (Handle::from_path(path), 1.0, 1.0);
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
/// number matters less than it sounds — Retail's bounds move 25px between an
/// eighth and two thirds, and every other logo in the catalogue moves less
/// than 5.
const SOLID_ALPHA: u8 = 32;

/// The smallest rectangle holding every pixel more opaque than `alpha`, as
/// `(x, y, width, height)`.
///
/// `None` when there is no such pixel — an all-transparent image, or one whose
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

/// Decode and downscale one image. An image this can't read falls back to
/// its path, leaving iced to fail on it the way it did before.
fn load(path: &Path, max_width: u32) -> Handle {
    // Triangle rather than Lanczos: a one-off downscale by less than 2x,
    // where the sharper filter costs several times as long for no difference
    // behind a scrim.
    scale(path, max_width, FilterType::Triangle)
}

fn scale(path: &Path, max_width: u32, filter: FilterType) -> Handle {
    let decoded = match image::open(path) {
        Ok(decoded) => decoded,
        Err(e) => {
            crate::trace::mark(format_args!("decode failed for {}: {e}", path.display()));
            return Handle::from_path(path);
        }
    };

    let decoded = if decoded.width() > max_width {
        let height = (u64::from(decoded.height()) * u64::from(max_width)
            / u64::from(decoded.width())) as u32;
        decoded.resize_exact(max_width, height.max(1), filter)
    } else {
        decoded
    };

    let rgba = decoded.into_rgba8();
    Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
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
        // the canvas's — but only a quarter of it is the mark.
        assert_eq!(rgba_size(&handle), Some((200, 100)));
        assert!((aspect - 2.0).abs() < 0.01, "{aspect}");
        assert!((ink - 0.25).abs() < 0.01, "{ink}");
    }

    #[test]
    fn unreadable_art_falls_back_to_its_path() {
        let path = std::env::temp_dir().join("lightforge-art-missing.jpg");
        assert!(matches!(
            load(&path, BACKGROUND_MAX_WIDTH),
            Handle::Path(..)
        ));
        assert!(matches!(logo(&path), (Handle::Path(..), ..)));
    }
}

// ------------------------------------------------------- report screenshots --

/// The longest edge a screenshot is sent at.
///
/// A 4K grab of a launcher is 8 MB of PNG and tells nobody anything a 1920px
/// one does not. Anything already smaller is left exactly as it was.
const SHOT_MAX_EDGE: u32 = 1920;

/// Above this, the PNG is re-encoded as JPEG instead.
///
/// Screenshots of flat UI compress well as PNG and badly as JPEG, so PNG is
/// the default; a photograph of a monitor is the other way round, and this is
/// what catches it.
const SHOT_PNG_LIMIT: usize = 2 * 1024 * 1024;

/// A screenshot somebody dropped on the bug report dialog.
#[derive(Debug, Clone)]
pub struct Shot {
    /// The file's own name, which is what the attachment is called.
    pub name: String,
    /// Encoded bytes exactly as they will be sent.
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
    pub size: (u32, u32),
    /// The same image, for the thumbnail in the dialog.
    pub preview: Handle,
}

/// Read an image from disk, shrink it if it is enormous, and encode it.
///
/// Fails rather than guesses: a file that is not an image at all is the common
/// case here, because anything can be dropped on a window.
pub fn load_shot(path: &Path) -> Result<Shot, String> {
    let decoded = image::open(path).map_err(|e| format!("not an image: {e}"))?;
    let mut rgba = decoded.into_rgba8();

    let longest = rgba.width().max(rgba.height());
    if longest > SHOT_MAX_EDGE {
        let scale = f64::from(SHOT_MAX_EDGE) / f64::from(longest);
        let width = ((f64::from(rgba.width()) * scale) as u32).max(1);
        let height = ((f64::from(rgba.height()) * scale) as u32).max(1);
        rgba = image::imageops::resize(&rgba, width, height, FilterType::Triangle);
    }
    let size = (rgba.width(), rgba.height());

    let encode = |format: image::ImageOutputFormat| -> Result<Vec<u8>, String> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba.clone())
            .write_to(&mut out, format)
            .map_err(|e| format!("could not encode it: {e}"))?;
        Ok(out.into_inner())
    };

    let png = encode(image::ImageOutputFormat::Png)?;
    let (bytes, media_type) = match png.len() > SHOT_PNG_LIMIT {
        // JPEG has no alpha, and a screenshot has none worth keeping.
        true => {
            let rgb = image::DynamicImage::ImageRgba8(rgba).into_rgb8();
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(rgb)
                .write_to(&mut out, image::ImageOutputFormat::Jpeg(85))
                .map_err(|e| format!("could not encode it: {e}"))?;
            let jpeg = out.into_inner();
            match jpeg.len() < png.len() {
                true => (jpeg, "image/jpeg"),
                false => (png, "image/png"),
            }
        }
        false => (png, "image/png"),
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "screenshot".to_owned());
    let name = match media_type {
        "image/jpeg" => format!("{name}.jpg"),
        _ => name,
    };

    Ok(Shot { name, preview: Handle::from_bytes(bytes.clone()), bytes, media_type, size })
}
