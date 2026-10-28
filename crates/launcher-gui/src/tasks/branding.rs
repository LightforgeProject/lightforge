//! Key art, logos and icons: fetched, cached and prefetched.

use super::*;

/// Branding straight off disk, for the frame before [`theme`] answers.
///
/// Network-free and typically a few tens of milliseconds, so on every run
/// after the first the dashboard opens already themed instead of showing its
/// unthemed card for the length of a revalidation.
pub async fn cached_theme(
    pipeline: Option<Pipeline>,
    codes: Vec<ProductCode>,
    code: ProductCode,
) -> Themed {
    let Some(pipeline) = pipeline else {
        return Themed::default();
    };

    // Reads and hashes every image it returns; off the UI thread.
    tokio::task::spawn_blocking(move || {
        let branding = pipeline.cached(&code).map(Art::decode);
        let manifest = pipeline.cached_manifest();
        let icons = match &manifest {
            Some(manifest) => cached_icons(&pipeline, manifest, &codes),
            None => HashMap::new(),
        };
        let warm = manifest.as_ref().is_some_and(|manifest| {
            let wants_art = manifest.slot(&code, |t| t.logo.as_ref()).is_some()
                && manifest.slot(&code, |t| t.background.as_ref()).is_some();
            let wants_icons = codes
                .iter()
                .filter(|c| manifest.own_theme(c).is_some_and(|t| t.icon.is_some()))
                .count();
            (!wants_art || branding.is_some()) && icons.len() == wants_icons
        });
        Themed { manifest: manifest.map(Arc::new), branding, icons, warm }
    })
    .await
    .unwrap_or_default()
}

/// Every product's cached icon, skipping the ones that aren't there yet.
pub(crate) fn cached_icons(
    pipeline: &Pipeline,
    manifest: &Manifest,
    codes: &[ProductCode],
) -> HashMap<ProductCode, Mark> {
    codes
        .iter()
        .filter_map(|code| {
            let icon = pipeline.cached_icon(manifest, code)?;
            Some((code.clone(), Mark::decode(icon.path)))
        })
        .collect()
}

/// A theming pass: the branding it produced, the manifest it read, and every
/// configured product's bar icon.
///
/// The manifest comes back so the app can hold onto it; see [`theme`]. The
/// icons ride along because the pass already holds the one manifest that names
/// them all, and the product bar needs the whole set, not just the selected
/// product's.
#[derive(Debug, Default, Clone)]
pub struct Themed {
    pub manifest: Option<Arc<Manifest>>,
    pub branding: Option<Art>,
    pub icons: HashMap<ProductCode, Mark>,
    /// Set only by the cache-only pass: everything the first screen draws
    /// was on disk, the selected game's art and every product's mark, so the
    /// splash has nothing left to wait for.
    pub warm: bool,
}

/// A product's decoded mark, beside the file it came from.
///
/// The path is kept for one reason: to tell whether a theming pass produced
/// *different* marks or the same ones again. [`Handle::from_path`] used to
/// make that free, because iced caches those by path and reassigning one costs
/// nothing. A handle built from pixels is a fresh id every time, so assigning
/// an identical set makes the renderer re-upload every texture, which is the
/// band blinking after a switch. Comparing paths answers it exactly, the same
/// way [`Art`] compares its [`lightforge_core::assets::Branding`].
#[derive(Debug, Clone)]
pub struct Mark {
    pub path: PathBuf,
    pub handle: Handle,
}

impl Mark {
    pub(crate) fn decode(path: PathBuf) -> Self {
        let handle = crate::art::mark(&path);
        Self { path, handle }
    }
}

/// Whether two icon sets name the same files, regardless of handle identity.
pub fn same_marks(a: &HashMap<ProductCode, Mark>, b: &HashMap<ProductCode, Mark>) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(code, mark)| b.get(code).is_some_and(|other| other.path == mark.path))
}

/// Nothing, after a wait. The one thing a `Task` cannot express on its own.
pub async fn after(delay: std::time::Duration) {
    tokio::time::sleep(delay).await;
}

/// How many products a prefetch fetches at once.
pub(crate) const PREFETCH_AT_ONCE: usize = 4;

/// [`prefetch`] under a deadline: a start must not be hostage to a network
/// that never answers. What lands in time stays cached; the rest is fetched
/// on demand.
pub async fn prefetch_within(
    budget: std::time::Duration,
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    codes: Vec<ProductCode>,
) -> usize {
    match tokio::time::timeout(budget, prefetch(http, pipeline, codes)).await {
        Ok(cached) => cached,
        Err(_) => {
            crate::trace::mark("prefetch: out of time, opening anyway");
            0
        }
    }
}

