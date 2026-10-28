//! A game moved between filesystems, from a real install.
//!
//! **Ignored by default**: it copies a whole game. The source is only read
//! (and locked against writers while it is); nothing is released from it.
//! Symlinks in its game folder, a linked `WTF` say, must arrive as links.
//!
//! ```text
//! LIGHTFORGE_MOVE_FROM=/mnt/games/Battle.net/games \
//! LIGHTFORGE_MOVE_TO=$HOME/lightforge-move-test/games \
//! LIGHTFORGE_MOVE_PRODUCT=wowf \
//!   cargo test -p lightforge-core --test migrate_live -- --ignored --nocapture
//! ```

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use lightforge_core::buildinfo::BuildInfo;
use lightforge_core::library::{Libraries, Library};
use lightforge_core::migrate::{self, Moved};
use lightforge_core::net::Http;
use lightforge_core::paths::Layout;
use lightforge_core::product::ProductCode;
use lightforge_core::tact::download::Cancel;
use lightforge_core::tact::verify;

fn var(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name}")))
}

fn device(path: &Path) -> u64 {
    let existing = path.ancestors().find(|p| p.exists()).expect("an existing ancestor");
    std::fs::metadata(existing).expect("a readable ancestor").dev()
}

#[tokio::test]
#[ignore = "copies a whole game between two real filesystems"]
async fn a_game_copied_to_another_filesystem_reads_back_whole() {
    let (from_games, to_games) = (var("LIGHTFORGE_MOVE_FROM"), var("LIGHTFORGE_MOVE_TO"));
    let code: ProductCode = std::env::var("LIGHTFORGE_MOVE_PRODUCT")
        .unwrap_or_else(|_| "wow_classic_era".to_owned())
        .parse()
        .unwrap();
    // On one filesystem the copy would be a rename of the real install.
    assert_ne!(device(&from_games), device(&to_games), "the two must be on different filesystems");

    let from = Library::main_at(from_games, "source");
    let to = Library::adopted(to_games, "target");
    let scratch = tempfile::tempdir().unwrap();
    let layout = Layout::with_root(scratch.path()).with_libraries(Libraries::new(vec![from.clone(), to.clone()]));

    let started = std::time::Instant::now();
    let mut last = 0;
    let moved = migrate::copy(&Http::new().unwrap(), &layout, &code, &to, &Cancel::new(), |p| {
        let tenth = p.bytes_done * 10 / p.bytes_total.max(1);
        if tenth != last {
            last = tenth;
            eprintln!("{} {}/{} B", if p.verifying { "verified" } else { "copied" }, p.bytes_done, p.bytes_total);
        }
    })
    .await
    .unwrap();
    eprintln!("{moved:?} in {:.0}s", started.elapsed().as_secs_f64());
    assert!(matches!(moved, Moved::Copied { .. }));

    let dest = layout.wow_install_in(&to);
    assert!(BuildInfo::load(dest.join(".build.info")).unwrap().row(&code).is_some());
    let report = verify::deep_scan(&dest, &Cancel::new(), |_| {}).unwrap();
    assert!(report.damaged.is_empty(), "{} damaged after the copy", report.damaged.len());
    assert!(report.missing_archives.is_empty());
    assert!(
        BuildInfo::load(layout.wow_install_in(&from).join(".build.info")).unwrap().row(&code).is_some(),
        "the source keeps the game until it is released"
    );

    let flavor = layout.flavor(&code);
    let links = symlinks(&layout.wow_install_in(&from).join(&*flavor));
    eprintln!("{} symlinks in the flavour folder", links.len());
    for (relative, target) in links {
        let copied = dest.join(&*flavor).join(&relative);
        assert_eq!(std::fs::read_link(&copied).ok(), Some(target), "{} is still the same link", relative.display());
    }
}

/// Every symlink under `dir`, relative to it, with its target text.
fn symlinks(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).expect("a readable game folder").flatten() {
            let kind = entry.file_type().expect("a readable entry");
            if kind.is_symlink() {
                let relative = entry.path().strip_prefix(dir).expect("found under dir").to_path_buf();
                found.push((relative, std::fs::read_link(entry.path()).expect("a readable link")));
            } else if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    found
}
