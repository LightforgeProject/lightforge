//! Updating against the real release files on GitHub.
//!
//! **Ignored by default.** It downloads a published release (about 10 MB)
//! and reads a PKGBUILD from `main`. Run it deliberately, after a release:
//!
//! ```text
//! cargo test -p lightforge-core --test upgrade_live -- --ignored --nocapture
//! ```
//!
//! The unit tests prove the routes against files they wrote. This proves the
//! names they look for are the names a release really publishes, and that
//! its tarball installs. Everything lands in a temporary folder:
//! `XDG_DATA_HOME` points there too, so the menu entry does not touch the
//! real one.

use lightforge_core::net::Http;
use lightforge_core::upgrade::{self, tarball};

/// The release these run against: the one this build calls itself.
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::test]
#[ignore = "downloads a release from github.com"]
async fn the_published_release_installs_and_every_package_is_found() {
    let http = Http::new().expect("an http client");
    let dir = std::env::temp_dir().join(format!("lightforge-upgrade-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("home/.local/bin");
    let data = dir.join("home/.local/share");
    std::env::set_var("XDG_DATA_HOME", &data);

    tarball::install(&http, VERSION, &bin, &dir.join("scratch"), |_, _| {}).await.expect("the tarball installs");
    let cli = std::process::Command::new(bin.join("lightforge")).arg("--version").output().unwrap();
    let said = String::from_utf8_lossy(&cli.stdout);
    println!("installed: {said}");
    assert!(said.contains(VERSION), "{said}");
    let entry = std::fs::read_to_string(data.join("applications/pro.tuxcore.Lightforge.desktop")).unwrap();
    assert!(entry.contains(&bin.join("lightforge-gui").display().to_string()), "{entry}");

    for suffix in ["_amd64.deb", ".x86_64.rpm"] {
        let path = upgrade::fetch_asset(&http, VERSION, suffix, &dir.join("packages"), |_, _| {})
            .await
            .unwrap_or_else(|e| panic!("{suffix}: {e}"));
        println!("checked: {}", path.display());
    }
    for package in ["lightforge-bin", "lightforge"] {
        match upgrade::fetch_pkgbuild(&http, package, VERSION).await {
            Ok(_) => println!("{package}: PKGBUILD on main is {VERSION}, with checksums"),
            Err(e) => panic!("{package}: {e}"),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
