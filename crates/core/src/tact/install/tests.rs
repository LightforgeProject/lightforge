use super::*;
use crate::tact::manifest::Tag;

#[test]
fn the_tag_column_matches_what_a_real_build_info_carries() {
    let options = Options { region: Region::Eu, locale: "enUS".into(), ..Options::default() };
    assert_eq!(
        options.tag_column(),
        "Windows x86_64 EU enUS speech:Windows x86_64 EU enUS text"
    );
}

fn locale_tag(name: &str) -> Tag {
    Tag::named(name, LOCALE_TAG)
}

#[test]
fn a_saved_language_that_is_no_longer_offered_reads_as_its_replacement() {
    assert_eq!(current_locale("enGB"), "enUS");
    assert_eq!(current_locale("ptPT"), "ptBR");
    assert_eq!(current_locale("deDE"), "deDE");
    assert!(LOCALES.iter().all(|(code, _)| current_locale(code) == *code));
}

#[test]
fn a_language_with_no_data_pack_reads_the_nearest_one() {
    let tags = [locale_tag("enUS"), locale_tag("deDE"), locale_tag("ptBR")];
    assert_eq!(shipped_locale("deDE", &tags), "deDE");
    assert_eq!(shipped_locale("enGB", &tags), "enUS");
    assert_eq!(shipped_locale("ptPT", &tags), "ptBR");
    // Nothing to choose between: left as asked.
    assert_eq!(shipped_locale("enGB", &[]), "enGB");
}

#[test]
fn any_limit_option_marks_the_run_as_deliberately_partial() {
    assert!(!Options::default().limited());
    assert!(Options { skip_loose: true, ..Options::default() }.limited());
    assert!(Options { limit_runs: Some(1), ..Options::default() }.limited());
    assert!(Options { limit_archives: Some(1), ..Options::default() }.limited());
}
