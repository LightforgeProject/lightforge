use super::*;

/// A `.tar.gz` holding `files`, written into `dir`.
fn tarball(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join("release.tar.gz");
    let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), Default::default());
    let mut tar = tar::Builder::new(gz);
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    path
}

fn staging_left(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().starts_with(crate::staging::PREFIX))
}

const BUILD: &[(&str, &[u8])] = &[
    ("GE-Proton11-7/proton", b"#!/usr/bin/env python3"),
    ("GE-Proton11-7/version", b"1760000000 GE-Proton11-7"),
    ("GE-Proton11-7/files/bin/wine", b"elf"),
];

#[test]
fn an_unpacked_build_lands_whole_and_leaves_no_staging() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proton");
    let built = unpack(&tarball(tmp.path(), BUILD), &root).unwrap();

    assert_eq!(built, root.join("GE-Proton11-7"));
    assert!(built.join("files/bin/wine").is_file());
    assert!(!staging_left(&root));
}

#[test]
fn a_truncated_download_leaves_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proton");
    let archive = tarball(tmp.path(), BUILD);
    let bytes = std::fs::read(&archive).unwrap();
    std::fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();

    assert!(unpack(&archive, &root).is_err());
    assert!(!root.join("GE-Proton11-7").exists());
    assert!(!staging_left(&root));
    assert!(discover_managed(&root).is_empty());
}

#[test]
fn a_tarball_that_is_not_a_proton_build_is_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proton");
    let archive = tarball(tmp.path(), &[("Something/readme", b"hi")]);

    assert!(matches!(unpack(&archive, &root), Err(ProtonError::NotAProtonBuild(_))));
    assert!(!root.join("Something").exists());
    assert!(!staging_left(&root));
}

#[test]
fn unpacking_a_release_again_replaces_it_rather_than_merging() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proton");
    let stale = root.join("GE-Proton11-7/files/stale.dll");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, b"old").unwrap();

    let built = unpack(&tarball(tmp.path(), BUILD), &root).unwrap();
    assert!(built.join("proton").is_file());
    assert!(!stale.exists());
    assert!(!staging_left(&root));
}

#[test]
fn classifies_real_world_tags() {
    // Tags observed on a live system.
    assert_eq!(
        ProtonFlavor::classify("GE-Proton11-6"),
        ProtonFlavor::GloriousEggroll
    );
    assert_eq!(
        ProtonFlavor::classify("GE-Proton10-34"),
        ProtonFlavor::GloriousEggroll
    );
    assert_eq!(
        ProtonFlavor::classify("UMU-Proton-10.0-4"),
        ProtonFlavor::Umu
    );
    assert_eq!(
        ProtonFlavor::classify("proton-11.0-2-x86_64"),
        ProtonFlavor::Valve
    );
    assert_eq!(
        ProtonFlavor::classify("experimental-11.0-20260910b-x86_64"),
        ProtonFlavor::Valve
    );
}

#[test]
fn only_ge_is_a_launch_target() {
    assert!(ProtonFlavor::GloriousEggroll.is_supported());
    assert!(!ProtonFlavor::Umu.is_supported());
    assert!(ProtonFlavor::Valve.is_supported());
    assert!(!ProtonFlavor::Valve.is_default_candidate());
    assert!(!ProtonFlavor::CachyOs.is_default_candidate());
    assert!(!ProtonFlavor::Other.is_supported());
}

