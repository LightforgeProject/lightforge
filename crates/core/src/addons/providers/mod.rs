//! Keyless addon providers, resolved client-side.
//!
//! These need no key, so they work without the proxy ([`super::catalog`]),
//! answering with the same [`Resolved`] shape.
//!
//! - [`wowinterface`]: a large catalogue, and an MD5 per download.
//! - [`tukui`]: the only source of ElvUI.
//! - [`github`]: many authors release here first; `release.json` picks the flavor.

pub mod github;
pub mod tukui;
pub mod wowinterface;

use super::catalog::{CatalogError, Request, Resolved, Status};
use super::Provider;
use crate::net::Http;

/// Resolve a batch that all belongs to one provider.
///
/// An unreachable provider turns its batch into `not_found` entries with a
/// reason; the other providers go on.
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
