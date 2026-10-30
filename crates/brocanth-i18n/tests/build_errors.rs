//! What stops the build.
//!
//! `build::compile` is what a build script calls, and a build script that
//! returns an error fails `cargo build` with the error's text. So each case
//! here is a build that does not happen, with the message the developer
//! reads.

use std::path::PathBuf;

use brocanth_i18n::build::{self, Catalog};

fn catalog(tag: &str, text: &str) -> Catalog {
    Catalog {
        tag: tag.to_string(),
        path: format!("locales/{tag}.toml"),
        text: text.to_string(),
    }
}

fn problems(catalogs: &[Catalog]) -> Vec<String> {
    match build::generate(catalogs, "en") {
        Ok(_) => panic!("the catalogs compiled"),
        Err(e) => e.problems,
    }
}

fn one_problem_containing(catalogs: &[Catalog], needle: &str) {
    let found = problems(catalogs);
    assert!(
        found.iter().any(|p| p.contains(needle)),
        "no problem mentions {needle:?}: {found:#?}"
    );
}

#[test]
fn a_key_only_a_translation_has_fails_the_build() {
    let found = problems(&[
        catalog("en", "title = \"Cart\"\n"),
        catalog("fr", "title = \"Panier\"\nsubtitle = \"Sous-titre\"\n"),
    ]);
    assert_eq!(
        found,
        [
            "locales/fr.toml:2: `subtitle` is not in the default locale `en` (locales/en.toml); \
          add it there first"
        ]
    );
}

#[test]
fn a_placeholder_a_translation_adds_fails_the_build() {
    one_problem_containing(
        &[
            catalog("en", "hi = \"Hello\"\n"),
            catalog("fr", "hi = \"Bonjour {name}\"\n"),
        ],
        "`hi` has the placeholder `{name}` and `en` does not",
    );
}

#[test]
fn a_placeholder_a_translation_drops_fails_the_build() {
    one_problem_containing(
        &[
            catalog("en", "hi = \"Hello {name}\"\n"),
            catalog("fr", "hi = \"Bonjour\"\n"),
        ],
        "`hi` lacks the placeholder `{name}` that `en` has",
    );
}

#[test]
fn a_placeholder_with_another_type_in_a_translation_fails_the_build() {
    one_problem_containing(
        &[
            catalog("en", "due = \"Due {when:date}\"\n"),
            catalog("fr", "due = \"Échéance {when}\"\n"),
        ],
        "takes `{when}` as anything Display here and as date in `en`",
    );
}

#[test]
fn a_date_length_may_differ_because_the_argument_type_does_not() {
    let ok = build::generate(
        &[
            catalog("en", "due = \"Due {when:date_long}\"\n"),
            catalog("fr", "due = \"Échéance {when:date_short}\"\n"),
        ],
        "en",
    );
    assert!(ok.is_ok(), "{:?}", ok.err());
}

#[test]
fn a_plural_in_one_locale_and_text_in_another_fails_the_build() {
    one_problem_containing(
        &[
            catalog(
                "en",
                "[items]\none = \"1 item\"\nother = \"{count} items\"\n",
            ),
            catalog("fr", "items = \"articles\"\n"),
        ],
        "`items` is plain text here and a plural in `en`",
    );
}

#[test]
fn a_plural_without_other_fails_the_build() {
    one_problem_containing(
        &[catalog("en", "[items]\none = \"1 item\"\n")],
        "the plural `items` has no `other`",
    );
}

#[test]
fn a_plural_key_cldr_does_not_define_fails_the_build() {
    one_problem_containing(
        &[catalog(
            "en",
            "[items]\nsome = \"a few\"\nother = \"{count}\"\n",
        )],
        "`some` in the plural `items` is not a CLDR category",
    );
}

#[test]
fn a_key_that_cannot_be_a_function_fails_the_build() {
    let found = problems(&[catalog(
        "en",
        "Title = \"x\"\ntype = \"y\"\n\"a-b\" = \"z\"\n",
    )]);
    assert_eq!(found.len(), 3, "{found:#?}");
    assert!(found
        .iter()
        .any(|p| p.contains("`Title` must start with a lower-case")));
    assert!(found.iter().any(|p| p.contains("`type` is a Rust keyword")));
    assert!(found.iter().any(|p| p.contains("`a-b` may hold only")));
}

#[test]
fn an_unknown_placeholder_type_and_a_stray_brace_fail_the_build() {
    let found = problems(&[catalog(
        "en",
        "a = \"{x:money}\"\nb = \"oops }\"\nc = \"{open\"\n",
    )]);
    assert_eq!(found.len(), 3, "{found:#?}");
    assert!(
        found[0].contains("the types are number, date"),
        "{found:#?}"
    );
    assert!(found[1].contains("a `}` with no `{`"), "{found:#?}");
    assert!(found[2].contains("never closes it"), "{found:#?}");
}

#[test]
fn a_missing_default_catalog_and_a_bad_file_name_fail_the_build() {
    let found = problems(&[catalog("fr", "a = \"b\"\n")]);
    assert_eq!(found, ["no catalog for the default locale `en`; found: fr"]);
    let found = problems(&[
        catalog("en", "a = \"b\"\n"),
        catalog("french", "a = \"b\"\n"),
    ]);
    assert!(
        found[0].contains("`french` is not a BCP 47 tag"),
        "{found:#?}"
    );
}

#[test]
fn every_problem_is_reported_at_once_with_its_line() {
    let found = problems(&[
        catalog("en", "a = \"A\"\n"),
        catalog("fr", "a = \"A {x}\"\n\nb = \"B\"\n"),
    ]);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(found.iter().any(|p| p.starts_with("locales/fr.toml:1:")));
    assert!(found.iter().any(|p| p.starts_with("locales/fr.toml:3:")));
}

#[test]
fn invalid_toml_names_the_line() {
    let found = problems(&[catalog("en", "a = \"A\"\nb = \n")]);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].starts_with("locales/en.toml:2:"), "{found:#?}");
}

/// The whole path a build script takes: files on disk, `OUT_DIR`, and an
/// error whose text is what `cargo build` prints.
#[test]
fn a_build_script_over_a_missing_key_fails_and_over_good_catalogs_writes_the_golden() {
    let scratch = std::env::temp_dir().join(format!("brocanth-i18n-{}", std::process::id()));
    let broken = scratch.join("broken");
    let out = scratch.join("out");
    std::fs::create_dir_all(&broken).expect("scratch");
    std::fs::create_dir_all(&out).expect("scratch");
    std::fs::write(broken.join("en.toml"), "title = \"Cart\"\n").expect("write");
    std::fs::write(broken.join("fr.toml"), "titre = \"Panier\"\n").expect("write");
    // The only test that touches the environment, and it sets the two
    // variables cargo sets for a build script.
    std::env::set_var("OUT_DIR", &out);
    std::env::set_var("CARGO_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"));

    let err = build::compile(&broken, "en").expect_err("a key only fr has");
    let text = err.to_string();
    assert!(
        text.starts_with("brocanth-i18n: 1 problem in the catalogs\n"),
        "{text}"
    );
    assert!(
        text.contains("`titre` is not in the default locale `en`"),
        "{text}"
    );
    assert!(!out.join("i18n.rs").exists(), "a failed build wrote code");

    build::compile(PathBuf::from("tests/locales"), "en").expect("the fixtures compile");
    let written = std::fs::read_to_string(out.join("i18n.rs")).expect("written");
    let golden = include_str!("golden/i18n.rs");
    assert_eq!(written, golden);
    let _ = std::fs::remove_dir_all(&scratch);
}
