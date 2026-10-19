//! News headlines and their images.

use super::*;

/// Fetch and decode the pictures for a set of headlines.
///
/// `have` is what is already drawn: those URLs are skipped entirely, so a
/// Refresh re-downloads nothing and, more importantly, mints no new handle
/// for a picture already on screen. `Handle::from_rgba` gets a fresh id every
/// call, so handing the view an identical-looking handle re-uploads the
/// texture and flashes. Returning only what is genuinely new is the guard.
///
/// Capped: a feed is forty items and forty images at once would saturate the
/// connection the downloader wants.
pub async fn news_images(
    http: Option<Http>,
    dir: PathBuf,
    urls: Vec<String>,
    have: HashSet<String>,
) -> Vec<(String, Handle)> {
    let Some(http) = http else {
        return Vec::new();
    };

    // Before fetching, not after: the pictures this feed no longer names are
    // dead the moment the feed arrives, and clearing them first means the
    // directory never holds two feeds' worth at once.
    {
        let dir = dir.clone();
        let keep = urls.clone();
        let _ = tokio::task::spawn_blocking(move || news::prune_images(&dir, &keep)).await;
    }

    let wanted: Vec<String> = urls.into_iter().filter(|url| !have.contains(url)).collect();

    let mut out = Vec::new();
    for chunk in wanted.chunks(NEWS_IMAGE_BATCH) {
        let fetched = futures_util::future::join_all(chunk.iter().map(|url| {
            let http = http.clone();
            let dir = dir.clone();
            let url = url.clone();
            async move {
                let path = news::image(&http, &dir, &url).await?;
                // Decoding and scaling is CPU-bound; off the UI thread.
                let handle = tokio::task::spawn_blocking(move || crate::art::thumbnail(&path))
                    .await
                    .ok()?;
                Some((url, handle))
            }
        }))
        .await;
        out.extend(fetched.into_iter().flatten());
    }
    out
}

/// How many headline pictures are fetched at once.
pub(crate) const NEWS_IMAGE_BATCH: usize = 6;

/// Decode the News page's bundled backdrop, once per session.
///
/// Off the UI thread: it is a 2560px JPEG, and decoding it on the render
/// thread is a visible stall on the frame that first shows the page.
pub async fn news_background(bytes: &'static [u8]) -> Handle {
    tokio::task::spawn_blocking(move || crate::art::decode_bundled(bytes))
        .await
        .unwrap_or_else(|_| crate::art::placeholder())
}

/// Fetch the News page's headlines.
///
/// Returns `None` only when there is genuinely nothing to show, no network
/// and nothing cached. Anything else, including a proxy that is down while a
/// cached copy exists, comes back as headlines that know whether they are
/// stale.
pub async fn headlines(
    http: Option<Http>,
    origin: news::Origin,
    cache_file: PathBuf,
) -> Option<news::Headlines> {
    let http = http?;
    news::headlines(&http, &origin, &cache_file).await
}
