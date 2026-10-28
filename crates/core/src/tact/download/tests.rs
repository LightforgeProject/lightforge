use super::*;
use crate::tact::config::CKey;
use crate::text::hex;
use crate::tact::plan::Plan;
use crate::tact::writer::StoreWriter;

mod downloading {
    use super::*;

    fn ckey_of(payload: &[u8]) -> CKey {
        *md5::compute(payload)
    }

    fn blte(payload: &[u8]) -> Vec<u8> {
        blte::compress_data_single(payload.to_vec(), blte::CompressionMode::ZLib, None).unwrap()
    }

    /// What the encoding file would list: the MD5 of the chunk table, or of
    /// the whole file when it has none.
    fn ekey_of(encoded: &[u8]) -> EKey {
        let header = u32::from_be_bytes(encoded[4..8].try_into().unwrap()) as usize;
        *md5::compute(if header == 0 { encoded } else { &encoded[..header] })
    }

    #[test]
    fn a_file_matching_its_content_key_verifies() {
        let encoded = blte(b"the real bytes");
        assert!(matches!(
            verify(&encoded, &ekey_of(&encoded), Some(&ckey_of(b"the real bytes")), None),
            Verified::Ok
        ));
    }

    #[test]
    fn a_file_that_decodes_to_something_else_is_refused_not_stored() {
        let encoded = blte(b"the real bytes");
        let wrong = ckey_of(b"something else entirely");

        match verify(&encoded, &ekey_of(&encoded), Some(&wrong), None) {
            Verified::Wrong(reason) => {
                // The message has to name both, or a mismatch is unactionable.
                assert!(reason.contains(&hex(&wrong)), "{reason}");
                assert!(reason.contains(&hex(&ckey_of(b"the real bytes"))), "{reason}");
            }
            _ => panic!("a content-key mismatch must not pass"),
        }
    }

    #[test]
    fn a_file_encoding_does_not_list_is_stored_unverified() {
        // The 2-in-1.5-million case: no content key to check it against.
        let encoded = blte(b"x");
        assert!(matches!(verify(&encoded, &ekey_of(&encoded), None, None), Verified::Unverifiable));
    }

    #[test]
    fn a_file_without_a_content_key_must_still_match_its_encoding_key() {
        let encoded = blte(b"x");
        let other = blte(b"something else");
        assert!(matches!(verify(&other, &ekey_of(&encoded), None, None), Verified::Wrong(_)));
        assert!(matches!(verify(b"not blte at all", &ekey_of(&encoded), None, None), Verified::Wrong(_)));
    }

    /// A BLTE container with an explicit chunk table.
    fn blte_chunks(modes: &[u8]) -> Vec<u8> {
        let header_size = 12 + 24 * modes.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"BLTE");
        out.extend_from_slice(&(header_size as u32).to_be_bytes());
        out.push(0x0F);
        out.extend_from_slice(&(modes.len() as u32).to_be_bytes()[1..]);

        // Each chunk here is one mode byte plus three of payload, and its
        // table row carries the MD5 of the chunk as stored, as Blizzard's do.
        let chunks: Vec<[u8; 4]> = modes.iter().map(|&mode| [mode, b'a', b'b', b'c']).collect();
        for chunk in &chunks {
            out.extend_from_slice(&4u32.to_be_bytes());
            out.extend_from_slice(&3u32.to_be_bytes());
            out.extend_from_slice(&md5::compute(chunk).0);
        }
        for chunk in &chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    #[test]
    fn an_encrypted_chunk_is_recognised_wherever_it_sits() {
        assert!(has_encrypted_chunk(&blte_chunks(b"E")));
        // Blizzard mixes modes within one file, so a later chunk counts too.
        assert!(has_encrypted_chunk(&blte_chunks(b"ZZE")));
        assert!(has_encrypted_chunk(&blte_chunks(b"ZEZ")));

        assert!(!has_encrypted_chunk(&blte_chunks(b"Z")));
        assert!(!has_encrypted_chunk(&blte_chunks(b"ZNZ")));
    }

