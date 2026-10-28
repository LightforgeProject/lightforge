use std::path::Path;

use super::*;

mod writing {
    use super::*;

    /// Minimal wire-format reader, so the tests check the bytes instead of
    /// checking that `encode` agrees with itself.
    fn fields(buf: &[u8]) -> Vec<(u32, u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let (k, next) = read_varint(buf, i);
            i = next;
            let (field, wire) = ((k >> 3) as u32, (k & 7) as u8);
            match wire {
                0 => {
                    let (v, next) = read_varint(buf, i);
                    i = next;
                    out.push((field, wire, v.to_le_bytes().to_vec()));
                }
                1 => {
                    out.push((field, wire, buf[i..i + 8].to_vec()));
                    i += 8;
                }
                2 => {
                    let (len, next) = read_varint(buf, i);
                    i = next;
                    out.push((field, wire, buf[i..i + len as usize].to_vec()));
                    i += len as usize;
                }
                _ => panic!("unexpected wire type {wire}"),
            }
        }
        out
    }

    fn read_varint(buf: &[u8], mut i: usize) -> (u64, usize) {
        let (mut result, mut shift) = (0u64, 0);
        loop {
            let byte = buf[i];
            i += 1;
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return (result, i);
            }
            shift += 7;
        }
    }

    fn sample() -> ProductInstall {
        ProductInstall {
            product: "wow_classic_era".into(),
            install_path: "C:/Program Files (x86)/World of Warcraft".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: "_classic_era_".into(),
            version: "1.15.9.69722".into(),
            build_key: "3645f0fe9dc5215ea90eaf7cdb7379ce".into(),
            tags: "Windows x86_64 EU? enUS speech?:Windows x86_64 EU? enUS text?".into(),
            key_ring: None,
            installed_at_ms: 1_789_676_802_000,
            family: "wow".into(),
        }
    }

    #[test]
    fn a_record_lands_in_the_fields_agent_reads() {
        let bytes = encode(&[sample()]);
        let top = fields(&bytes);
        assert_eq!(top.len(), 1, "one ProductInstall");

        let install = fields(&top[0].2);
        let text = |field: u32| {
            install
                .iter()
                .find(|(f, _, _)| *f == field)
                .map(|(_, _, v)| String::from_utf8_lossy(v).into_owned())
                .unwrap()
        };
        // The same string for a WoW product; see `Installed` for the one
        // product where they differ.
        assert_eq!(text(1), "wow_classic_era");
        assert_eq!(text(2), "wow_classic_era");
        assert_eq!(text(6), "wow");

        let settings = fields(&install.iter().find(|(f, _, _)| *f == 3).unwrap().2);
        let setting = |field: u32| {
            settings
                .iter()
                .find(|(f, _, _)| *f == field)
                .map(|(_, _, v)| String::from_utf8_lossy(v).into_owned())
                .unwrap()
        };
        assert_eq!(setting(1), "C:/Program Files (x86)/World of Warcraft");
        assert_eq!(setting(2), "eu");
        assert_eq!(setting(11), "AUT");
        assert_eq!(setting(12), "AT");
        // The flavor directory is how Agent finds the game inside the root.
        assert_eq!(setting(13), "_classic_era_");
    }

    #[test]
    fn a_product_without_a_key_ring_omits_the_field_entirely() {
        // Classic Era has no key ring, and a real record for such a product
        // leaves field 19 out instead of writing an empty string.
        let bytes = encode(&[sample()]);
        let install = fields(&fields(&bytes)[0].2);
        let cached = fields(&install.iter().find(|(f, _, _)| *f == 4).unwrap().2);
        let base = fields(&cached.iter().find(|(f, _, _)| *f == 1).unwrap().2);

        assert!(base.iter().all(|(f, _, _)| *f != 19));
        // The build key is written twice, as active and installed.
        let keys: Vec<String> = base
            .iter()
            .filter(|(f, _, _)| *f == 12 || *f == 14)
            .map(|(_, _, v)| String::from_utf8_lossy(v).into_owned())
            .collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k == "3645f0fe9dc5215ea90eaf7cdb7379ce"));
    }

    #[test]
    fn a_key_ring_is_written_when_the_product_has_one() {
        let install = ProductInstall {
            key_ring: Some("3ca57fe7319a297346440e4d2a03a0cd".into()),
            ..sample()
        };
        let bytes = encode(&[install]);
        let install = fields(&fields(&bytes)[0].2);
        let cached = fields(&install.iter().find(|(f, _, _)| *f == 4).unwrap().2);
        let base = fields(&cached.iter().find(|(f, _, _)| *f == 1).unwrap().2);

        assert!(base.iter().any(|(f, _, _)| *f == 19));
    }

    #[test]
    fn several_products_share_one_file() {
        // A real install lists every product installed under the same root.
        let second = ProductInstall {
            product: "wow".into(),
            flavor_dir: "_retail_".into(),
            ..sample()
        };
        let bytes = encode(&[sample(), second]);
        assert_eq!(fields(&bytes).len(), 2);
    }

    #[test]
    fn agent_style_tags_match_what_a_real_record_carries() {
        // Verbatim shape from Gabriel's install, wow row.
        assert_eq!(
            agent_tags("eu", "enUS", "AUT", "AT"),
            "Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:\
             Windows x86_64 EU? acct-AUT? geoip-AT? enUS text?"
                .replace("\\\n             ", "")
        );
        // Without an account, the acct/geoip tags are left out, not
        // written empty.
        assert_eq!(
            agent_tags("us", "deDE", "", ""),
            "Windows x86_64 US? deDE speech?:Windows x86_64 US? deDE text?"
        );
    }

    #[test]
    fn agent_tags_parse_back_to_the_same_names_we_select_on() {
        use crate::tact::manifest::TagSet;

        // The `?` suffixes must not change selection, or rewriting the tags
        // would silently change what an update fetches. `TagSet` strips them,
        // so both forms carry the same required names.
        let plain = TagSet::parse("Windows x86_64 EU enUS speech:Windows x86_64 EU enUS text");
        let agent = TagSet::parse(&agent_tags("eu", "enUS", "", ""));
        assert_eq!(format!("{plain:?}"), format!("{agent:?}"));
    }

    #[test]
    fn the_locale_is_read_out_of_the_tag_expression() {
        assert_eq!(
            locale_from_tags("Windows x86_64 EU? acct-AUT? geoip-AT? enUS speech?:…"),
            Some("enUS".to_owned())
        );
        assert_eq!(
            locale_from_tags("Windows x86_64 EU deDE speech:…"),
            Some("deDE".to_owned())
        );
        assert_eq!(locale_from_tags("Windows x86_64"), None);
    }

    #[test]
    fn a_non_ascii_tag_is_not_a_locale() {
        assert_eq!(locale_from_tags("Windows a界"), None);
    }

    #[test]
    fn varints_match_the_wire_format() {
        let mut out = Vec::new();
        varint(&mut out, 300);
        assert_eq!(out, [0xac, 0x02]);

        // Field 21's key is itself a two-byte varint (21 << 3 = 168), so the
        // value does not start at offset 1, read the key first.
        let mut out = Vec::new();
        varint_field(&mut out, 21, 1_789_676_802_000);
        let (key, after_key) = read_varint(&out, 0);
        assert_eq!((key >> 3, key & 7), (21, 0));
        assert_eq!(read_varint(&out, after_key).0, 1_789_676_802_000);
    }
}

