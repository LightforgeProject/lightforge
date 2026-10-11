//! Keyless addon providers, resolved client-side.
//!
//! CurseForge needs a key and therefore a server (see [`super::catalog`]).
//! These three do not, so Lightforge talks to them directly, which means addon
//! management keeps working when the Tuxcore proxy is down, and works at all
//! before a CurseForge key exists.
//!
//! Each provider answers with the same [`Resolved`] shape the catalog does, so
//! everything downstream, download, extract, ledger, is provider-agnostic.
//!
//! What each one is for:
//!
//! - [`wowinterface`], a large back catalogue, and it hands back an MD5, so
//!   its downloads are the only ones we can verify.
//! - [`tukui`], the *only* source for ElvUI, which is not on CurseForge at
//!   all. It also declares its folder list up front.
//! - [`github`], where many authors release first, and the BigWigs packager's
//!   `release.json` makes per-flavor selection exact rather than guessed.

pub mod github;
pub mod tukui;
pub mod wowinterface;

use super::catalog::{CatalogError, Request, Resolved, Status};
use super::Provider;
use crate::net::Http;

/// Resolve a batch that all belongs to one provider.
///
/// Errors are per-provider, not per-addon: a provider being unreachable turns
/// its whole batch into `not_found` entries with a reason rather than failing
/// the run, because the other providers' addons are still perfectly installable.
pub async fn resolve(
    http: &Http,
    provider: &Provider,
    requests: &[Request],
    flavor: &str,
) -> Vec<Resolved> {
    let result = match provider {
        Provider::WowInterface => wowinterface::resolve(http, requests).await,
        Provider::TukUI => tukui::resolve(http, requests).await,
        Provider::Wago => Ok(unsupported(requests, "Wago needs an API key Lightforge does not have")),
        Provider::GitHub => github::resolve(http, requests, flavor).await,
        Provider::Curse => Ok(unsupported(
            requests,
            "CurseForge is resolved through the catalog, not directly",
        )),
        Provider::Other(name) => Ok(unsupported(requests, &format!("unknown provider `{name}`"))),
    };

    match result {
        Ok(resolved) => resolved,
        Err(e) => unsupported(requests, &e.to_string()),
    }
}

/// Turn a batch into "could not resolve" entries carrying one explanation.
pub(crate) fn unsupported(requests: &[Request], reason: &str) -> Vec<Resolved> {
    requests
        .iter()
        .map(|request| Resolved {
            id: request.id.clone(),
            status: Status::NotFound,
            note: Some(reason.to_owned()),
            ..Resolved::default()
        })
        .collect()
}

/// An empty [`Resolved`] for `id`, for providers to fill in.
pub(crate) fn blank(id: &str) -> Resolved {
    Resolved {
        id: id.to_owned(),
        ..Resolved::default()
    }
}

/// Not found, with a reason worth showing.
pub(crate) fn missing(id: &str, reason: impl Into<String>) -> Resolved {
    Resolved {
        status: Status::NotFound,
        note: Some(reason.into()),
        ..blank(id)
    }
}

pub type ProviderResult = Result<Vec<Resolved>, CatalogError>;
