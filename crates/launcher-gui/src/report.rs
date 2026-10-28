//! Sending a bug report, or a note to the developer.
//!
//! A Sentry event to our GlitchTip, only on Send. No automatic capture: the
//! `panic` and `release-health` features are off in `Cargo.toml`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use lightforge_core::net::Http;

/// Our GlitchTip project; a DSN is a write-only key meant to ship.
const DSN: &str = "https://788f3511837f42abade303476cdeef11@bugs.k3nny.tech/1";

/// How long to wait for the event to leave: the dialog says it was sent.
const FLUSH: Duration = Duration::from_secs(15);

/// Where a screenshot is uploaded, so the issue carries a link (GlitchTip
/// drops attachments). `/upload`: the proxy routes `/shots/v1/*` only.
const SHOTS: &str = "https://lightforge.tuxcore.pro/shots/v1/upload";

/// How long the upload may take before the report goes without it.
///
/// Short: on failure the picture travels inside the event.
const UPLOAD: Duration = Duration::from_secs(20);

/// What is being sent.
pub enum Kind {
    /// A bug, with the debug bundle, already scrubbed by
    /// [`lightforge_core::debug::DebugInfo::render`].
    Bug(String),
    /// A note about the app. No diagnostics: nothing is wrong with the
    /// machine, so none of its details are anyone's business.
    Feedback,
}

/// What a report is about, picked in the dialog before it can be sent.
///
/// It tags the report and prefixes its title, and decides what a bug report
/// attaches beside the bundle (the addon list, the settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// The game itself: it will not start, crashes, looks or runs wrong.
    Game,
    Addons,
    /// Installing, updating, repairing, the download queue and the drives.
    Downloader,
    Settings,
    /// Battle.net and the games it starts for us.
    BattleNet,
    /// Lightforge's own window: layout, controls, the controller.
    Launcher,
    General,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Self::Game,
        Self::Addons,
        Self::Downloader,
        Self::Settings,
        Self::BattleNet,
        Self::Launcher,
        Self::General,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Addons => "Addons",
            Self::Downloader => "Downloader",
            Self::Settings => "Settings",
            Self::BattleNet => "Battle.net",
            Self::Launcher => "Launcher",
            Self::General => "General",
        }
    }

    /// The tag value, stable and lower-case so a GlitchTip search for
    /// `category:addons` keeps working whatever the label becomes.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Addons => "addons",
            Self::Downloader => "downloader",
            Self::Settings => "settings",
            Self::BattleNet => "battlenet",
            Self::Launcher => "launcher",
            Self::General => "general",
        }
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

static CLIENT: OnceLock<Option<Arc<sentry::Client>>> = OnceLock::new();

/// Feedback goes out without the `contexts` integration's machine details.
fn without_machine_details(mut event: sentry::protocol::Event<'static>) -> Option<sentry::protocol::Event<'static>> {
    if event.tags.get("type").map(String::as_str) == Some("feedback") {
        event.contexts.clear();
    }
    Some(event)
}

/// One report at a time.
///
/// [`Ours`] records one send's outcome for `flush`, exact only one at a time.
static SENDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The launcher's own HTTP client, wearing sentry's transport trait.
///
/// sentry's own transport brings a second `reqwest` and `aws-lc-sys`.
/// Synchronous: the caller is on a blocking thread ([`send`]). No retries.
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
        // Nothing is queued: this is whether the one send worked.
        *self.outcome.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Builds [`Ours`] when the client asks for a transport.
///
/// With the launcher's HTTP client and the runtime to drive it on.
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
/// Until then nothing exists: no client, no thread, no connection.
// `ClientOptions` cannot be built by a struct literal (see below), so the
// fields are assigned after the default; that is the only way available.
#[allow(clippy::field_reassign_with_default)]
fn client(http: Option<Http>, runtime: tokio::runtime::Handle) -> Option<Arc<sentry::Client>> {
    CLIENT
        .get_or_init(|| {
            // The launcher's client when there is one. It is the same stack
            // with the same roots either way, this only shares the pool.
            let http = http.or_else(|| Http::new().ok())?;
            // `ClientOptions` is `#[non_exhaustive]`.
            let mut options = sentry::ClientOptions::default();
            options.release = sentry::release_name!();
            // The default, set anyway because it is load-bearing: it is what
            // stops the server recording the sender's IP address.
            options.send_default_pii = false;
            // Or `contexts` fills in the hostname, often a person's name,
            // ignoring `send_default_pii`.
            options.server_name = Some("redacted".into());
            // After the integrations, so it sees what they added.
            options.before_send = Some(Arc::new(without_machine_details));

            // Parsed: `from_config` panics on a malformed DSN.
            let Ok(dsn) = DSN.parse::<sentry::types::Dsn>() else { return None };
            options.dsn = Some(dsn);

            // Before `apply_defaults`, which only fills in a missing transport.
            options.transport = Some(Arc::new(OursFactory { http, runtime }));

            // Without `apply_defaults` the client is disabled and `flush`
            // returns true on an empty queue.
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
    pub category: Category,
    pub note: String,
    /// An address, a Discord name, or nothing. The one piece of personal data
    /// in either report, and it is here because they typed it.
    pub contact: String,
    /// Facts to filter issues by (product, Proton, distro), also in the bundle.
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
/// Every failure means the picture goes inside the event, silently.
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
/// `http` is the launcher's own client; without it the picture is embedded.
pub async fn send(report: Report, http: Option<Http>) -> bool {
    // Taken here, for the transport on its blocking thread.
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
        let Report { kind, category, note, contact, tags, shot: _, history } = report;

        // The category leads the message because the message is the issue's
        // title, and the title is all the issue list shows.
        let message = format!("[{}] {}", category.label(), note.trim());
        let mut event = sentry::protocol::Event { message: Some(message), ..Default::default() };
        event.tags.insert("category".to_owned(), category.tag().to_owned());

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

        // Inside the event: GlitchTip drops attachments.
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
            // An address in `email`, anything else in `username`.
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
    /// Over a real socket, not mocked: a silent failure looks like success.
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

    /// Multi-threaded: `Ours` waits on work from another thread.
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
    fn feedback_leaves_the_machine_details_behind() {
        let context = || sentry::protocol::Context::Os(Box::default());
        let mut feedback = sentry::protocol::Event::default();
        feedback.tags.insert("type".to_owned(), "feedback".to_owned());
        feedback.contexts.insert("os".to_owned(), context());
        assert!(without_machine_details(feedback).unwrap().contexts.is_empty());

        // A bug report keeps them: its card shows the same facts.
        let mut bug = sentry::protocol::Event::default();
        bug.contexts.insert("os".to_owned(), context());
        assert_eq!(without_machine_details(bug).unwrap().contexts.len(), 1);
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
