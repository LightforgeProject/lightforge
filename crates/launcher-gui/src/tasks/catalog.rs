//! What is out there and what is installed: versions, offerings, registry.

use super::*;

/// One product's situation, as of a refresh.
#[derive(Debug, Clone)]
pub struct Refreshed {
    pub local: Local,
    pub remote: Option<Version>,
    /// Why the remote half is missing, if it is. Not an error state: an
    /// unreachable version service must not stop a complete install launching.
    pub offline: Option<String>,
}

/// Returned in place of a network error when the shared client never built.
pub const NO_HTTP: &str = "no HTTP client available";

/// Read the disk and, when `check_remote`, ask the version service what is
/// current.
///
/// `check_remote` is `false` for a product Blizzard does not serve us builds
/// for: the probe can only fail there, and would report itself as the version
/// service being down.
pub async fn refresh(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    region: Region,
    check_remote: bool,
) -> Refreshed {
    let local = {
        let layout = layout.clone();
        let code = code.clone();
        // Opening the store to count files is an index read, but on a 123 GB
        // install it is still tens of milliseconds, off the UI thread.
        tokio::task::spawn_blocking(move || Local::inspect(&layout, &code))
            .await
            .unwrap_or_default()
    };

    if !check_remote {
        return Refreshed { local, remote: None, offline: None };
    }

    let (remote, offline) = match &http {
        Some(http) => match version::versions(http, region, code.as_str()).await {
            Ok(rows) => (
                version::for_region(&rows, region, |v| &v.region).cloned(),
                None,
            ),
            Err(e) => (None, Some(chain(&e))),
        },
        None => (None, Some(NO_HTTP.to_owned())),
    };

    Refreshed { local, remote, offline }
}

/// What a build is and what it offers, for the Check for Updates action.
///
/// Much lighter than planning: the download manifest only, so pressing the
/// button costs one manifest rather than the whole prelude a download needs.
pub async fn offerings(
    http: Option<Http>,
    code: ProductCode,
    region: Region,
    optional_tags: Vec<String>,
    store: Option<PathBuf>,
) -> Result<lightforge_core::tact::install::Offerings, String> {
    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let options = InstallOptions {
        region,
        flavor: code.install_dir().into_owned(),
        optional_tags,
        ..InstallOptions::default()
    };
    // Reading the indices is disk work, so it stays off the runtime's
    // threads; a store that will not open is priced as empty.
    let probe = match store {
        Some(dir) => tokio::task::spawn_blocking(move || install::StoreProbe::open(&dir))
            .await
            .ok(),
        None => None,
    };
    install::offerings(&http, &code, &options, probe.as_ref())
        .await
        .map_err(|e| chain(&e))
}

/// Ask GitHub whether a newer Lightforge exists.
///
/// Runs beside the splash's artwork fetch. Answers `None` for every reason
/// not to interrupt someone: the check is off, no connection, GitHub errors,
/// the list does not parse, or this is already the newest build.
pub async fn check_for_update(
    http: Option<Http>,
    enabled: bool,
) -> Option<lightforge_core::update::Available> {
    // A development build is ahead of every release or about to be, and
    // its own checkout is how it updates.
    if !enabled || cfg!(debug_assertions) {
        return None;
    }
    match latest_release(http?).await {
        Ok(found) => {
            tracing::info!("update check: {}", found.as_ref().map_or("up to date", |_| "a newer release exists"));
            found
        }
        Err(e) => {
            tracing::warn!("update check failed: {e}");
            None
        }
    }
}

/// The same question, asked because someone pressed the button in Settings.
///
/// Unlike the start-up check it says why when it cannot answer, and it runs
/// in a development build too: pressing the button is the request, not an
/// interruption.
pub async fn check_for_update_now(
    http: Option<Http>,
) -> Result<Option<lightforge_core::update::Available>, String> {
    let http = http.ok_or_else(|| "no network connection".to_owned())?;
    latest_release(http).await
}