    #[test]
    fn a_single_chunk_container_declares_no_header() {
        // header_size 0: one chunk, its mode byte immediately after.
        let mut encrypted = Vec::from(*b"BLTE");
        encrypted.extend_from_slice(&0u32.to_be_bytes());
        encrypted.extend_from_slice(b"Epayload");
        assert!(has_encrypted_chunk(&encrypted));

        let mut plain = Vec::from(*b"BLTE");
        plain.extend_from_slice(&0u32.to_be_bytes());
        plain.extend_from_slice(b"Zpayload");
        assert!(!has_encrypted_chunk(&plain));
    }

    #[test]
    fn malformed_containers_are_not_encrypted_and_do_not_panic() {
        assert!(!has_encrypted_chunk(b""));
        assert!(!has_encrypted_chunk(b"BLTE"));
        assert!(!has_encrypted_chunk(b"not blte at all"));

        // A header claiming more than the file holds must not index past it.
        let mut lying = Vec::from(*b"BLTE");
        lying.extend_from_slice(&9999u32.to_be_bytes());
        lying.extend_from_slice(&[0u8; 8]);
        assert!(!has_encrypted_chunk(&lying));

        // A chunk count larger than the table.
        let mut truncated = blte_chunks(b"ZZE");
        truncated.truncate(20);
        assert!(!has_encrypted_chunk(&truncated));
    }

    #[test]
    fn an_encrypted_file_is_stored_unverified_rather_than_refused() {
        // The end-to-end shape of the Classic Era bug: a real encrypted file
        // fails to decode, and must still reach the store.
        let encrypted = blte_chunks(b"E");
        assert!(matches!(
            verify(&encrypted, &ekey_of(&encrypted), Some(&ckey_of(b"whatever")), None),
            Verified::Unverifiable
        ));

        // Corruption that is not encrypted is still refused.
        let plain = blte_chunks(b"Z");
        assert!(matches!(
            verify(&plain, &ekey_of(&plain), Some(&ckey_of(b"whatever")), None),
            Verified::Wrong(_)
        ));
    }

    #[test]
    fn an_encrypted_chunk_does_not_carry_altered_bytes_past_the_check() {
        let published = blte_chunks(b"ZE");
        let ekey = ekey_of(&published);
        let ckey = ckey_of(b"whatever");

        // Bytes changed inside the encrypted chunk itself.
        let mut altered = published.clone();
        *altered.last_mut().unwrap() ^= 0xff;
        assert!(matches!(verify(&altered, &ekey, Some(&ckey), None), Verified::Wrong(_)));

        // A whole file rebuilt around an encrypted chunk: its own table is
        // consistent, but it is not the one the encoding key names.
        let forged = blte_chunks(b"EZ");
        assert!(matches!(verify(&forged, &ekey, Some(&ckey), None), Verified::Wrong(_)));
    }

    #[test]
    fn junk_is_refused_rather_than_counted_as_unverifiable() {
        // Corruption must not pass as encryption.
        match verify(b"not blte at all", &[0; 16], Some(&ckey_of(b"x")), None) {
            Verified::Wrong(_) => {}
            _ => panic!("junk must be refused"),
        }
    }

