use super::*;
use crate::addons::catalog::Request;
use crate::addons::install::Record;
use crate::addons::Collection;

#[test]
fn a_pass_saving_keeps_choices_made_while_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("addons.json");
    let record = |file: &str| Record {
        provider: "curse".into(),
        id: "1".into(),
        name: "BigWigs".into(),
        file_id: Some(file.into()),
        folders: vec!["BigWigs".into()],
        ..Default::default()
    };
    // The pass read this, then took minutes installing file 101.
    let mut before = Ledger::default();
    before.record(record("100"));
    before.save(&path).unwrap();

    // Meanwhile the user ignored it and pinned it to what is installed.
    let mut now = before.clone();
    let row = now.find_mut("curse", "1").unwrap();
    row.ignored = true;
    row.pinned = Some("101".into());
    now.save(&path).unwrap();

    save_merged(&path, vec![(record("101"), None)], false).unwrap();
    let saved = Ledger::load(&path).unwrap();
    let row = saved.find("curse", "1").unwrap();
    assert_eq!(row.file_id.as_deref(), Some("101"), "the install is recorded");
    assert!(row.ignored, "and the ignore set meanwhile survives");
    assert_eq!(row.pinned.as_deref(), Some("101"));
}

const TOKEN: &str = concat!(
    "eyJjb2xsZWN0aW9uX25hbWUiOiJXb3dVcF9leHBvcnRfMSIsImNsaWVudF90eXBlIjoiUmV0YWlsIiwi",
    "YWRkb25zIjpbeyJpZCI6IjIzODIiLCJuYW1lIjoiQmlnV2lncyIsInByb3ZpZGVyX25hbWUiOiJDdXJz",
    "ZSIsInZlcnNpb25faWQiOiI4ODY2NTA4In0seyJpZCI6IjEzMzQ0IiwibmFtZSI6IkJ1Z0dyYWJiZXIi",
    "LCJwcm92aWRlcl9uYW1lIjoiQ3Vyc2UiLCJ2ZXJzaW9uX2lkIjoiODkwNzU4NyJ9XX0"
);

fn options_with(channel: crate::addons::Channel, ledger: &Path) -> Options {
    Options {
        flavor: "retail".to_owned(),
        addons_dir: PathBuf::from("/nowhere"),
        ledger_path: ledger.to_path_buf(),
        pinned: false,
        reinstall: false,
        channel,
        backup: None,
    }
}

#[test]
fn an_addons_own_channel_beats_the_default_and_none_means_follow_it() {
    use crate::addons::Channel;

    let mut ledger = Ledger::default();
    ledger.record(Record {
        provider: "curse".into(),
        id: "2382".into(),
        name: "BigWigs".into(),
        file_id: None,
        folders: vec!["BigWigs".into()],
        channel: Some(Channel::Alpha),
        ..Default::default()
    });
    ledger.record(Record {
        provider: "curse".into(),
        id: "13344".into(),
        name: "BugGrabber".into(),
        file_id: None,
        folders: vec!["BugGrabber".into()],
        channel: None,
        ..Default::default()
    });

    let options = options_with(Channel::Beta, Path::new("/nowhere/ledger.json"));
    let ask = |id: &str| Request {
        provider: "curse".to_owned(),
        id: id.to_owned(),
        version_id: None,
        channel: None,
    };

    assert_eq!(effective_channel(&ledger, &ask("2382"), &options), Channel::Alpha);
    // No opinion of its own, so it moves with the default.
    assert_eq!(effective_channel(&ledger, &ask("13344"), &options), Channel::Beta);
    // And an addon the ledger has never seen takes the default too.
    assert_eq!(effective_channel(&ledger, &ask("999"), &options), Channel::Beta);
}

