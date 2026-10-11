//! Sending a bug report, or a note to the developer.
//!
//! Both go to our own GlitchTip as a Sentry event, and both only ever happen
//! because somebody filled in a dialog and pressed Send. There is no panic
//! hook and no automatic capture: the `panic` and `release-health` features
//! are off in `Cargo.toml` precisely so that nothing can be sent behind
//! anyone's back.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use lightforge_core::net::Http;

/// Our GlitchTip project. A Sentry DSN is a write-only ingest key and is meant
/// to ship inside the client, which is why this is a plain constant rather
/// than something read from the environment.
const DSN: &str = "https://788f3511837f42abade303476cdeef11@bugs.k3nny.tech/1";

/// How long to wait for the event to actually leave the machine. The dialog
/// tells someone their report was sent, so that has to be true rather than
/// hopeful, `capture` alone only queues it.
const FLUSH: Duration = Duration::from_secs(15);

/// Where a screenshot is uploaded, so the issue carries a link instead of a
/// megabyte of base64, GlitchTip drops envelope attachments, so this is the
/// only way the picture gets to be a picture. Our own host, the same one the
/// artwork manifest comes from.
///
/// `/upload` and not the bare namespace: the reverse proxy in front of it
/// routes `/shots/v1/*`, which matches nothing without a segment after `v1`,
/// so a POST to the bare path never reaches the service at all.
const SHOTS: &str = "https://lightforge.tuxcore.pro/shots/v1/upload";

/// How long the upload may take before the report goes without it.
///
/// Short on purpose. Nothing depends on it working: the picture travels
/// inside the event when it fails, exactly as it used to, so the only thing a
/// longer wait buys is a longer wait in front of someone who has already
/// pressed Send.
const UPLOAD: Duration = Duration::from_secs(20);

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

/// One report at a time.
///
/// [`Ours`] records the outcome of the send it just made so that `flush` can
/// report it, which is only exact while one report is in flight. Two at once
/// would have each reading the other's answer, and the dialog would tell
/// somebody their report arrived because a different one did.
static SENDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The launcher's own HTTP client, wearing sentry's transport trait.
///
/// sentry's own transport is a `reqwest` of a different major version than
/// the workspace's, which drags in a second HTTP client, `tower-http`, a
/// platform certificate verifier and, through feature unification on the
/// shared `rustls`: **a second crypto provider**, `aws-lc-sys`, built from C
/// for a client we already have. This is the whole of what that bought:
/// serialize the envelope, post it with the auth header, say whether it
/// worked.
///
/// The send is synchronous because the only caller is already on a blocking
/// thread (see [`send`]), and that is what makes `flush` able to answer
/// truthfully with nothing queued.
///
/// What it deliberately does not do: honour `x-sentry-rate-limits` or retry.
/// Nothing here sends by itself, a report exists because somebody filled in
/// a dialog and pressed Send, and the dialog already refuses the same report
/// twice in five minutes.
struct Ours {
    http: Http,
    runtime: tokio::runtime::Handle,
    url: String,
    auth: String,
    /// What the last send did, read by `flush`.
    outcome: std::sync::Mutex<bool>,
}

impl sentry::Transport for Ours {
    fn send_envelope(&self, envelope: sentry::Envelope) {
        let mut body = Vec::new();
        let ok = envelope.to_writer(&mut body).is_ok()
            && self
                .runtime
                .block_on(self.http.post(
                    &self.url,
                    "application/x-sentry-envelope",
                    &[("X-Sentry-Auth", self.auth.as_str())],
                    body,
                    FLUSH,
                ))
                .is_ok();
        *self.outcome.lock().unwrap_or_else(|e| e.into_inner()) = ok;
    }