    #[test]
    fn install_paths_collapse_onto_a_directory_that_differs_only_by_case() {
        let dir = std::env::temp_dir().join(format!("lightforge-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The manifest spells it `Utils\cef.pak` here and `UTILS\...` there.
        // Whichever lands first decides the case, and the rest join it.
        let first = resolve_windows_path(&dir, std::path::Path::new("Utils/cef.pak"));
        crate::fsutil::write_atomic_with_mode(&first, b"cef", 0o644).unwrap();

        let second = resolve_windows_path(&dir, std::path::Path::new("UTILS/LIBCEF.DLL"));
        crate::fsutil::write_atomic_with_mode(&second, b"libcef", 0o644).unwrap();

        // One directory, not two, the split left a real install's Utils/
        // without LIBCEF.DLL and Wine could not find CEF.
        let dirs: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        assert_eq!(dirs[0], "Utils");

        // File name case is preserved: a real install holds LIBCEF.DLL next
        // to cef.pak inside that one directory.
        let mut names: Vec<String> = std::fs::read_dir(dir.join("Utils"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["LIBCEF.DLL", "cef.pak"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_with_nothing_to_match_keeps_the_spelling_it_was_given() {
        let dir = std::env::temp_dir().join(format!("lightforge-case2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = resolve_windows_path(&dir, std::path::Path::new("Utils/swiftshader/libEGL.dll"));
        assert_eq!(path, dir.join("Utils/swiftshader/libEGL.dll"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flavor_file_matches_the_shape_a_real_install_has() {
        let dir = std::env::temp_dir().join(format!("lightforge-flavor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let code: crate::ProductCode = "wow_classic_era".parse().unwrap();
        let path = write_flavor_info(&dir, &code).unwrap();

        // Byte-for-byte the shape of a real `_classic_beta_/.flavor.info`.
        assert_eq!(path.file_name().unwrap(), ".flavor.info");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Product Flavor!STRING:0\nwow_classic_era\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_install_report_totals_what_it_wrote() {
        let mut report = InstallReport::default();
        report.written.push(Extracted { path: "Wow.exe".into(), bytes: 74_778_320 });
        report.written.push(Extracted { path: "wow_loader.dll".into(), bytes: 40_693_456 });

        assert_eq!(report.bytes(), 115_471_776);
        assert!(report.is_complete());

        report.failed.push(("Utils/cef.pak".into(), "connection reset".into()));
        assert!(!report.is_complete());
    }

    #[test]
    fn pausing_a_download_trips_its_token_and_leaves_a_restart_owed() {
        use crate::preflight::DownloadGuard;

        let code: crate::ProductCode = "wow".parse().unwrap();
        let cancel = Cancel::new();
        let mut downloads = Downloads::new();
        downloads.register(code.clone(), cancel.clone());

        // The launch path pauses: the loop stops, and a restart is owed.
        assert!(downloads.pause(&code).unwrap());
        assert!(cancel.is_cancelled());
        assert_eq!(downloads.pending().count(), 1);

        // After Battle.net exits, the caller is told to start it again, once.
        assert!(downloads.resume(&code).unwrap());
        assert!(!downloads.resume(&code).unwrap());
        assert_eq!(downloads.pending().count(), 0);
    }

    #[test]
    fn a_product_with_no_download_owes_nothing_either_way() {
        use crate::preflight::DownloadGuard;

        let code: crate::ProductCode = "wow".parse().unwrap();
        let mut downloads = Downloads::new();

        // Pausing nothing owes no resume.
        assert!(!downloads.pause(&code).unwrap());
        assert!(!downloads.resume(&code).unwrap());
    }

    #[test]
    fn a_download_that_finished_on_its_own_is_not_owed_a_restart() {
        use crate::preflight::DownloadGuard;

        let code: crate::ProductCode = "wow".parse().unwrap();
        let mut downloads = Downloads::new();
        downloads.register(code.clone(), Cancel::new());
        downloads.finished(&code);

        assert!(!downloads.pause(&code).unwrap());
        assert!(!downloads.resume(&code).unwrap());
    }

    #[test]
    fn a_small_download_still_checkpoints_several_times() {
        let options = Options::default();

        // The bug this guards: Classic Era is 4.5 GiB against a 4 GiB default,
        // so it would have checkpointed once at the very end, if at all. A
        // restart orphaned 3.2 GB because no index had ever been written.
        let classic_era = 4_900_000_000u64;
        let interval = options.interval_for(classic_era);
        assert!(interval < options.checkpoint_bytes);
        assert!(classic_era / interval >= 6, "only {} checkpoints", classic_era / interval);

        // A big install still honours the cap rather than checkpointing on an
        // eighth of 123 GiB.
        let retail = 132_000_000_000u64;
        assert_eq!(options.interval_for(retail), options.checkpoint_bytes);

        // A tiny plan does not rewrite sixteen indices every few megabytes.
        assert_eq!(options.interval_for(1_000_000), MIN_CHECKPOINT_BYTES);
        assert!(options.interval_for(0) > 0);
    }

    #[test]
    fn a_cancel_token_is_shared_between_clones() {
        let cancel = Cancel::new();
        let clone = cancel.clone();
        assert!(!cancel.is_cancelled());

        clone.cancel();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn progress_is_measured_in_bytes_not_files() {
        let mut state = Progress { bytes_total: 1000, ..Progress::default() };
        assert_eq!(state.fraction(), 0.0);
        state.bytes_stored = 250;
        assert_eq!(state.fraction(), 0.25);

        // A plan with nothing to do is finished, not divided by zero.
        assert_eq!(Progress::default().fraction(), 1.0);
    }

    #[test]
    fn an_outcome_is_complete_only_when_nothing_failed_and_nothing_was_cancelled() {
        let mut outcome = Outcome { files_stored: 10, ..Outcome::default() };
        assert!(outcome.is_complete());

        // Unverified files are expected and do not make a run incomplete.
        outcome.unverified = 3;
        assert!(outcome.is_complete());

        outcome.cancelled = true;
        assert!(!outcome.is_complete());
        outcome.cancelled = false;
        outcome.failed.push(Failure { ekey: [0; 16], reason: "timed out".into() });
        assert!(!outcome.is_complete());
    }
}

mod rate_limit {
    use super::*;

    #[test]
    fn no_limit_is_no_limiter_at_all() {
        assert!(RateLimit::new(0).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn the_average_converges_on_the_limit() {
        // 1 MB/s, so ten 1 MB requests must take about ten seconds, minus
        // the first, which the full bucket pays for outright.
        let limit = RateLimit::new(1_000_000).unwrap();
        let started = tokio::time::Instant::now();
        for _ in 0..10 {
            limit.acquire(1_000_000).await;
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!((8.5..=10.5).contains(&elapsed), "took {elapsed}s");
        // Not merely slow: an unlimited loop would finish instantly.
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_is_bounded_by_one_second_of_the_limit() {
        let limit = RateLimit::new(1_000_000).unwrap();

        // The bucket starts full, so exactly one second's worth is free.
        let free = tokio::time::Instant::now();
        limit.acquire(1_000_000).await;
        assert!(free.elapsed().as_secs_f64() < 0.1, "the first second should not wait");

        // The next one has to be earned.
        let earned = tokio::time::Instant::now();
        limit.acquire(1_000_000).await;
        assert!(earned.elapsed().as_secs_f64() >= 0.9, "a second burst was let through");
    }

    /// A run larger than the bucket (8 MiB at 1 MB/s) still goes, or the
    /// download deadlocks.
    #[tokio::test(start_paused = true)]
    async fn a_request_larger_than_the_bucket_still_goes_through() {
        let limit = RateLimit::new(1_000_000).unwrap();
        limit.acquire(8 * 1024 * 1024).await;
        // And the overdraft is repaid before the next one starts.
        let started = tokio::time::Instant::now();
        limit.acquire(1).await;
        assert!(started.elapsed().as_secs_f64() >= 7.0, "overdraft was not repaid");
    }
}

mod pausing {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::tact::cdn::Endpoint;
    use crate::tact::plan::Loose;

    /// A CDN that starts a response and then goes quiet with the connection
    /// held open, the stall a pause used to wait on forever.
    fn stalling_server() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for mut stream in listener.incoming().flatten() {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\n\r\n");
                let _ = stream.write_all(&[0u8; 16]);
                held.push(stream);
            }
        });
        origin
    }

    #[tokio::test]
    async fn a_pause_does_not_wait_on_a_stalled_fetch() {
        let http = Http::new().unwrap();
        let client = Client::new(vec![Endpoint {
            origin: stalling_server(),
            path: "tpr/wow".into(),
        }])
        .unwrap();
        let dir = std::env::temp_dir()
            .join(format!("lightforge-pause-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut writer = StoreWriter::create(&dir).unwrap();
        let plan = Plan {
            loose: vec![Loose { ekey: [7; 16], ckey: None, size: 1 << 20 }],
            ..Plan::default()
        };

        let cancel = Cancel::new();
        let tripper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            tripper.cancel();
        });

        let started = Instant::now();
        let outcome = Downloader::new(Options::default())
            .run(
                Source { http: &http, client: &client, keys: None },
                &plan,
                &mut writer,
                &cancel,
                |_| {},
            )
            .await
            .unwrap();

        // Well inside the 30 s read timeout: the pause itself ended the run.
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        assert!(outcome.cancelled);
        assert_eq!(outcome.files_stored, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
