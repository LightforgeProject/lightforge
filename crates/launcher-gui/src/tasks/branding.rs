//! Key art, logos and icons: fetched, cached and prefetched.

use super::*;

/// Branding straight off disk, for the frame before [`theme`] answers.
///
/// Network-free, tens of milliseconds.
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
/// The manifest comes back for the app to keep ([`theme`]).
#[derive(Debug, Default, Clone)]
pub struct Themed {
    pub manifest: Option<Arc<Manifest>>,
    pub branding: Option<Art>,
    pub icons: HashMap<ProductCode, Mark>,
    /// Set by the cache-only pass when the first screen's art was all on disk.
    pub warm: bool,
}

/// A product's decoded mark, beside the file it came from.
///
/// The path tells an identical set of marks from a new one: a pixel handle is
/// a fresh id, and reassigning re-uploads every texture (the band blinks).
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
/// So every panel opens from disk on a first run. Not decoded: thirteen 4K
/// backgrounds in memory cost more than [`cached_theme`]'s disk read. Returns
/// how many products have art cached.
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

    // A few at a time (no HTTP/2 here). Each future owns its handles, or
    // `buffer_unordered` fails the higher-ranked `Send` check.
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
/// Pass the session's manifest back in and only the two images are revalidated.
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
                // Only on a fresh manifest.
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

    // Every product's mark: the bar shows them all.
    let mut icons = HashMap::new();
    for product in &codes {
        if let Ok(Some(icon)) = pipeline.icon(&http, &manifest, product).await {
            // Scaled here: the renderer's reduction pixelates.
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

    /// An identical set reassigned made the band blink after each switch.
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
