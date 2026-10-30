//! A translation in an rsx page and in a jt document.
//!
//! Neither engine knows this crate exists. A key is a function returning a
//! `String`, and both engines take an expression: rsx writes it through
//! `Display` with HTML escaping, jt writes it as a JSON string.

#[allow(dead_code)]
mod i18n {
    include!("golden/i18n.rs");
}

mod views {
    rsx::load!("tests/views/cart.rsx");
}

use i18n::Locale;
use rsx::Template;

#[allow(non_snake_case)]
mod json {
    use crate::i18n::{t, Locale};

    jt::template! {
        pub fn Cart(locale: Locale, count: u64) -> Json {
            "locale": jt::Display(locale),
            "title": t::title(locale),
            "items": t::cart_items(locale, count),
            "total": t::total(locale, 1234),
        }
    }
}

#[test]
fn an_rsx_page_renders_each_locale_from_the_same_template() {
    let en = views::Cart {
        locale: Locale::En,
        count: 0,
        name: "Ada",
    }
    .render_to_string()
    .expect("renders");
    assert!(en.contains(r#"<section lang="en">"#), "{en}");
    assert!(en.contains("<h1>Your cart</h1>"), "{en}");
    assert!(en.contains("<p>Hello, Ada!</p>"), "{en}");
    assert!(
        en.contains(r#"<p class="count">Your cart is empty</p>"#),
        "{en}"
    );

    let fr = views::Cart {
        locale: Locale::Fr,
        count: 3,
        name: "Ada",
    }
    .render_to_string()
    .expect("renders");
    assert!(fr.contains(r#"<section lang="fr">"#), "{fr}");
    assert!(fr.contains("<h1>Votre panier</h1>"), "{fr}");
    assert!(fr.contains(r#"<p class="count">3 articles</p>"#), "{fr}");
}

#[test]
fn a_value_inside_a_translation_is_escaped_by_the_template() {
    let html = views::Cart {
        locale: Locale::En,
        count: 1,
        name: "<script>",
    }
    .render_to_string()
    .expect("renders");
    assert!(html.contains("Hello, &lt;script&gt;!"), "{html}");
}

#[test]
fn a_jt_document_carries_the_translation_as_a_json_string() {
    let en = jt::to_string(&json::Cart(Locale::En, 1));
    assert_eq!(
        en,
        r#"{"locale":"en","title":"Your cart","items":"1 item","total":"Total: 1,234"}"#
    );
    let fr = jt::to_string(&json::Cart(Locale::FrCa, 2));
    assert_eq!(
        fr,
        "{\"locale\":\"fr-CA\",\"title\":\"Votre panier d’achats\",\"items\":\"2 articles\",\
         \"total\":\"Total : 1\u{202f}234\"}"
    );
}
