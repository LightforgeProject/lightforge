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
    pub background: Handle,
}

impl Art {
    /// Decode both images. Blocking and CPU-bound — call it off the UI thread.
    pub fn decode(branding: Branding) -> Self {
        let logo = load(&branding.logo.path, LOGO_MAX_WIDTH);
        let background = load(&branding.background.path, BACKGROUND_MAX_WIDTH);
        Self {
            branding,
            logo,
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

    #[test]
    fn unreadable_art_falls_back_to_its_path() {
        let path = std::env::temp_dir().join("lightforge-art-missing.jpg");
        assert!(matches!(
            load(&path, BACKGROUND_MAX_WIDTH),
            Handle::Path(..)
        ));
    }
}