/// A real build directory under `root`, complete enough for discovery.
fn installed(root: &Path, version: &str) -> ProtonRelease {
    let dir = root.join(format!("{version}-x86_64"));
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(dir.join("proton"), b"#!/usr/bin/env python3\n").unwrap();
    std::fs::write(dir.join("version"), format!("1787951532 {version}\n")).unwrap();
    std::fs::write(dir.join("files/blob"), vec![0u8; 4096]).unwrap();
    ProtonRelease::read_from_dir(&dir, ProtonSource::Managed).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightforge-proton-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_unused_managed_build_can_be_removed() {
    let root = scratch("remove-ok");
    let build = installed(&root, "GE-Proton11-7");
    assert!(build.path.is_dir());

    remove_build(&build, &root, &[]).unwrap();
    assert!(!build.path.exists());
}

#[test]
fn a_build_something_is_pinned_to_is_refused_by_name() {
    let root = scratch("remove-pinned");
    let build = installed(&root, "GE-Proton11-7");

    let err = remove_build(&build, &root, &["the app default".into(), "WoW PTR".into()])
        .unwrap_err();
    // The message has to say what to unpin, or "no" is unactionable.
    let message = err.to_string();
    assert!(message.contains("the app default"), "{message}");
    assert!(message.contains("WoW PTR"), "{message}");
    assert!(build.path.is_dir(), "a refused removal must delete nothing");
}

#[test]
fn a_build_we_did_not_install_is_never_deleted() {
    let root = scratch("remove-theirs");
    let elsewhere = scratch("remove-steam");
    let build = installed(&elsewhere, "GE-Proton11-7");

    // Right shape, wrong directory: this is Steam's, or hand-placed.
    let err = remove_build(&build, &root, &[]).unwrap_err();
    assert!(matches!(err, ProtonError::NotOurs(_)));
    assert!(build.path.is_dir());

    // And the same build claiming a non-managed source, even in our dir.
    let mut theirs = installed(&root, "GE-Proton11-6");
    theirs.source = ProtonSource::SystemCompatTools;
    assert!(matches!(
        remove_build(&theirs, &root, &[]).unwrap_err(),
        ProtonError::NotOurs(_)
    ));
    assert!(theirs.path.is_dir());
}

#[test]
fn size_counts_the_whole_tree() {
    let root = scratch("size");
    let build = installed(&root, "GE-Proton11-7");
    // The 4 KiB blob plus the two small text files.
    assert!(size_on_disk(&build.path) > 4096);
    // A path that is not there is zero, not a panic.
    assert_eq!(size_on_disk(&root.join("nothing")), 0);
}

fn build(version: &str, built_at: Option<u64>, flavor: ProtonFlavor) -> ProtonRelease {
    ProtonRelease {
        version: version.to_owned(),
        flavor,
        source: ProtonSource::Managed,
        path: PathBuf::from("/tmp").join(version),
        built_at,
    }
}

#[test]
fn without_the_tested_build_the_newest_supported_one_stands_in() {
    let builds = vec![
        build(
            "GE-Proton10-25",
            Some(1_787_951_532),
            ProtonFlavor::GloriousEggroll,
        ),
        build(
            "GE-Proton11-10",
            Some(1_780_000_000),
            ProtonFlavor::GloriousEggroll,
        ),
        build(
            "proton-11.0-2-x86_64",
            Some(1_799_999_999),
            ProtonFlavor::Valve,
        ),
    ];
    assert_eq!(select(&builds, None).unwrap().version, "GE-Proton10-25");
}

#[test]
fn the_tested_build_beats_a_newer_one() {
    // The machine GE-Proton11-9 broke had both, and newest-wins took 11-9.
    let builds = vec![
        build(TESTED, Some(1_787_951_532), ProtonFlavor::GloriousEggroll),
        build("GE-Proton11-9", Some(1_791_936_695), ProtonFlavor::GloriousEggroll),
    ];
    assert_eq!(select(&builds, None).unwrap().version, TESTED);
    // A pin still means exactly what it says.
    assert_eq!(select(&builds, Some("GE-Proton11-9")).unwrap().version, "GE-Proton11-9");
}

#[test]
fn the_tested_build_is_fetched_only_as_a_step_forward() {
    let have = |tags: &[&str]| -> Vec<ProtonRelease> {
        tags.iter().map(|t| build(t, None, ProtonFlavor::GloriousEggroll)).collect()
    };
    let old = have(&["GE-Proton11-5"]);
    assert!(wants_tested(&old, None, Some("GE-Proton11-5")));
    assert!(wants_tested(&old, None, None), "no prefix yet");
    assert!(!wants_tested(&old, Some("GE-Proton11-5"), None), "a pin is a choice");
    assert!(!wants_tested(&have(&["GE-Proton11-5", TESTED]), None, None), "already there");
    assert!(!wants_tested(&[], None, None), "setup installs the first build");
    assert!(!wants_tested(&have(&["GE-Proton11-11"]), None, None), "a newer build is kept");
    assert!(!wants_tested(&old, None, Some("GE-Proton11-9")), "would downgrade the prefix");
    let valve = [build("proton-11.0-2-x86_64", None, ProtonFlavor::Valve)];
    assert!(!wants_tested(&valve, None, None), "nothing supported installed");
}

#[test]
fn a_pin_is_never_silently_substituted() {
    let builds = vec![build("GE-Proton11-7", None, ProtonFlavor::GloriousEggroll)];
    assert_eq!(
        select(&builds, Some("GE-Proton11-7")).unwrap().version,
        "GE-Proton11-7"
    );
    assert!(select(&builds, Some("GE-Proton9-20")).is_none());
}

#[test]
fn version_file_splits_timestamp_from_tag() {
    assert_eq!(
        parse_version_file("1787951532 GE-Proton11-6\n"),
        Some((Some(1787951532), "GE-Proton11-6".to_owned()))
    );
}

#[test]
fn version_file_tolerates_a_bare_tag() {
    assert_eq!(
        parse_version_file("GE-Proton9-20"),
        Some((None, "GE-Proton9-20".to_owned()))
    );
    assert_eq!(parse_version_file(""), None);
}

#[test]
fn ge_tags_order_numerically_not_lexically() {
    // The whole reason this parse exists: as strings, 11-7 sorts below 9-20.
    assert!(ge_version("GE-Proton11-7") > ge_version("GE-Proton9-20"));
    assert!(ge_version("GE-Proton11-10") > ge_version("GE-Proton11-7"));
    assert_eq!(ge_version("GE-Proton11-7-x86_64"), ge_version("GE-Proton11-7"));
    assert_eq!(ge_version("proton-11.0-2-x86_64"), None);
    assert_eq!(ge_version("Proton - Experimental"), None);
}

#[test]
fn a_prefix_reports_the_build_that_made_it() {
    let dir = scratch("prefix-build");
    assert_eq!(prefix_build(&dir), None);
    // What Proton actually writes there: the tag alone, no timestamp.
    std::fs::write(dir.join("version"), "GE-Proton11-7\n").unwrap();
    assert_eq!(prefix_build(&dir), Some("GE-Proton11-7".to_owned()));
}

#[test]
fn only_an_older_build_counts_as_a_downgrade() {
    assert_eq!(prefix_fit("GE-Proton11-7", None), PrefixFit::Fresh);
    assert_eq!(
        prefix_fit("GE-Proton11-7", Some("GE-Proton11-7")),
        PrefixFit::Same
    );
    assert_eq!(
        prefix_fit("GE-Proton11-8", Some("GE-Proton11-7")),
        PrefixFit::Newer
    );
    // Across a major, where a string comparison would get it backwards.
    assert_eq!(
        prefix_fit("GE-Proton11-7", Some("GE-Proton9-20")),
        PrefixFit::Newer
    );
    let fit = prefix_fit("GE-Proton10-34", Some("GE-Proton11-7"));
    assert_eq!(
        fit,
        PrefixFit::Older {
            built_by: "GE-Proton11-7".to_owned()
        }
    );
    assert!(fit.is_downgrade());
}

#[test]
fn an_unparseable_tag_is_never_warned_about() {
    // A prefix built by Valve's Proton, or by a hand-placed build: there
    // is no ordering between the two tags, so nothing is claimed.
    assert_eq!(
        prefix_fit("GE-Proton11-7", Some("proton-11.0-2")),
        PrefixFit::Unknown
    );
    assert!(!prefix_fit("GE-Proton11-7", Some("proton-11.0-2")).is_downgrade());
}

#[test]
fn discovery_lists_builds_in_numeric_order() {
    let dir = scratch("discovery-order");
    for version in ["GE-Proton9-20", "GE-Proton11-7", "GE-Proton10-34"] {
        installed(&dir, version);
    }
    let versions: Vec<String> =
        discover_all(None, &dir).into_iter().map(|b| b.version).collect();
    assert_eq!(versions, ["GE-Proton9-20", "GE-Proton10-34", "GE-Proton11-7"]);
}

fn release(tag: &str, prerelease: bool) -> UpstreamRelease {
    UpstreamRelease {
        tag_name: tag.to_owned(),
        prerelease,
        html_url: format!("https://example.invalid/{tag}"),
        assets: vec![
            ReleaseAsset {
                name: format!("{tag}.tar.gz"),
                browser_download_url: format!("https://example.invalid/{tag}.tar.gz"),
                size: 400_000_000,
            },
            ReleaseAsset {
                name: format!("{tag}.sha512sum"),
                browser_download_url: format!("https://example.invalid/{tag}.sha512sum"),
                size: 150,
            },
        ],
    }
}

#[test]
fn latest_stable_skips_prereleases() {
    // GitHub returns newest-first.
    let releases = vec![
        release("GE-Proton12-1", true),
        release("GE-Proton11-6", false),
    ];
    assert_eq!(latest_stable(&releases).unwrap().tag_name, "GE-Proton11-6");
}

#[test]
fn update_notice_only_when_behind() {
    let releases = vec![release("GE-Proton11-6", false)];
    assert!(check_for_update("GE-Proton11-6", &releases).is_none());

    let notice = check_for_update("GE-Proton10-34", &releases).unwrap();
    assert_eq!(notice.current, "GE-Proton10-34");
    assert_eq!(notice.available, "GE-Proton11-6");
}

#[test]
fn no_notice_when_upstream_has_only_prereleases() {
    let releases = vec![release("GE-Proton12-1", true)];
    assert!(check_for_update("GE-Proton11-6", &releases).is_none());
}

fn with_assets(tag: &str, names: &[&str]) -> UpstreamRelease {
    UpstreamRelease {
        tag_name: tag.to_owned(),
        prerelease: false,
        html_url: String::new(),
        assets: names
            .iter()
            .map(|n| ReleaseAsset {
                name: (*n).to_owned(),
                browser_download_url: format!("https://example.invalid/{n}"),
                size: 0,
            })
            .collect(),
    }
}

#[test]
fn dual_arch_release_does_not_hand_x86_the_arm_build() {
    // Real asset list for GE-Proton11-7; aarch64 sorts first.
    let r = with_assets(
        "GE-Proton11-7",
        &[
            "GE-Proton11-7-aarch64.sha512sum",
            "GE-Proton11-7-aarch64.tar.gz",
            "GE-Proton11-7-x86_64.sha512sum",
            "GE-Proton11-7-x86_64.tar.gz",
        ],
    );
    let tarball = r.tarball_for("x86_64").unwrap();
    assert_eq!(tarball.name, "GE-Proton11-7-x86_64.tar.gz");
    assert_eq!(
        r.checksum_for(tarball).unwrap().name,
        "GE-Proton11-7-x86_64.sha512sum"
    );
    assert_eq!(
        r.tarball_for("aarch64").unwrap().name,
        "GE-Proton11-7-aarch64.tar.gz"
    );
}

#[test]
fn mixed_era_treats_the_unsuffixed_asset_as_x86() {
    // GE-Proton11-3 shipped aarch64 alongside an unsuffixed x86_64 build.
    let r = with_assets(
        "GE-Proton11-3",
        &[
            "GE-Proton11-3-aarch64.sha512sum",
            "GE-Proton11-3-aarch64.tar.gz",
            "GE-Proton11-3.sha512sum",
            "GE-Proton11-3.tar.gz",
        ],
    );
    let tarball = r.tarball_for("x86_64").unwrap();
    assert_eq!(tarball.name, "GE-Proton11-3.tar.gz");
    assert_eq!(
        r.checksum_for(tarball).unwrap().name,
        "GE-Proton11-3.sha512sum"
    );
}

#[test]
fn cachyos_takes_the_plain_x86_64_xz_not_the_v3_build() {
    // Real asset list for cachyos-11.0-20261015-slr.
    let r = with_assets(
        "cachyos-11.0-20261015-slr",
        &[
            "proton-cachyos-11.0-20261015-slr-arm64.sha512sum",
            "proton-cachyos-11.0-20261015-slr-arm64.tar.xz",
            "proton-cachyos-11.0-20261015-slr-x86_64.sha512sum",
            "proton-cachyos-11.0-20261015-slr-x86_64.tar.xz",
            "proton-cachyos-11.0-20261015-slr-x86_64_v3.sha512sum",
            "proton-cachyos-11.0-20261015-slr-x86_64_v3.tar.xz",
        ],
    );
    let tarball = r.tarball_for("x86_64").unwrap();
    assert_eq!(tarball.name, "proton-cachyos-11.0-20261015-slr-x86_64.tar.xz");
    assert_eq!(
        r.checksum_for(tarball).unwrap().name,
        "proton-cachyos-11.0-20261015-slr-x86_64.sha512sum"
    );
    assert_eq!(
        r.tarball_for("aarch64").unwrap().name,
        "proton-cachyos-11.0-20261015-slr-arm64.tar.xz"
    );
}

#[test]
fn a_steam_tool_is_recognised_by_the_folder_steam_put_it_in() {
    let at = |dir: &str, source| ProtonRelease {
        version: "proton-9.0-4".to_owned(),
        flavor: ProtonFlavor::Valve,
        source,
        path: PathBuf::from("/lib/steamapps/common").join(dir),
        built_at: None,
    };
    let experimental = STEAM_TOOLS.iter().find(|t| t.app_id == 1493710).unwrap();
    assert!(experimental.is_installed_as(&at("Proton - Experimental", ProtonSource::SteamApp)));
    assert!(!experimental.is_installed_as(&at("Proton 9.0", ProtonSource::SteamApp)));
    // A copy someone dropped into compatibilitytools.d is not Steam's.
    assert!(!experimental.is_installed_as(&at("Proton - Experimental", ProtonSource::SystemCompatTools)));
    assert_eq!(experimental.install_url(), "steam://install/1493710");
}

#[test]
fn a_tag_names_its_family() {
    assert_eq!(ProtonFamily::of_tag("GE-Proton11-7"), Some(ProtonFamily::GloriousEggroll));
    assert_eq!(ProtonFamily::of_tag("cachyos-11.0-20261015-slr"), Some(ProtonFamily::CachyOs));
    assert_eq!(ProtonFamily::of_tag("proton-11.0-2-x86_64"), None);
    assert!(ProtonFlavor::CachyOs.is_supported());
}

#[test]
fn an_xz_build_unpacks_like_a_gz_one() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("release.tar.xz");
    let xz = liblzma::write::XzEncoder::new(std::fs::File::create(&archive).unwrap(), 1);
    let mut tar = tar::Builder::new(xz);
    for (name, bytes) in BUILD {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();

    let root = tmp.path().join("proton");
    let built = unpack(&archive, &root).unwrap();
    assert!(built.join("files/bin/wine").is_file());
}

#[test]
fn legacy_single_asset_release_resolves() {
    let r = with_assets(
        "GE-Proton10-34",
        &["GE-Proton10-34.sha512sum", "GE-Proton10-34.tar.gz"],
    );
    let tarball = r.tarball_for("x86_64").unwrap();
    assert_eq!(tarball.name, "GE-Proton10-34.tar.gz");
    assert_eq!(
        r.checksum_for(tarball).unwrap().name,
        "GE-Proton10-34.sha512sum"
    );
    // No ARM build in this era.
    assert!(r.tarball_for("aarch64").is_none());
}

#[test]
fn sha512sum_line_yields_the_digest() {
    let digest = "a".repeat(128);
    assert_eq!(
        parse_sha512sum(&format!("{digest}  GE-Proton11-6.tar.gz\n")),
        Some(digest.clone())
    );
    // Uppercase digests normalize.
    assert_eq!(
        parse_sha512sum(&format!("{}  f", "A".repeat(128))),
        Some("a".repeat(128))
    );
}

#[test]
fn malformed_checksums_are_rejected() {
    assert_eq!(parse_sha512sum(""), None);
    assert_eq!(parse_sha512sum("deadbeef  short.tar.gz"), None);
    assert_eq!(parse_sha512sum(&format!("{}  f", "z".repeat(128))), None);
}

#[test]
fn pin_round_trips_through_an_optional_string() {
    assert_eq!(ProtonPin::from(None), ProtonPin::Default);
    assert_eq!(ProtonPin::Default.version(), None);
    assert_eq!(
        ProtonPin::from(Some("GE-Proton11-6".to_owned())).version(),
        Some("GE-Proton11-6")
    );
}
