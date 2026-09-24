//! End-to-end acceptance: a real download, and every artifact that has to come
//! out of it.
//!
//! **Ignored by default.** These hit Blizzard's CDN and write gigabytes, so
//! they are not part of `cargo test`. Run them deliberately:
//!
//! ```text
//! cargo test -p lightforge-core --test acceptance -- --ignored --nocapture
//! cargo test -p lightforge-core --test acceptance -- --ignored --nocapture bounded
//! ```
//!
//! **Why an integration test rather than more unit tests.** Every bug worth
//! catching lately lived in a seam, not in a function: the store writer was
//! fine and the registry splice was fine, but the splice silently did nothing
//! against a registry shaped like a real one. Unit tests are written against
//! the data the author imagined. This drives the same path the launcher does.

use std::path::{Path, PathBuf};

use lightforge_core::net::Http;
use lightforge_core::product::ProductCode;
use lightforge_core::tact::download::Cancel;
use lightforge_core::tact::install::{self, Options};
use lightforge_core::tact::storage::Storage;
use lightforge_core::tact::{productdb, uninstall};
use lightforge_core::tact::version::Region;
use lightforge_core::tact::writer::StoreWriter;

/// Classic Era: the smallest complete WoW product, and the one the manual
/// acceptance run has always used.
const PRODUCT: &str = "wow_classic_era";

/// A scratch root that is removed first, so a failed run leaves evidence and
/// the next run still starts clean.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightforge-acceptance-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Agent's registry, seeded with records Lightforge does not own.
///
/// The point is not that it looks real — it is that these records must come
/// back out untouched. Registering into a registry we cannot read, or one
/// whose other products we quietly drop, is the failure mode that would cost
/// someone their library.
fn seed_agent_registry(root: &Path) -> PathBuf {
    let agent = root
        .join("prefix/pfx/drive_c/ProgramData/Battle.net/Agent");
    std::fs::create_dir_all(&agent).expect("an agent directory");

    let foreign = |product: &str, path: &str, version: &str| productdb::ProductInstall {
        product: product.into(),
        install_path: path.into(),
        region: "eu".into(),
        locale: "enUS".into(),
        account_country: "AUT".into(),
        geo_country: "AT".into(),
        flavor_dir: String::new(),
        version: version.into(),
        build_key: "f".repeat(32),
        tags: String::new(),
        key_ring: None,
        family: String::new(),
        installed_at_ms: 0,
    };

    let bytes = productdb::encode(&[
        foreign("agent", "C:/ProgramData/Battle.net/Agent", "2.41.0.9775"),
        foreign("battle.net", "C:/Program Files (x86)/Battle.net", "2.53.0.17840"),
        foreign("hs_beta", "C:/Program Files (x86)/Hearthstone", "36.6.0.253216"),
    ]);
    let path = agent.join("product.db");
    std::fs::write(&path, bytes).expect("a seeded registry");
    path
}

fn http() -> Http {
    Http::new().expect("an http client")
}

/// A download that is cut short, which is the common case: a launcher gets
/// closed, a network drops, someone hits pause.
///
/// What must hold afterwards is that the store is still a store — readable,
/// and re-indexable without touching an archive. An interrupted run that
/// leaves an unreadable store is the `NeedsRepair` this project already hit
/// once.
#[tokio::test]
#[ignore = "downloads from Blizzard's CDN"]
async fn bounded_run_leaves_a_readable_store() {
    let root = scratch("bounded");
    let code: ProductCode = PRODUCT.parse().unwrap();

    let options = Options {
        region: Region::Eu,
        flavor: code.install_dir().into_owned(),
        limit_archives: Some(40),
        limit_runs: Some(2_000),
        install_files: false,
        ..Options::default()
    };

    let summary = install::run(&http(), &root, &code, &options, &Cancel::new(), |_| {})
        .await
        .expect("the run itself must not fail");

    // Deliberately incomplete, and it must say so rather than claim an
    // install: `.build.info` is the completion marker and gates `Play`.
    assert!(!summary.complete, "a truncated plan must not report a complete install");
    assert!(summary.version.is_none());
    assert!(!root.join(".build.info").exists(), ".build.info was written for a partial run");

    let store = Storage::open(&root).expect("the store must be readable after a partial run");
    let stats = store.stats();
    assert!(stats.files > 0, "nothing was stored");
    assert_eq!(stats.buckets, 16, "a CASC store has sixteen index buckets");

    // Re-indexing is the repair path. It must round-trip without losing a file.
    drop(store);
    let data_dir = root.join("Data").join("data");
    StoreWriter::create(&data_dir).unwrap().finish().expect("indices must rewrite");

    let after = Storage::open(&root).expect("the store must survive a reindex").stats();
    assert_eq!(after.files, stats.files, "reindex changed the file count");
    assert_eq!(after.buckets, stats.buckets);
}