#[test]
fn an_ignored_addon_is_left_out_of_bulk_work_but_can_still_be_asked_for() {
    use crate::addons::Channel;

    let dir = std::env::temp_dir().join(format!("lf-ignore-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ledger.json");

    let mut ledger = Ledger::default();
    ledger.record(Record {
        provider: "curse".into(),
        id: "2382".into(),
        name: "BigWigs".into(),
        file_id: Some("100".into()),
        folders: vec!["BigWigs".into()],
        ..Default::default()
    });
    ledger.record(Record {
        provider: "curse".into(),
        id: "13344".into(),
        name: "BugGrabber".into(),
        file_id: Some("200".into()),
        folders: vec!["BugGrabber".into()],
        ..Default::default()
    });
    ledger.save(&path).unwrap();

    let options = options_with(Channel::Stable, &path);
    set_ignored(&options, "curse", "13344", true).unwrap();

    let reloaded = Ledger::load(&path).unwrap();
    assert!(!reloaded.find("curse", "2382").unwrap().ignored);
    assert!(reloaded.find("curse", "13344").unwrap().ignored);

    // What "update everything" would ask for: the one addon still in.
    let requests = update_requests(&reloaded, Channel::Stable);
    let asked: Vec<&str> = requests.iter().map(|request| request.id.as_str()).collect();
    assert_eq!(asked, vec!["2382"], "an ignored addon is never asked about");

    // Ignoring is not a one-way door.
    set_ignored(&options, "curse", "13344", false).unwrap();
    assert!(!Ledger::load(&path).unwrap().find("curse", "13344").unwrap().ignored);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_update_follows_the_default_until_an_addon_says_otherwise() {
    use crate::addons::Channel;

    let mut ledger = Ledger::default();
    let mut add = |id: &str, auto: Option<bool>, ignored: bool| {
        ledger.record(Record {
            provider: "curse".into(),
            id: id.into(),
            name: id.into(),
            folders: vec![id.into()],
            auto_update: auto,
            ignored,
            ..Default::default()
        });
    };
    add("follows", None, false);
    add("opted-in", Some(true), false);
    add("opted-out", Some(false), false);
    add("ignored-but-opted-in", Some(true), true);

    let asked = |default_auto: bool| {
        let mut ids: Vec<String> = auto_update_requests(&ledger, Channel::Stable, default_auto)
            .iter()
            .map(|request| request.id.clone())
            .collect();
        ids.sort();
        ids
    };

    // Ignoring beats auto-updating: "leave this alone" is the stronger
    // of the two statements, whichever way the default points.
    assert_eq!(asked(true), vec!["follows", "opted-in"]);
    assert_eq!(asked(false), vec!["opted-in"]);

    // And "Update all" is a different question: it asks for everything
    // not ignored, whatever each addon does on its own.
    let mut all: Vec<String> = update_requests(&ledger, Channel::Stable)
        .iter()
        .map(|request| request.id.clone())
        .collect();
    all.sort();
    assert_eq!(all, vec!["follows", "opted-in", "opted-out"]);
}

#[test]
fn setting_an_auto_update_choice_survives_a_reload_and_can_be_cleared() {
    use crate::addons::Channel;

    let dir = std::env::temp_dir().join(format!("lf-auto-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ledger.json");

    let mut ledger = Ledger::default();
    ledger.record(Record {
        provider: "curse".into(),
        id: "2382".into(),
        name: "BigWigs".into(),
        folders: vec!["BigWigs".into()],
        ..Default::default()
    });
    ledger.save(&path).unwrap();

    let options = options_with(Channel::Stable, &path);
    set_auto_update(&options, "curse", "2382", Some(false)).unwrap();
    let record = Ledger::load(&path).unwrap().find("curse", "2382").unwrap().clone();
    assert_eq!(record.auto_update, Some(false));
    assert!(!record.updates_itself(true), "its own choice beats the default");

    set_auto_update(&options, "curse", "2382", None).unwrap();
    let record = Ledger::load(&path).unwrap().find("curse", "2382").unwrap().clone();
    assert_eq!(record.auto_update, None, "cleared means follow the default");
    assert!(record.updates_itself(true));
    assert!(!record.updates_itself(false));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_folder_of_a_package_matches_but_it_is_still_one_addon() {
    use crate::addons::catalog::Match;

    let bigwigs = Match {
        id: "2382".to_owned(),
        name: Some("BigWigs".to_owned()),
        file_id: Some("9017404".to_owned()),
        folders: vec!["BigWigs".to_owned(), "BigWigs_Core".to_owned()],
    };
    let other = Match {
        id: "1477613".to_owned(),
        name: Some("EllesmereUI".to_owned()),
        file_id: Some("9069177".to_owned()),
        folders: vec!["EllesmereUI".to_owned()],
    };

    // Three hashes, three answers, two addons, which is exactly the
    // shape that produced twelve BigWigs rows before this was grouped.
    let matches = std::collections::BTreeMap::from([
        ("10".to_owned(), bigwigs.clone()),
        ("20".to_owned(), bigwigs),
        ("30".to_owned(), other),
    ]);
    let by_print = std::collections::BTreeMap::from([
        (10, "BigWigs".to_owned()),
        (20, "BigWigs_Core".to_owned()),
        (30, "EllesmereUI".to_owned()),
        (40, "NotMatched".to_owned()),
    ]);

    let grouped = group_by_project(&matches, &by_print);
    assert_eq!(grouped.len(), 2, "one entry per project, not per folder");
    assert_eq!(grouped["2382"].1, ["BigWigs", "BigWigs_Core"]);
    assert_eq!(grouped["1477613"].1, ["EllesmereUI"]);

    // A hash nobody claimed contributes nothing, not an entry
    // with no project.
    assert!(grouped.values().all(|(_, folders)| !folders.contains(&"NotMatched".to_owned())));
}

#[test]
fn adoption_only_claims_folders_that_are_free_and_present() {
    // The decision adoption makes, without the network: given what the
    // catalog recognised, which folders may a new record claim?
    let claimed: std::collections::BTreeSet<String> =
        ["BigWigs".to_owned(), "BigWigs_Core".to_owned()].into_iter().collect();
    let on_disk: std::collections::BTreeSet<&str> =
        ["BigWigs", "BigWigs_Core", "Details", "Details_Streamer"].into_iter().collect();

    let claim = |matched: &str, package: &[&str]| -> Vec<String> {
        let mut folders: Vec<String> = package
            .iter()
            .filter(|f| on_disk.contains(*f) && !claimed.contains(**f))
            .map(|f| (*f).to_owned())
            .collect();
        if !folders.iter().any(|f| f == matched) {
            folders.push(matched.to_owned());
        }
        folders.sort();
        folders.dedup();
        folders
    };

    // The whole package, because a match on one folder identifies all of
    // it, adopting Details and leaving Details_Streamer unmanaged would
    // be worse than adopting neither.
    assert_eq!(claim("Details", &["Details", "Details_Streamer"]), ["Details", "Details_Streamer"]);

    // A folder the catalog lists but that is not installed is not
    // claimed: recording it would make Remove promise to delete it.
    assert_eq!(claim("Details", &["Details", "Details_Nope"]), ["Details"]);

    // And nothing is taken from a record that already owns it, which is
    // what stops adoption competing with the entry that installed it.
    assert_eq!(claim("Details", &["Details", "BigWigs_Core"]), ["Details"]);
}

#[test]
fn a_pinned_addon_is_asked_about_by_nothing() {
    use crate::addons::Channel;

    let dir = std::env::temp_dir().join(format!("lf-pin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ledger.json");

    let mut ledger = Ledger::default();
    ledger.record(Record {
        provider: "curse".into(),
        id: "2382".into(),
        name: "BigWigs".into(),
        file_id: Some("100".into()),
        folders: vec!["BigWigs".into()],
        ..Default::default()
    });
    ledger.save(&path).unwrap();

    let options = options_with(Channel::Stable, &path);
    set_pinned(&options, "curse", "2382", Some("100".to_owned())).unwrap();

    let reloaded = Ledger::load(&path).unwrap();
    let record = reloaded.find("curse", "2382").unwrap();
    assert_eq!(record.pinned.as_deref(), Some("100"));
    // Pinning and ignoring are different statements that reach the same
    // place: an addon held still is not worth asking a provider about.
    assert!(!record.tracks_updates());
    assert!(!record.updates_itself(true));
    assert!(update_requests(&reloaded, Channel::Stable).is_empty());
    assert!(auto_update_requests(&reloaded, Channel::Stable, true).is_empty());

    // Unpinning is not a one-way door either.
    set_pinned(&options, "curse", "2382", None).unwrap();
    let reloaded = Ledger::load(&path).unwrap();
    assert!(reloaded.find("curse", "2382").unwrap().tracks_updates());
    assert_eq!(update_requests(&reloaded, Channel::Stable).len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pin_describes_a_file_and_is_dropped_once_it_no_longer_does() {
    let resolve = |asked: Option<&str>, holding: bool, held: Option<&str>, got: &str| {
        pin_after_install(
            asked.map(str::to_owned),
            holding,
            held.map(str::to_owned),
            Some(got),
        )
    };

    // A version picked from the list is held there.
    assert_eq!(resolve(Some("100"), true, None, "100").as_deref(), Some("100"));
    // A file named by anything else, an import token's say, pins nothing.
    assert_eq!(resolve(Some("100"), false, None, "100"), None);
    // An ordinary reinstall at the pinned file keeps the pin.
    assert_eq!(resolve(None, false, Some("100"), "100").as_deref(), Some("100"));
    // Installing a different file has moved the addon off the pin, and a
    // pin naming a file that is not there would be a lie.
    assert_eq!(resolve(None, false, Some("100"), "101"), None);
}

#[test]
fn setting_a_channel_survives_a_reload_and_clearing_it_returns_to_the_default() {
    use crate::addons::Channel;

    let dir = std::env::temp_dir().join(format!("lf-channel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ledger.json");

    let mut ledger = Ledger::default();
    ledger.record(Record {
        provider: "curse".into(),
        id: "2382".into(),
        name: "BigWigs".into(),
        file_id: None,
        folders: vec!["BigWigs".into()],
        channel: None,
        ..Default::default()
    });
    ledger.save(&path).unwrap();

    let options = options_with(Channel::Stable, &path);
    set_channel(&options, "curse", "2382", Some(Channel::Beta)).unwrap();
    assert_eq!(
        Ledger::load(&path).unwrap().find("curse", "2382").unwrap().channel,
        Some(Channel::Beta)
    );

    set_channel(&options, "curse", "2382", None).unwrap();
    assert_eq!(
        Ledger::load(&path).unwrap().find("curse", "2382").unwrap().channel,
        None,
        "cleared means follow the default, not stable"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_token_becomes_requests_that_keep_the_pin() {
    let collection = Collection::parse(TOKEN).unwrap();
    let requests = requests_from(&collection);

    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].provider, "curse");
    assert_eq!(requests[0].version_id.as_deref(), Some("8866508"));
    assert!(unsupported(&collection).is_empty());
}

#[test]
fn an_unsupported_provider_is_reported_rather_than_requested() {
    let json = br#"{"collection_name":"x","client_type":"Retail","addons":[
        {"id":"1","name":"Known","provider_name":"Curse"},
        {"id":"2","name":"Odd","provider_name":"SomethingElse"}]}"#;
    let collection: Collection = serde_json::from_slice(json).unwrap();

    assert_eq!(requests_from(&collection).len(), 1);
    let skipped = unsupported(&collection);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].name, "Odd");
}

#[test]
fn a_published_md5_is_enforced_in_both_directions() {
    let package = b"addon bytes";
    let digest = format!("{:x}", md5::compute(package));

    assert!(verify_md5(package, Some(&digest), "BigWigs").is_ok());
    // Case differences are not corruption.
    assert!(verify_md5(package, Some(&digest.to_uppercase()), "BigWigs").is_ok());
    // A provider that publishes no digest is not an error.
    assert!(verify_md5(package, None, "ElvUI").is_ok());

    let err = verify_md5(b"tampered", Some(&digest), "BigWigs").unwrap_err();
    assert!(matches!(err, ManageError::Checksum { .. }), "{err}");
}

#[test]
fn a_channel_change_is_a_switch_rather_than_an_update() {
    use crate::addons::Channel;

    let status = |installed_build, available_build| UpdateStatus {
        provider: "curse".into(),
        id: "1682733".into(),
        name: "Naowh Smart Reminders".into(),
        // Different files, so `has_update` is true either way, which is
        // exactly why the label cannot be decided from that alone.
        installed: Some("8986554".into()),
        available: Some("8986546".into()),
        installed_build,
        available_build,
        available_name: Some("NaowhSmartReminders-1.4.26.zip".into()),
        available_date: None,
        problem: None,
    };

    // The real case: an alpha was installed, stable is now what is wanted,
    // and the stable file is *older*. Offering "Update" would be a lie.
    let back_to_stable = status(Some(Channel::Alpha), Some(Channel::Stable));
    assert!(back_to_stable.has_update());
    assert!(back_to_stable.is_channel_switch());

    // Opting into a pre-release is a switch too, not an update.
    assert!(status(Some(Channel::Stable), Some(Channel::Alpha)).is_channel_switch());

    // Moving forward on the same channel is an ordinary update.
    assert!(!status(Some(Channel::Stable), Some(Channel::Stable)).is_channel_switch());

    // Installed before builds were recorded: do not guess.
    assert!(!status(None, Some(Channel::Stable)).is_channel_switch());
    assert!(!status(Some(Channel::Beta), None).is_channel_switch());
}

#[test]
fn an_unknown_remote_version_is_not_an_update() {
    let status = |installed: Option<&str>, available: Option<&str>| UpdateStatus {
        provider: "curse".into(),
        id: "1".into(),
        name: "Thing".into(),
        installed: installed.map(str::to_owned),
        available: available.map(str::to_owned),
        installed_build: None,
        available_build: None,
        available_name: None,
        available_date: None,
        problem: None,
    };

    assert!(status(Some("100"), Some("101")).has_update());
    assert!(!status(Some("100"), Some("100")).has_update());
    // Provider down, or an addon with no recorded version: unknown, and
    // unknown must not render as "update available".
    assert!(!status(Some("100"), None).has_update());
    assert!(!status(None, Some("101")).has_update());
}

#[test]
fn a_report_is_complete_only_when_nothing_was_left_out() {
    let mut report = Report {
        installed: vec!["BigWigs".into()],
        ..Report::default()
    };
    assert!(report.is_complete());

    report.unavailable.push(Unavailable {
        name: "EXCore".into(),
        reason: "opted out".into(),
        website_url: Some("https://curseforge.com/x".into()),
        github: None,
    });
    assert!(!report.is_complete());
}
