//! A value the URL helpers encode reads back, from the route table, as the
//! value they were given.

use brocanth::routing::{encode_catch_all, encode_segment, percent_decode, Router, Verb};

fn table() -> Router<&'static str> {
    let mut builder = Router::builder();
    builder.route("/files/{name}", Verb::Get, "one").expect("path");
    builder.route("/tree/{*rest}", Verb::Get, "rest").expect("path");
    builder.build().expect("table")
}

#[test]
fn an_encoded_segment_reads_back_as_it_was_given() {
    let table = table();
    for given in ["a/b", "café", "100% sure", "x y?z#", "plain"] {
        let path = format!("/files/{}", encode_segment(given));
        let matched = table.at(&path).expect("the route matches");
        assert_eq!(matched.params().get("name"), Some(given), "{path}");
    }
}

#[test]
fn an_encoded_catch_all_reads_back_as_it_was_given() {
    let table = table();
    let given = "docs/a b/ü.txt";
    let path = format!("/tree/{}", encode_catch_all(given));
    let matched = table.at(&path).expect("the route matches");
    assert_eq!(matched.params().get("rest"), Some(given));
}

#[test]
fn a_value_that_cannot_be_decoded_is_kept_as_it_arrived() {
    assert_eq!(percent_decode("50%"), "50%");
    assert_eq!(percent_decode("%zz"), "%zz");
    // Not UTF-8 once decoded.
    assert_eq!(percent_decode("%FF"), "%FF");
    assert_eq!(percent_decode("%41"), "A");
}
