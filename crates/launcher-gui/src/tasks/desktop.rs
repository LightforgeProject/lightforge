//! Asking the desktop session for things: no sleep while the store is being
//! written, and a word when a download ends out of sight.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use ashpd::zbus::{self, zvariant};

/// A folder, through the desktop portal; see `update/report.rs` for why not `rfd`.
pub async fn pick_folder() -> Option<std::path::PathBuf> {
    let request = ashpd::desktop::file_chooser::OpenFileRequest::default()
        .title("Choose a folder for the Wine prefix")
        .modal(true)
        .multiple(false)
        .directory(true);
    let files = request.send().await.ok()?.response().ok()?;
    files.uris().first().and_then(|uri| uri.to_file_path().ok())
}

/// Hold off suspend, an idle one included, until the descriptor is dropped.
/// logind's lock rather than the portal's: it also holds in Steam's Game Mode,
/// where no portal backend implements Inhibit.
pub async fn keep_awake(why: &'static str) -> Result<Arc<OwnedFd>, String> {
    let system = zbus::Connection::system().await.map_err(|e| format!("system bus: {e}"))?;
    let reply = system
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Inhibit",
            &("sleep", "Lightforge", why, "block"),
        )
        .await
        .map_err(|e| format!("logind refused the sleep lock: {e}"))?;
    let lock: zvariant::OwnedFd = reply.body().deserialize().map_err(|e| format!("logind's reply: {e}"))?;
    Ok(Arc::new(lock.into()))
}

/// Show a desktop notification. Best effort: a session without a
/// notification server (gamescope) just goes without.
pub async fn notify(summary: String, body: String) {
    let sent = async {
        let session = zbus::Connection::session().await?;
        // Lets the server use the app's own name and icon.
        let hints = HashMap::from([("desktop-entry", zvariant::Value::from(crate::APP_ID))]);
        session
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &("Lightforge", 0u32, crate::APP_ID, summary.as_str(), body.as_str(), Vec::<&str>::new(), hints, -1i32),
            )
            .await
            .map(drop)
    };
    if let Err(e) = sent.await {
        tracing::warn!("no desktop notification for \"{summary}\": {e}");
    }
}
