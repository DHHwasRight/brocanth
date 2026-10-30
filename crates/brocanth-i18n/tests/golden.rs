//! The generated code, checked in.
//!
//! `tests/golden/i18n.rs` is what `build::compile` writes for
//! `tests/locales`. The other tests `include!` it, so they run the code an
//! application would get. This test fails when the generator changes what it
//! writes; `BROCANTH_BLESS=1 cargo test -p brocanth-i18n --test golden`
//! accepts the change, and the diff shows it.

use std::path::Path;

use brocanth_i18n::build;

#[test]
fn the_generator_writes_what_it_wrote_yesterday() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalogs = build::read_dir(&root.join("tests/locales")).expect("the fixtures");
    let generated = build::generate(&catalogs, "en").expect("the fixtures compile");
    let file = root.join("tests/golden/i18n.rs");
    if std::env::var_os("BROCANTH_BLESS").is_some() {
        std::fs::write(&file, &generated.code).expect("write the golden");
        return;
    }
    let want = std::fs::read_to_string(&file).expect("tests/golden/i18n.rs");
    assert!(
        want == generated.code,
        "the generated code changed; rerun with BROCANTH_BLESS=1 and read the diff\n{}",
        generated.code
    );
}

#[test]
fn a_key_a_translation_lacks_is_a_warning_naming_where_it_falls_back_to() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalogs = build::read_dir(&root.join("tests/locales")).expect("the fixtures");
    let generated = build::generate(&catalogs, "en").expect("the fixtures compile");
    let fr_ca_title = generated.warnings.iter().any(|w| w.contains("`title`"));
    assert!(
        !fr_ca_title,
        "fr-CA has its own title: {:#?}",
        generated.warnings
    );
    assert!(
        generated
            .warnings
            .iter()
            .any(|w| w.contains("fr.toml") && w.contains("`nav.back`") && w.ends_with("`en`")),
        "{:#?}",
        generated.warnings
    );
    assert!(
        generated
            .warnings
            .iter()
            .any(|w| w.contains("fr-CA.toml") && w.contains("`greeting`") && w.ends_with("`fr`")),
        "{:#?}",
        generated.warnings
    );
}