async fn latest_release(http: Http) -> Result<Option<lightforge_core::update::Available>, String> {
    use lightforge_core::net::ETagCache;
    use lightforge_core::update;

    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    // Shares GitHub's 60-per-hour unauthenticated budget with the Proton-GE
    // check, so the ETag is what keeps a start from costing two of it.
    let cache = ETagCache::new(dirs.asset_cache_dir().join("lightforge-releases.json"));
    let releases = update::fetch(&http, &cache).await.map_err(|e| e.to_string())?;
    // `LIGHTFORGE_PRETEND_VERSION` is for development, like
    // `LIGHTFORGE_OS_RELEASE`: a build that calls itself older is offered
    // the real newest release, which is the only way to walk an update
    // through before the next one exists.
    let running = std::env::var("LIGHTFORGE_PRETEND_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    let found = update::newer_than(&running, &releases);
    crate::trace::mark(match &found {
        Some(available) => format!("update: {} is out", available.latest),
        None => "update: this is the newest build".to_owned(),
    });
    Ok(found)
}

/// Read the client's cached catalogue, for the game families in it.
///
/// About 25 ms against a real cache, and only at startup: the answer changes
/// when Blizzard publishes a new title, not while the launcher is open.
pub async fn game_families(layout: Layout, user: String, region: Region) -> Families {
    tokio::task::spawn_blocking(move || Families::read(&layout, &user, region))
        .await
        .unwrap_or_default()
}

/// The product codes Battle.net's own config lists, so the catalog can use
/// the codes the client uses rather than the ones our table happens to carry.
pub async fn known_codes(layout: Layout, user: String) -> Vec<String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::battlenet::known_product_codes(&layout, &user)
    })
    .await
    .unwrap_or_default()
}

/// Index Battle.net's registry by every name a record answers to.
///
/// The registry is keyed by install key, the tiles by CDN code, and the two
/// differ for Hearthstone (`hs_beta` vs `hsb`), which left it reading Install
/// forever. A record with no version is skipped: Agent writes the row before
/// the download finishes.
pub(crate) fn by_every_name(
    installs: Vec<lightforge_core::tact::productdb::Installed>,
) -> HashMap<String, String> {
    let mut found = HashMap::new();
    for install in installs {
        let Some(version) = install.version else { continue };
        if !install.product_code.is_empty() && install.product_code != install.product {
            found.insert(install.product_code, version.clone());
        }
        found.insert(install.product, version);
    }
    found
}

/// What Battle.net reports as installed, keyed by product code.
pub async fn installed_versions(layout: Layout) -> HashMap<String, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::tact::productdb;

        by_every_name(productdb::read(&layout.agent_dir().join("product.db")).unwrap_or_default())
    })
    .await
    .unwrap_or_default()
}

/// When Agent's registry last changed, and, when asked, whether the
/// Battle.net client is up. Polled while [`RegistryWatch`] runs: a stat is
/// nothing, the process scan is ~60 ms, so the caller asks for it rarely.
///
/// [`RegistryWatch`]: crate::RegistryWatch
pub async fn registry_probe(
    layout: Layout,
    check_client: bool,
) -> (Option<std::time::SystemTime>, Option<bool>) {
    tokio::task::spawn_blocking(move || {
        let stamp = std::fs::metadata(layout.agent_dir().join("product.db"))
            .and_then(|m| m.modified())
            .ok();
        let running = check_client.then(|| lightforge_core::preflight::client_running(&layout));
        (stamp, running)
    })
    .await
    .unwrap_or((None, None))
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use lightforge_core::tact::productdb::Installed;

    fn record(product: &str, product_code: &str, version: Option<&str>) -> Installed {
        Installed {
            product: product.to_owned(),
            product_code: product_code.to_owned(),
            install_path: "C:/Program Files (x86)/Whatever".to_owned(),
            version: version.map(str::to_owned),
            build_key: None,
            family: String::new(),
        }
    }

    /// Gabriel's bug, 2026-10-10: Hearthstone installed through Battle.net,
    /// located in the client, and the tile still read `Install via
    /// Battle.net`. The registry calls it `hs_beta`; the tile asks for `hsb`.
    #[test]
    fn hearthstone_answers_to_the_name_its_tile_uses() {
        let found = by_every_name(vec![record("hs_beta", "hsb", Some("36.6.3.253932"))]);

        assert_eq!(found.get("hsb").map(String::as_str), Some("36.6.3.253932"));
        // And still under the name the registry itself uses, which is what
        // anything reading Agent's own file will hold.
        assert_eq!(found.get("hs_beta").map(String::as_str), Some("36.6.3.253932"));
    }

    #[test]
    fn a_product_whose_names_agree_is_indexed_once() {
        let found = by_every_name(vec![record("wow", "wow", Some("12.1.0.69933"))]);
        assert_eq!(found.len(), 1);
        assert_eq!(found.get("wow").map(String::as_str), Some("12.1.0.69933"));
    }

    /// A row Agent wrote before the download finished is not an install.
    #[test]
    fn a_record_without_a_version_is_not_installed() {
        let found = by_every_name(vec![record("hs_beta", "hsb", None)]);
        assert!(found.is_empty(), "{found:?}");
    }
}