/// Fetch every product's artwork into the cache, without decoding any of it.
///
/// The first run's "it looks weird for the first second or two on every game"
/// fix: a panel's background and wordmark are revalidated the first time it is
/// opened, and until they land the card is empty. Pulling them all down while
/// the wizard's first slide is on screen means every panel opens from disk.
///
/// Deliberately not decoded and not kept: decoding thirteen 4K backgrounds to
/// hold them all in memory would cost more than it saves, and [`cached_theme`]
/// reads them off disk per product in tens of milliseconds. Returns how many
/// products now have art cached.
pub async fn prefetch(
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    codes: Vec<ProductCode>,
) -> usize {
    let (Some(http), Some(pipeline)) = (http, pipeline) else {
        return 0;
    };
    let manifest = match pipeline.manifest(&http).await {
        Ok(manifest) => manifest,
        Err(e) => {
            crate::trace::mark(format_args!("prefetch: no manifest ({e})"));
            tracing::warn!("artwork: no asset manifest: {}", chain(&e));
            return 0;
        }
    };
    use futures_util::StreamExt;

    // A few products at a time. This only runs with art missing, usually a
    // first run, where everything is a real download: one at a time made the
    // splash the sum of every transfer, and all at once would open a dozen
    // connections to one host (no HTTP/2 here) fighting over a slow link.
    //
    // Each future owns its handles: borrowing them across `buffer_unordered`
    // trips the compiler's higher-ranked `Send` check on the task.
    let manifest = Arc::new(manifest);
    let total = codes.len();
    let cached = futures_util::stream::iter(codes)
        .map(|code| {
            let (http, pipeline, manifest) = (http.clone(), pipeline.clone(), Arc::clone(&manifest));
            async move {
                match pipeline.resolve_with(&http, &manifest, &code).await {
                    // Branding carries the icon; asking again is a round trip.
                    Ok(_) => true,
                    // A hand-off has a mark and no art.
                    Err(_) => {
                        let _ = pipeline.icon(&http, &manifest, &code).await;
                        false
                    }
                }
            }
        })
        .buffer_unordered(PREFETCH_AT_ONCE)
        .filter(|resolved| std::future::ready(*resolved))
        .count()
        .await;
    crate::trace::mark(format_args!("prefetched art for {cached}/{total} products"));
    cached
}

/// Resolve a product's branding, or an empty [`Themed`] when theming isn't
/// configured or the manifest can't be read.
///
/// The manifest is fetched once per session rather than once per product
/// switch: one document describes every product, so re-asking for it on each
/// switch buys a round trip and nothing else. Pass the cached copy back in and
/// only the two images are revalidated. The app drops its copy when the asset
/// source changes, which is the only thing that can invalidate it mid-session.
pub async fn theme(
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    manifest: Option<Arc<Manifest>>,
    codes: Vec<ProductCode>,
    code: ProductCode,
) -> Themed {
    let (Some(http), Some(pipeline)) = (http, pipeline) else {
        crate::trace::mark("theme skipped: no client or no asset source configured");
        return Themed::default();
    };

    let manifest = match manifest {
        Some(cached) => cached,
        None => match pipeline.manifest(&http).await {
            Ok(fresh) => {
                // Only on a fresh manifest, which is the only moment the full
                // set of live URLs is known. Published art is
                // content-addressed, so a changed image arrives under a new
                // URL and the old cache file would otherwise sit there
                // forever, a few megabytes per revision.
                let removed = pipeline.prune_cache(&fresh);
                if removed > 0 {
                    crate::trace::mark(format_args!("pruned {removed} stale cached assets"));
                }
                Arc::new(fresh)
            }
            Err(e) => {
                crate::trace::mark(format_args!("manifest fetch failed: {e}"));
                return Themed::default();
            }
        },
    };

    // A product missing from the manifest is not a reason to throw the
    // manifest away, so the branding half fails on its own.
    let branding = match pipeline.resolve_with(&http, &manifest, &code).await {
        Ok(branding) => Some(branding),
        Err(e) => {
            crate::trace::mark(format_args!("branding failed for {}: {e}", code.as_str()));
            None
        }
    };

    // Every product's mark, not just the selected one's: the bar shows them
    // all at once. They are small and revalidate to a 304, so the whole set
    // costs about what one image does.
    let mut icons = HashMap::new();
    for product in &codes {
        if let Ok(Some(icon)) = pipeline.icon(&http, &manifest, product).await {
            // Scaled here rather than handed over as a path: the band draws
            // these at 40pt, and letting the renderer reduce a 256px image
            // that far is what made them look pixelated.
            icons.insert(product.clone(), Mark::decode(icon.path));
        }
    }

    // Decoded here, off the UI thread, so the dashboard never decodes on
    // draw; see [`crate::art`].
    let branding = match branding {
        Some(branding) => tokio::task::spawn_blocking(move || Art::decode(branding)).await.ok(),
        None => None,
    };

    Themed { manifest: Some(manifest), branding, icons, warm: false }
}

#[cfg(test)]
mod mark_tests {
    use super::*;

    fn mark(path: &str) -> Mark {
        Mark { path: PathBuf::from(path), handle: Handle::from_rgba(1, 1, vec![0, 0, 0, 0]) }
    }

    /// The regression this guards: icons became decoded handles, and a handle
    /// built from pixels is a fresh id every time. Reassigning an identical
    /// set makes the renderer re-upload every texture, which is the band
    /// blinking a second after each product switch, the same fault the
    /// branding swap was fixed for in October.
    #[test]
    fn identical_icon_sets_compare_equal_despite_new_handles() {
        let retail = ProductCode::from("wow".to_owned());
        let a = HashMap::from([(retail.clone(), mark("/cache/abc.png"))]);
        let b = HashMap::from([(retail.clone(), mark("/cache/abc.png"))]);
        assert!(same_marks(&a, &b), "same file must compare equal");

        let c = HashMap::from([(retail, mark("/cache/def.png"))]);
        assert!(!same_marks(&a, &c), "a different file must swap");
    }

    #[test]
    fn a_changed_set_size_is_not_the_same() {
        let a = HashMap::from([(ProductCode::from("wow".to_owned()), mark("/x.png"))]);
        let b = HashMap::new();
        assert!(!same_marks(&a, &b));
        assert!(!same_marks(&b, &a));
    }
}