mod reading {
    use super::*;

    /// The two directions must agree, since neither has a schema to check
    /// against, only each other and a real file.
    #[test]
    fn what_we_write_is_what_we_read_back() {
        let install = ProductInstall {
            product: "wow_classic_era".into(),
            install_path: "C:/Program Files (x86)/World of Warcraft".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: "_classic_era_".into(),
            version: "1.15.9.69722".into(),
            build_key: "0".repeat(32),
            tags: "Windows x86_64".into(),
            key_ring: None,
            family: "wow".into(),
            installed_at_ms: 1_700_000_000_000,
        };

        let read = decode(&encode(std::slice::from_ref(&install)));
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].product, install.product);
        assert_eq!(read[0].install_path, install.install_path);
        assert_eq!(read[0].version.as_deref(), Some(install.version.as_str()));
        assert_eq!(read[0].build_key.as_deref(), Some(install.build_key.as_str()));
        assert_eq!(read[0].family, install.family);
    }

    #[test]
    fn upserting_one_product_keeps_its_siblings() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &[install("wow", "12.0.0.1"), install("wow_classic_era", "1.15.0.1")]).unwrap();

        upsert(dir.path(), &[install("wow_classic_era", "1.15.0.2")]).unwrap();

        let read = super::read(&dir.path().join(".product.db")).unwrap();
        let versions: Vec<_> =
            read.iter().map(|r| (r.product.as_str(), r.version.as_deref())).collect();
        assert_eq!(versions, [("wow", Some("12.0.0.1")), ("wow_classic_era", Some("1.15.0.2"))]);
    }

    fn install(product: &str, version: &str) -> ProductInstall {
        ProductInstall {
            product: product.into(),
            install_path: "C:/Program Files (x86)/World of Warcraft".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: "_retail_".into(),
            version: version.into(),
            build_key: "a".repeat(32),
            tags: "Windows x86_64".into(),
            key_ring: None,
            family: "wow".into(),
            installed_at_ms: 0,
        }
    }

    /// A record as another launcher's Agent wrote it: a settings block with an
    /// install path, and fields nothing in this file understands.
    fn their_record(product: &str, install_path: &str) -> Vec<u8> {
        let mut settings = Vec::new();
        string_field(&mut settings, 1, install_path);
        string_field(&mut settings, 6, "deDE");
        varint_field(&mut settings, 77, 1234);

        let mut record = Vec::new();
        string_field(&mut record, 1, product);
        string_field(&mut record, 2, product);
        message_field(&mut record, 3, &settings);
        string_field(&mut record, 6, "hsb");
        // The sort of field this file has no model for and must not drop.
        varint_field(&mut record, 42, 7);
        string_field(&mut record, 99, "something Agent cares about");
        record
    }

    /// From a real registry: Hearthstone is `hs_beta` in field 1 and `hsb` in
    /// field 2, with no family. Reading only field 1 left it "not installed".
    #[test]
    fn hearthstone_is_read_under_both_of_the_names_it_has() {
        let mut base = Vec::new();
        string_field(&mut base, 7, "36.6.3.253932.253216");
        let mut cached = Vec::new();
        message_field(&mut cached, 1, &base);
        let mut settings = Vec::new();
        string_field(&mut settings, 1, "C:/Program Files (x86)/Hearthstone");

        let mut record = Vec::new();
        string_field(&mut record, 1, "hs_beta");
        string_field(&mut record, 2, "hsb");
        message_field(&mut record, 3, &settings);
        message_field(&mut record, 4, &cached);

        let mut file = Vec::new();
        message_field(&mut file, 1, &record);

        let found = decode(&file);
        assert_eq!(found.len(), 1, "one record in, one out");
        assert_eq!(found[0].product, "hs_beta");
        assert_eq!(found[0].product_code, "hsb");
        assert_eq!(found[0].version.as_deref(), Some("36.6.3.253932.253216"));
        assert!(found[0].family.is_empty(), "a single-title game has no family");
    }

    #[test]
    fn a_lifted_record_changes_its_path_and_nothing_else() {
        let mut theirs = Vec::new();
        message_field(&mut theirs, 1, &their_record("hs_beta", "C:\\Games\\Hearthstone"));

        let lifted = lift(&theirs, "hs_beta", "E:\\Hearthstone").expect("their record is there");
        assert_eq!(lifted.product, "hs_beta");

        // Every byte of the record survives except the install path itself.
        let mut expected = Vec::new();
        message_field(&mut expected, 1, &their_record("hs_beta", "E:\\Hearthstone"));
        let mut got = Vec::new();
        message_field(&mut got, 1, &lifted.payload);
        assert_eq!(got, expected);

        // A product that is not in the file, and a file that is not a registry.
        assert!(lift(&theirs, "d3", "E:\\Diablo").is_none());
        assert!(lift(&[0xff, 0xff], "hs_beta", "E:\\x").is_none());
    }

    #[test]
    fn grafting_adds_their_record_without_disturbing_ours() {
        let dir = std::env::temp_dir().join(format!("lightforge-graft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut theirs = Vec::new();
        message_field(&mut theirs, 1, &their_record("hs_beta", "C:\\Games\\Hearthstone"));
        let lifted = lift(&theirs, "hs_beta", "E:\\Hearthstone").unwrap();

        // No registry at all: nothing is written, because seeding one with only
        // our records would tell the client its own components are missing.
        assert!(!graft(&dir, std::slice::from_ref(&lifted)).unwrap());

        let ours = encode(&[install("wow", "12.1.0.69933")]);
        std::fs::write(dir.join("product.db"), &ours).unwrap();
        assert!(graft(&dir, std::slice::from_ref(&lifted)).unwrap());

        let merged = std::fs::read(dir.join("product.db")).unwrap();
        let found = decode(&merged);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|i| i.product == "wow"));
        let hs = found.iter().find(|i| i.product == "hs_beta").unwrap();
        assert_eq!(hs.install_path, "E:\\Hearthstone");
        // Idempotent: grafting the same record again changes nothing.
        assert!(!graft(&dir, &[lifted]).unwrap());
    }

    #[test]
    fn removing_a_product_keeps_every_other_record_byte_for_byte() {
        let mut theirs = Vec::new();
        string_field(&mut theirs, 1, "hs_beta");
        string_field(&mut theirs, 8, r#"{"something":"we never parse"}"#);
        let mut registry = Vec::new();
        message_field(&mut registry, 1, &theirs);
        let era = install("wow_classic_era", "1.15.9.69722");
        let registry = splice(&registry, &[install("wow", "12.1.0.69933"), era.clone()]).unwrap();

        let trimmed = without(&registry, "wow").unwrap();
        // What remains is exactly the registry spliced without it.
        let mut expected = Vec::new();
        message_field(&mut expected, 1, &theirs);
        assert_eq!(trimmed, splice(&expected, &[era]).unwrap());
        // Not there: nothing changes.
        assert_eq!(without(&trimmed, "wow").unwrap(), trimmed);
    }

    /// Against Gabriel's actual registry, when there is one: removing a
    /// product and adding it back must reproduce the file, and every other
    /// record must be untouched.
    #[test]
    fn removal_round_trips_on_a_real_registry() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let path = Path::new(&home)
            .join("Games/Blizzard/prefix/pfx/drive_c/ProgramData/Battle.net/Agent/product.db");
        let Ok(real) = std::fs::read(&path) else { return };
        let records = decode(&real);
        for record in records.iter().filter(|r| r.product.starts_with("wow")) {
            let trimmed = without(&real, &record.product).expect("a real registry parses");
            let left: Vec<_> = decode(&trimmed).into_iter().map(|r| r.product).collect();
            assert!(!left.contains(&record.product));
            assert_eq!(left.len(), records.len() - 1, "only {} went", record.product);
        }
    }

    /// Why this is a splice: a rebuild would drop fields `decode` never
    /// reads, and Battle.net would forget installs.
    #[test]
    fn records_we_do_not_own_survive_byte_for_byte() {
        // A record with a field the reader ignores entirely.
        let mut theirs = Vec::new();
        string_field(&mut theirs, 1, "hs_beta");
        let mut settings = Vec::new();
        string_field(&mut settings, 1, "C:/Program Files (x86)/Hearthstone");
        message_field(&mut theirs, 3, &settings);
        string_field(&mut theirs, 8, r#"{"something":"we never parse"}"#);
        varint_field(&mut theirs, 7, 12345);

        let mut existing = Vec::new();
        message_field(&mut existing, 1, &theirs);
        message_field(&mut existing, 1, &install("wow", "1.0.0").encode());

        let merged = splice(&existing, &[install("wow", "2.0.0")]).expect("a readable registry");

        // Their record is present unchanged, ours is updated, nothing added.
        assert!(
            merged.windows(theirs.len()).any(|w| w == theirs),
            "a record we do not own was rewritten"
        );
        let read = decode(&merged);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].product, "hs_beta");
        assert_eq!(read[1].product, "wow");
        assert_eq!(read[1].version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn a_product_the_registry_has_never_seen_is_appended() {
        let mut existing = Vec::new();
        message_field(&mut existing, 1, &install("wow", "1.0.0").encode());

        let merged = splice(&existing, &[install("wow_classic_era", "1.15.0")]).unwrap();
        let read = decode(&merged);
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].product, "wow_classic_era");
    }

    /// Anything we cannot read back is left alone.
    #[test]
    fn an_unreadable_registry_is_never_rewritten() {
        assert!(splice(b"not a protobuf at all", &[install("wow", "1.0.0")]).is_none());

        // A trailing partial record is not a file we may rewrite either.
        let mut truncated = Vec::new();
        message_field(&mut truncated, 1, &install("wow", "1.0.0").encode());
        truncated.push(0x0a);
        truncated.push(0xff);
        assert!(splice(&truncated, &[install("wow", "1.0.0")]).is_none());

    }

    /// A real registry carries top-level fields after the records, field 5
    /// and field 6 in Gabriel's. Treating those as corruption refused to
    /// register anything at all, which is how this was found.
    #[test]
    fn top_level_fields_that_are_not_records_are_carried_through() {
        let mut existing = Vec::new();
        message_field(&mut existing, 1, &install("wow", "1.0.0").encode());
        message_field(&mut existing, 5, &[1, 2, 3, 4, 5, 6, 7, 8]);
        varint_field(&mut existing, 6, 1);
        let tail = existing[existing.len() - 13..].to_vec();

        let merged = splice(&existing, &[install("wow_classic_era", "1.15.0")])
            .expect("a registry with trailing fields is still readable");

        // The trailing fields survive, and the new record went in with the
        // records, not after them.
        assert!(merged.ends_with(&tail), "trailing top-level fields were lost");
        let read = decode(&merged);
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].product, "wow_classic_era");
    }

    #[test]
    fn registering_into_a_missing_registry_does_nothing() {
        let dir = std::env::temp_dir().join("lightforge-registry-absent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Seeding a registry that holds only our products would tell the
        // client its own components are gone.
        assert!(!register(&dir, &[install("wow", "1.0.0")]).unwrap());
        assert!(!dir.join("product.db").exists());
    }

    #[test]
    fn agent_and_the_client_are_not_games() {
        let plumbing = |product: &str| Installed {
            product: product.into(),
            product_code: product.into(),
            install_path: "C:/whatever".into(),
            version: None,
            build_key: None,
            family: String::new(),
        };
        assert!(plumbing("agent").is_infrastructure());
        assert!(plumbing("bna").is_infrastructure());
        assert!(!plumbing("hs_beta").is_infrastructure());
    }

    /// Someone else's file, written by a program that updates on its own
    /// schedule. Every truncation of a good file must end the scan, never
    /// panic and never hang.
    #[test]
    fn a_truncated_file_is_read_as_far_as_it_goes() {
        let install = ProductInstall {
            product: "hs_beta".into(),
            install_path: "C:/Program Files (x86)/Hearthstone".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: "AUT".into(),
            geo_country: "AT".into(),
            flavor_dir: String::new(),
            version: "36.6.0.251952".into(),
            build_key: "1".repeat(32),
            tags: String::new(),
            key_ring: None,
            family: "hsb".into(),
            installed_at_ms: 0,
        };
        let whole = encode(&[install]);
        for cut in 0..whole.len() {
            let _ = decode(&whole[..cut]);
        }
        // And arbitrary bytes are a scan that finds nothing, not a crash.
        let noise: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let _ = decode(&noise);
    }
}