/// The whole chain, on a complete install: store, `.build.info`,
/// `.product.db`, Agent's registry, and then removing it again.
///
/// Slow — Classic Era is a few GiB — and the only test that proves the thing
/// the user actually cares about, which is that Battle.net says `Play`.
#[tokio::test]
#[ignore = "downloads all of Classic Era from Blizzard's CDN"]
async fn a_complete_install_produces_every_artifact() {
    let root = scratch("complete");
    let code: ProductCode = PRODUCT.parse().unwrap();
    let registry = seed_agent_registry(&root);
    let before = std::fs::read(&registry).unwrap();
    let foreign_records = productdb::decode(&before);

    let options = Options {
        region: Region::Eu,
        flavor: code.install_dir().into_owned(),
        install_files: true,
        agent_dir: registry.parent().map(Path::to_path_buf),
        ..Options::default()
    };

    let summary = install::run(&http(), &root, &code, &options, &Cancel::new(), |_| {})
        .await
        .expect("the install must not fail");

    assert!(summary.complete, "the install did not finish: {summary:?}");
    let version = summary.version.clone().expect("a complete install names its version");

    // 1. The store.
    let stats = Storage::open(&root).expect("a readable store").stats();
    assert!(stats.files > 0);

    // 2. `.build.info` — the completion marker, and what the resolver reads
    //    to decide between Install, Update and Play.
    let info = lightforge_core::buildinfo::BuildInfo::load(root.join(".build.info"))
        .expect("a complete install writes .build.info");
    assert_eq!(
        info.get(&code, lightforge_core::buildinfo::COL_VERSION).unwrap(),
        version
    );

    // 3. The per-install `.product.db`, without which Battle.net's Locate
    //    flow answers error 2800.
    let ours = productdb::read(&root.join(".product.db")).expect("a per-install registry");
    assert!(ours.iter().any(|i| i.product == PRODUCT), "our product is not in .product.db");

    // 4. Agent's registry: our product added, everything else untouched.
    //    This is what decides whether the tile says Play or Install.
    let after = productdb::read(&registry).unwrap();
    let ours_registered = after
        .iter()
        .find(|i| i.product == PRODUCT)
        .expect("the product was not registered with Agent");
    assert_eq!(ours_registered.version.as_deref(), Some(version.as_str()));

    for foreign in &foreign_records {
        let kept = after
            .iter()
            .find(|i| i.product == foreign.product)
            .unwrap_or_else(|| panic!("{} was lost from the registry", foreign.product));
        assert_eq!(kept, foreign, "{} was rewritten", foreign.product);
    }

    // 5. The game binary, which is what `Local::is_installed` looks for.
    let exe = lightforge_core::paths::game_executable(&root.join(code.install_dir().as_ref()), &code);
    assert!(exe.is_file(), "no game executable at {}", exe.display());

    // 6. Removing it again leaves nothing behind.
    let plan = uninstall::plan(&root, &code).expect("an uninstall plan");
    assert!(!plan.is_empty(), "nothing to remove after a complete install");
    uninstall::execute(&root, &code, &plan).expect("uninstall must succeed");
    assert!(!exe.exists(), "the flavor directory survived uninstall");
}
