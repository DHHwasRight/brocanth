//! The generated catalog, called the way a handler calls it.

mod i18n {
    include!("golden/i18n.rs");
}

use brocanth_i18n::{Date, Locales};
use i18n::{t, Locale};

#[test]
fn every_locale_has_a_variant_and_the_default_is_first() {
    assert_eq!(Locale::ALL, &[Locale::En, Locale::Fr, Locale::FrCa]);
    assert_eq!(Locale::DEFAULT, Locale::En);
    assert_eq!(Locale::FrCa.parent(), Some(Locale::Fr));
    assert_eq!(Locale::Fr.parent(), None);
    assert_eq!(Locale::FrCa.to_string(), "fr-CA");
}

#[test]
fn a_plain_message_is_the_locales_own_text() {
    assert_eq!(t::title(Locale::En), "Your cart");
    assert_eq!(t::title(Locale::Fr), "Votre panier");
    assert_eq!(t::title(Locale::FrCa), "Votre panier d’achats");
}

#[test]
fn a_regional_locale_falls_back_to_its_language_then_to_the_default() {
    // fr-CA has no `greeting`; fr does.
    assert_eq!(t::greeting(Locale::FrCa, "Max"), "Bonjour, Max !");
    // Neither has `nav.back`; the default does.
    assert_eq!(t::nav::back(Locale::FrCa, "Home"), "Back to Home");
    assert_eq!(t::nav::back(Locale::Fr, "Home"), "Back to Home");
    // A namespace is a module, and falls back the same way.
    assert_eq!(t::nav::home(Locale::FrCa), "Accueil");
}

#[test]
fn english_plurals_follow_cldr_and_the_exact_zero_wins() {
    assert_eq!(t::cart_items(Locale::En, 0), "Your cart is empty");
    assert_eq!(t::cart_items(Locale::En, 1), "1 item");
    assert_eq!(t::cart_items(Locale::En, 2), "2 items");
    assert_eq!(t::cart_items(Locale::En, 1234), "1,234 items");
}

#[test]
fn french_zero_is_singular_and_a_million_is_many() {
    // No `=0` in French, so CLDR decides: 0 is `one` in French.
    assert_eq!(t::cart_items(Locale::Fr, 0), "0 article");
    assert_eq!(t::cart_items(Locale::Fr, 1), "1 article");
    assert_eq!(t::cart_items(Locale::Fr, 2), "2 articles");
    assert_eq!(
        t::cart_items(Locale::Fr, 1_000_000),
        "1\u{202f}000\u{202f}000 d’articles"
    );
    // fr-CA takes the French plural, with French rules.
    assert_eq!(t::cart_items(Locale::FrCa, 0), "0 article");
}

#[test]
fn a_number_placeholder_is_grouped_by_the_locale() {
    assert_eq!(t::total(Locale::En, 1_234_567), "Total: 1,234,567");
    assert_eq!(
        t::total(Locale::Fr, 1_234_567),
        "Total : 1\u{202f}234\u{202f}567"
    );
}

#[test]
fn a_message_with_two_arguments_takes_them_by_name() {
    let day = Date::new(2026, 10, 30).expect("a real day");
    let args = t::Paid { name: &"Ada", day };
    assert_eq!(t::paid(Locale::En, args), "Ada paid on October 30, 2026");
    // The French text asked for a medium date; the call site is the same.
    assert_eq!(t::paid(Locale::Fr, args), "Ada a payé le 30 oct. 2026");
}

#[test]
fn doubled_braces_are_literal_braces() {
    assert_eq!(t::braces(Locale::En), "Write {name} for a placeholder");
}

#[test]
fn formats_are_built_once_per_locale() {
    let a: *const _ = Locale::Fr.formats();
    let b: *const _ = Locale::Fr.formats();
    assert_eq!(a, b);
    assert_eq!(Locale::FrCa.formats().tag(), "fr-CA");
}
