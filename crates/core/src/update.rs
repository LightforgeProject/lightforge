//! Is there a newer Lightforge?
//!
//! Asked once per start; installing is [`crate::upgrade`]'s. The full release
//! list, not `releases/latest` (which skips prereleases), compared by
//! [`semver`] precedence (`1.0.0-beta.10` > `1.0.0-beta.2`).

use serde::{Deserialize, Serialize};

use crate::net::{ETagCache, Http, NetError};

/// GitHub Releases endpoint for Lightforge itself.
const RELEASES_URL: &str = "https://api.github.com/repos/LightforgeProject/lightforge/releases";

/// Where to send someone who wants the new build.
pub const RELEASES_PAGE: &str = "https://github.com/LightforgeProject/lightforge/releases";

/// One published release, as much of it as this needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// Tag as published, e.g. `v1.0.0-beta.2`.
    pub tag_name: String,
    /// GitHub's flag, which can disagree with the version string.
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
}

/// A newer release than the one running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    /// The version running, as the binary was built with it.
    pub current: String,
    /// The version found, spelled as SemVer, not as the tag.
    pub latest: String,
    /// That release's own page, or the releases page when it has none.
    pub url: String,
}

/// Fetch the release list, revalidating with `cache`.
///
/// Shares GitHub's 60/hour budget with the Proton check; a `304` is free.
pub async fn fetch(http: &Http, cache: &ETagCache) -> Result<Vec<Release>, UpdateError> {
    match http.get_json_cached(RELEASES_URL, cache).await? {
        Some(releases) => Ok(releases),
        None => Ok(cache.read_json()?),
    }
}

/// Tags are published as `v1.0.0-beta.2`; SemVer does not want the `v`.
fn version_of(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches('v')).ok()
}

/// The newest release worth telling someone running `current` about.
///
/// A prerelease build hears about anything newer; a stable build only about
/// stable releases. `None` when current, or when `current` is no version (a
/// development build).
pub fn newer_than(current: &str, releases: &[Release]) -> Option<Available> {
    let here = version_of(current)?;
    let wants_prereleases = !here.pre.is_empty();

    releases
        .iter()
        .filter_map(|release| version_of(&release.tag_name).map(|version| (version, release)))
        // Stable only if both the tag and GitHub's flag say so.
        .filter(|(version, release)| {
            wants_prereleases || (version.pre.is_empty() && !release.prerelease)
        })
        // `cmp_precedence`: `Ord` counts build metadata (`1.0.0+2` > `1.0.0`).
        .filter(|(version, _)| version.cmp_precedence(&here) == std::cmp::Ordering::Greater)
        .max_by(|(a, _), (b, _)| a.cmp_precedence(b))
        .map(|(version, release)| Available {
            current: here.to_string(),
            latest: version.to_string(),
            url: match release.html_url.is_empty() {
                true => RELEASES_PAGE.to_owned(),
                false => release.html_url.clone(),
            },
        })
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Net(#[from] NetError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.to_owned(),
            prerelease,
            html_url: format!("https://example.invalid/{tag}"),
        }
    }

    #[test]
    fn a_beta_is_told_about_the_next_beta() {
        let releases = vec![release("v1.0.0-beta.3", true), release("v1.0.0-beta.2", true)];
        let found = newer_than("1.0.0-beta.2", &releases).expect("beta.3 is newer");
        assert_eq!(found.latest, "1.0.0-beta.3");
        assert_eq!(found.url, "https://example.invalid/v1.0.0-beta.3");
    }

    /// The reason this cannot be a string compare, and the reason
    /// `releases/latest` cannot be used either.
    #[test]
    fn ten_is_newer_than_two_and_the_release_is_newer_than_both() {
        let releases = vec![
            release("v1.0.0-beta.2", true),
            release("v1.0.0-beta.10", true),
        ];
        assert_eq!(
            newer_than("1.0.0-beta.2", &releases).unwrap().latest,
            "1.0.0-beta.10",
            "string order would put beta.10 before beta.2"
        );

        let shipped = vec![release("v1.0.0", false), release("v1.0.0-beta.10", true)];
        assert_eq!(newer_than("1.0.0-beta.10", &shipped).unwrap().latest, "1.0.0");
    }

    #[test]
    fn a_stable_build_is_not_pushed_at_prereleases() {
        let releases = vec![release("v1.1.0-beta.1", true), release("v1.0.0", false)];
        assert!(newer_than("1.0.0", &releases).is_none());

        // A tag that reads as stable but was published as a prerelease is
        // still a prerelease: either opinion is enough to hold it back.
        let mislabelled = vec![release("v1.1.0", true)];
        assert!(newer_than("1.0.0", &mislabelled).is_none());
        // And the same release does reach someone already on a beta.
        assert_eq!(newer_than("1.0.0-beta.1", &mislabelled).unwrap().latest, "1.1.0");
    }

    #[test]
    fn the_newest_wins_whatever_order_the_list_arrives_in() {
        let releases = vec![
            release("v1.0.0-beta.1", true),
            release("v1.0.0-beta.9", true),
            release("v1.0.0-beta.4", true),
        ];
        assert_eq!(newer_than("1.0.0-beta.1", &releases).unwrap().latest, "1.0.0-beta.9");
    }

    #[test]
    fn being_current_says_nothing_and_neither_does_nonsense() {
        let releases = vec![release("v1.0.0-beta.2", true)];
        assert!(newer_than("1.0.0-beta.2", &releases).is_none(), "already on it");
        assert!(newer_than("1.0.0-beta.3", &releases).is_none(), "ahead of it");
        assert!(newer_than("not-a-version", &releases).is_none());
        assert!(newer_than("1.0.0-beta.2", &[release("nightly", true)]).is_none());
        assert!(newer_than("1.0.0-beta.2", &[]).is_none());
    }

    /// Build metadata is not part of precedence, so a rebuild of the same
    /// version is not an update.
    #[test]
    fn build_metadata_is_not_a_new_version() {
        let releases = vec![release("v1.0.0-beta.2+20261013", true)];
        assert!(newer_than("1.0.0-beta.2", &releases).is_none());
    }
}
