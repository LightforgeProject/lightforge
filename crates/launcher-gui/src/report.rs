//! Sending a bug report, or a note to the developer.
//!
//! Both go to our own GlitchTip as a Sentry event, and both only ever happen
//! because somebody filled in a dialog and pressed Send. There is no panic
//! hook and no automatic capture: the `panic` and `release-health` features
//! are off in `Cargo.toml` precisely so that nothing can be sent behind
//! anyone's back.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Our GlitchTip project. A Sentry DSN is a write-only ingest key and is meant
/// to ship inside the client, which is why this is a plain constant rather
/// than something read from the environment.
const DSN: &str = "https://788f3511837f42abade303476cdeef11@bugs.k3nny.tech/1";

/// How long to wait for the event to actually leave the machine. The dialog
/// tells someone their report was sent, so that has to be true rather than
/// hopeful — `capture` alone only queues it.
const FLUSH: Duration = Duration::from_secs(15);

/// What is being sent.
pub enum Kind {
    /// A bug, with the rendered debug bundle. Already scrubbed: every bundle
    /// is, because [`lightforge_core::debug::DebugInfo::render`] scrubs on the
    /// way out.
    Bug(String),
    /// A note about the app. No diagnostics: nothing is wrong with the
    /// machine, so none of its details are anyone's business.
    Feedback,
}

static CLIENT: OnceLock<Option<Arc<sentry::Client>>> = OnceLock::new();

/// The client, built the first time something is sent.
///
/// Nothing exists until then: no client, no transport thread, no connection to
/// our server. A launcher that never reports a bug never talks to us at all,
/// which is the consent model rather than a promise about it.
// `ClientOptions` cannot be built by a struct literal (see below), so the
// fields are assigned after the default; that is the only way available.
#[allow(clippy::field_reassign_with_default)]
fn client() -> Option<Arc<sentry::Client>> {
    CLIENT
        .get_or_init(|| {
            // Assembled field by field rather than with a struct literal
            // because `ClientOptions` is `#[non_exhaustive]`: a literal does
            // not compile against it, whatever the field list.
            let mut options = sentry::ClientOptions::default();
            options.release = sentry::release_name!();
            // The default, set anyway because it is load-bearing: it is what
            // stops the server recording the sender's IP address.
            options.send_default_pii = false;
            // Pinned, because the `contexts` integration fills an empty
            // `server_name` with the machine's hostname and does not consult
            // `send_default_pii` before doing it. A desktop hostname is
            // usually somebody's name.
            options.server_name = Some("redacted".into());

            // Parsed rather than handed to `from_config` as a string, which
            // panics on a malformed DSN. A typo in the constant should cost a
            // report, not the launcher.
            let Ok(dsn) = DSN.parse::<sentry::types::Dsn>() else { return None };
            options.dsn = Some(dsn);

            // `apply_defaults`, not the options alone: it is what installs
            // the transport and the `contexts` integration. Without it the
            // client has no way to send anything, reports `is_enabled()` as
            // false, and `flush` cheerfully returns true on the empty queue —
            // which looks exactly like a working client that cannot reach the
            // network.
            let client = sentry::Client::from_config(sentry::apply_defaults(options));
            client.is_enabled().then(|| Arc::new(client))
        })
        .clone()
}

/// An image sent with a report.
pub struct Shot {
    pub name: String,
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
}

/// One report, assembled by the dialog and handed over whole.
pub struct Report {
    pub kind: Kind,
    pub note: String,
    /// An address, a Discord name, or nothing. The one piece of personal data
    /// in either report, and it is here because they typed it.
    pub contact: String,
    /// Facts worth filtering issues by — product, Proton build, distro. Every
    /// one of them is already in the bundle the card showed; they are repeated
    /// here only so GlitchTip can group and search on them.
    pub tags: Vec<(String, String)>,
    pub shot: Option<Shot>,
    /// The last things the launcher said, oldest first.
    pub history: Vec<String>,
}

/// Send one report, and say whether it arrived.
pub async fn send(report: Report) -> bool {
    // `flush` parks until the transport thread has drained, so it belongs on
    // the blocking pool and not on a runtime worker.
    tokio::task::spawn_blocking(move || {
        let Some(client) = client() else { return false };
        let Report { kind, note, contact, tags, shot, history } = report;

        let mut event = sentry::protocol::Event { message: Some(note), ..Default::default() };

        for (key, value) in tags {
            event.tags.insert(key, value);
        }

        // Breadcrumbs rather than another block of text: GlitchTip draws them
        // as a timeline under the event, which is what they are.
        event.breadcrumbs = history
            .into_iter()
            .map(|message| sentry::protocol::Breadcrumb {
                message: Some(message),
                category: Some("status".to_owned()),
                ..Default::default()
            })
            .collect::<Vec<_>>()
            .into();

        // Inside the event, not beside it: GlitchTip's ingest lists
        // `attachment` among the envelope item types it ignores, so an
        // attached image is parsed and dropped without an error anywhere.
        // A data URI survives, and pastes straight into a browser's address
        // bar from the issue page.
        if let Some(shot) = shot {
            use base64::Engine as _;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&shot.bytes);
            event.extra.insert(
                "screenshot".to_owned(),
                format!("data:{};base64,{encoded}", shot.media_type).into(),
            );
            event.extra.insert("screenshot_name".to_owned(), shot.name.into());
        }

        let contact = contact.trim();
        if !contact.is_empty() {
            // An address goes in `email` and anything else in `username`, so
            // GlitchTip shows it in the place a reply would be looked for.
            // The test is deliberately crude — it only decides which of two
            // fields the same string lands in.
            let mut user = sentry::protocol::User::default();
            match contact.contains('@') {
                true => user.email = Some(contact.to_owned()),
                false => user.username = Some(contact.to_owned()),
            }
            event.user = Some(user);
        }
        match kind {
            Kind::Bug(debug) => {
                event.level = sentry::Level::Error;
                event.extra.insert("debug_info".to_owned(), debug.into());
            }
            Kind::Feedback => {
                event.level = sentry::Level::Info;
                event.tags.insert("type".to_owned(), "feedback".to_owned());
            }
        }

        client.capture_event(event, None);
        client.flush(Some(FLUSH))
    })
    .await
    .unwrap_or(false)
}