    fn flush(&self, _timeout: Duration) -> bool {
        // Nothing is ever queued: `send_envelope` has already been and
        // waited. So this is not "did the queue drain" but "did the one send
        // work", which is the question the dialog is actually asking.
        *self.outcome.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Builds [`Ours`] when the client asks for a transport.
///
/// It holds the pieces the transport cannot find for itself: the launcher's
/// HTTP client, and a handle to the runtime it has to be driven on.
struct OursFactory {
    http: Http,
    runtime: tokio::runtime::Handle,
}

impl sentry::TransportFactory for OursFactory {
    fn create_transport_with_options(
        &self,
        options: sentry::TransportOptions,
    ) -> Arc<dyn sentry::Transport> {
        Arc::new(Ours {
            http: self.http.clone(),
            runtime: self.runtime.clone(),
            url: options.dsn.envelope_api_url().to_string(),
            auth: options.dsn.to_auth(Some(&options.user_agent)).to_string(),
            outcome: std::sync::Mutex::new(false),
        })
    }
}

/// The client, built the first time something is sent.
///
/// Nothing exists until then: no client, no transport thread, no connection to
/// our server. A launcher that never reports a bug never talks to us at all,
/// which is the consent model rather than a promise about it.
// `ClientOptions` cannot be built by a struct literal (see below), so the
// fields are assigned after the default; that is the only way available.
#[allow(clippy::field_reassign_with_default)]
fn client(http: Option<Http>, runtime: tokio::runtime::Handle) -> Option<Arc<sentry::Client>> {
    CLIENT
        .get_or_init(|| {
            // The launcher's client when there is one. It is the same stack
            // with the same roots either way, this only shares the pool.
            let http = http.or_else(|| Http::new().ok())?;
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

            // Ours, set before `apply_defaults`, which only fills a transport
            // in when there is none. With it, sentry's `reqwest`, `rustls`
            // and `transport` features are all off in `Cargo.toml` and the
            // release carries one HTTP client and one crypto provider.
            options.transport = Some(Arc::new(OursFactory { http, runtime }));

            // `apply_defaults` still, for the `contexts` integration, and
            // because without it a client reports `is_enabled()` as false and
            // `flush` cheerfully returns true on an empty queue, which looks
            // exactly like a working client that cannot reach the network.
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
    /// Facts worth filtering issues by, product, Proton build, distro. Every
    /// one of them is already in the bundle the card showed; they are repeated
    /// here only so GlitchTip can group and search on them.
    pub tags: Vec<(String, String)>,
    pub shot: Option<Shot>,
    /// The last things the launcher said, oldest first.
    pub history: Vec<String>,
}

/// What the event ends up saying about the screenshot.
enum Picture {
    /// It was uploaded and this is where it lives.
    Link(String),
    /// It could not be, so it rides inside the event as a data URI, a wall
    /// of base64 on the issue page, and still better than losing it.
    Embedded(String),
}

/// Upload the screenshot, or say why the report will have to carry it.
///
/// Every failure lands in the same place deliberately: an unreachable host, a
/// refusal, a rate limit and a reply we cannot read are all "the picture goes
/// inside the event", and none of them is worth putting in front of someone
/// who is already reporting a bug.
async fn upload(shot: &Shot, http: Option<&Http>) -> Picture {
    use base64::Engine as _;

    let embed = || {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&shot.bytes);
        Picture::Embedded(format!("data:{};base64,{encoded}", shot.media_type))
    };

    let Some(http) = http else { return embed() };
    let uploaded: Result<Uploaded, _> =
        http.post_bytes(SHOTS, shot.media_type, shot.bytes.clone(), UPLOAD).await;
    match uploaded {
        Ok(Uploaded { url }) if url.starts_with("https://") => Picture::Link(url),
        _ => embed(),
    }
}

/// What the upload endpoint answers with.
#[derive(serde::Deserialize)]
struct Uploaded {
    url: String,
}

/// Send one report, and say whether it arrived.
///
/// `http` is the launcher's own client, so the upload shares the connection
/// pool and the TLS roots with everything else; without one the picture is
/// embedded as before.
pub async fn send(report: Report, http: Option<Http>) -> bool {
    // Taken here, where there is certainly a runtime, and handed down: the
    // transport is driven from a blocking thread and has to be told which
    // runtime to drive it on.
    let runtime = tokio::runtime::Handle::current();

    // Before the blocking half, because it is the one part that is actually
    // asynchronous, and because what it answers decides what the event says.
    let picture = match &report.shot {
        Some(shot) => Some((shot.name.clone(), upload(shot, http.as_ref()).await)),
        None => None,
    };

    // `flush` parks until the transport thread has drained, so it belongs on
    // the blocking pool and not on a runtime worker.
    tokio::task::spawn_blocking(move || {
        // One at a time; see `SENDING`.
        let _sending = SENDING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(client) = client(http, runtime) else { return false };
        let Report { kind, note, contact, tags, shot: _, history } = report;

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

        // Inside the event either way, never beside it: GlitchTip's ingest
        // lists `attachment` among the envelope item types it ignores, so an
        // attached image is parsed and dropped without an error anywhere.
        // A link is one click from the issue page; a data URI pastes into a
        // browser's address bar.
        if let Some((name, picture)) = picture {
            let (key, value) = match picture {
                Picture::Link(url) => ("screenshot_url", url),
                Picture::Embedded(uri) => ("screenshot", uri),
            };
            event.extra.insert(key.to_owned(), value.into());
            event.extra.insert("screenshot_name".to_owned(), name.into());
        }

        let contact = contact.trim();
        if !contact.is_empty() {
            // An address goes in `email` and anything else in `username`, so
            // GlitchTip shows it in the place a reply would be looked for.
            // The test is deliberately crude, it only decides which of two
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A server that answers one request and hands back what it was sent.
    ///
    /// The transport is the one piece of the report path with nobody above it
    /// to notice a mistake: a report that silently fails to leave the machine
    /// looks exactly like a report that arrived. So it is spoken to over a
    /// real socket rather than mocked.
    fn one_request() -> (String, std::sync::mpsc::Receiver<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut reader = BufReader::new(stream);
            let mut headers = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                let done = line == "\r\n" || line == "\n";
                headers.push_str(&line);
                if done {
                    break;
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let mut stream = reader.into_inner();
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            );
            let _ = stream.flush();
            let _ = tx.send((headers, body));
        });

        (format!("http://abc123@127.0.0.1:{port}/1"), rx)
    }

    fn transport(dsn: &str, runtime: &tokio::runtime::Runtime) -> Ours {
        let dsn: sentry::types::Dsn = dsn.parse().expect("dsn");
        Ours {
            http: Http::new().expect("http"),
            runtime: runtime.handle().clone(),
            url: dsn.envelope_api_url().to_string(),
            auth: dsn.to_auth(Some("lightforge-test")).to_string(),
            outcome: std::sync::Mutex::new(false),
        }
    }

    /// Multi-threaded, like the one the launcher actually runs under: a
    /// current-thread runtime is driven only while something calls into it,
    /// and `Ours` hands it work from a *different* thread and waits, which
    /// is a deadlock, not a test.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime")
    }

    #[test]
    fn an_event_reaches_the_server_with_its_auth_header() {
        use sentry::Transport as _;

        let (dsn, received) = one_request();
        let runtime = runtime();
        let transport = transport(&dsn, &runtime);

        let event = sentry::protocol::Event {
            message: Some("a report".to_owned()),
            ..Default::default()
        };
        transport.send_envelope(sentry::Envelope::from(event));

        // The dialog says "sent", so this has to mean it.
        assert!(transport.flush(FLUSH), "the send reported failure");

        let (headers, body) = received.recv_timeout(FLUSH).expect("nothing arrived");
        assert!(headers.starts_with("POST /api/1/envelope/"), "{headers}");
        let lower = headers.to_ascii_lowercase();
        assert!(lower.contains("x-sentry-auth: sentry sentry_key=abc123"), "no key in {headers}");
        assert!(lower.contains("sentry_version=7"), "no protocol version in {headers}");
        assert!(
            lower.contains("content-type: application/x-sentry-envelope"),
            "not sent as an envelope: {headers}"
        );
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("\"type\":\"event\""), "not an event envelope: {body}");
        assert!(body.contains("a report"), "the message did not travel: {body}");
    }

    #[test]
    fn a_server_that_is_not_there_is_reported_as_a_failure() {
        use sentry::Transport as _;

        let runtime = runtime();
        // Port 9 is discard; nothing accepts there.
        let transport = transport("http://abc123@127.0.0.1:9/1", &runtime);
        transport.send_envelope(sentry::Envelope::from(sentry::protocol::Event::default()));
        assert!(!transport.flush(FLUSH), "a failed send reported success");
    }
}
