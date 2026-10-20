//! Every vocabulary item accepts and rejects, every issue lands at the right
//! path, and the description covers the whole vocabulary.

use brocanth_schema::prelude::*;
use brocanth_schema::{Date, ObjectSchema, SchemaNode, UnknownKeys};

// ---------------------------------------------------------------------------
// Accepts and rejects
// ---------------------------------------------------------------------------

#[test]
fn every_string_constraint_accepts_and_rejects() {
    assert!(string().min_length(3).validate("abc").is_ok());
    assert!(string().min_length(3).validate("ab").is_err());

    assert!(string().max_length(3).validate("abc").is_ok());
    assert!(string().max_length(3).validate("abcd").is_err());

    assert!(string().length(3).validate("abc").is_ok());
    assert!(string().length(3).validate("ab").is_err());

    assert!(string().non_empty().validate("a").is_ok());
    assert!(string().non_empty().validate("").is_err());

    assert!(string().pattern("[a-z]+").validate("abc").is_ok());
    assert!(string().pattern("[a-z]+").validate("ab1").is_err());

    assert!(string().email().validate("a@b.co").is_ok());
    assert!(string().email().validate("a@b").is_err());

    assert!(string().url().validate("https://b.co").is_ok());
    assert!(string().url().validate("b.co").is_err());

    assert!(string()
        .uuid()
        .validate("3f2504e0-4f89-11d3-9a0c-0305e82c3301")
        .is_ok());
    assert!(string().uuid().validate("nope").is_err());

    assert!(string().starts_with("a").validate("abc").is_ok());
    assert!(string().starts_with("a").validate("bbc").is_err());

    assert!(string().ends_with("c").validate("abc").is_ok());
    assert!(string().ends_with("c").validate("abd").is_err());

    assert!(string().contains("b").validate("abc").is_ok());
    assert!(string().contains("z").validate("abc").is_err());
}

#[test]
fn every_number_constraint_accepts_and_rejects() {
    assert!(int().min(3).validate(&3).is_ok());
    assert!(int().min(3).validate(&2).is_err());

    assert!(int().max(3).validate(&3).is_ok());
    assert!(int().max(3).validate(&4).is_err());

    assert!(int().exclusive_min(3).validate(&4).is_ok());
    assert!(int().exclusive_min(3).validate(&3).is_err());

    assert!(int().exclusive_max(3).validate(&2).is_ok());
    assert!(int().exclusive_max(3).validate(&3).is_err());

    assert!(int().multiple_of(3).validate(&9).is_ok());
    assert!(int().multiple_of(3).validate(&10).is_err());

    assert!(int().positive().validate(&1).is_ok());
    assert!(int().positive().validate(&0).is_err());

    assert!(int().non_negative().validate(&0).is_ok());
    assert!(int().non_negative().validate(&-1).is_err());

    assert!(float().integer().validate(&2.0).is_ok());
    assert!(float().integer().validate(&2.5).is_err());

    assert!(float().finite().validate(&2.5).is_ok());
    assert!(float().finite().validate(&f64::INFINITY).is_err());
}

#[test]
fn booleans_enums_and_literals_accept_and_reject() {
    assert!(boolean().validate(&true).is_ok());

    assert!(one_of(["todo", "done"]).validate("done").is_ok());
    assert!(one_of(["todo", "done"]).validate("maybe").is_err());

    assert!(literal("todo").validate(&Value::text("todo")).is_ok());
    assert!(literal("todo").validate(&Value::text("done")).is_err());
    assert!(literal(3i64).validate(&Value::Int(3)).is_ok());
    assert!(literal(3i64).validate(&Value::Int(4)).is_err());
}

#[test]
fn dates_and_times_accept_and_reject() {
    let bounded = date().min(Date::new(2020, 1, 1).unwrap());
    assert!(bounded.validate("2020-01-01").is_ok());
    assert!(bounded.validate("2019-12-31").is_err());
    assert!(bounded.validate("not-a-date").is_err());

    assert!(time().validate("23:59:59").is_ok());
    assert!(time().validate("24:00:00").is_err());

    assert!(datetime().validate("2026-10-12T09:00:00Z").is_ok());
    assert!(datetime().validate("2026-10-12").is_err());
}

#[test]
fn every_array_constraint_accepts_and_rejects() {
    let one = [Value::text("a")];
    let two = [Value::text("a"), Value::text("b")];
    let twice = [Value::text("a"), Value::text("a")];

    assert!(array(string()).min_items(2).validate(&two).is_ok());
    assert!(array(string()).min_items(2).validate(&one).is_err());

    assert!(array(string()).max_items(1).validate(&one).is_ok());
    assert!(array(string()).max_items(1).validate(&two).is_err());

    assert!(array(string()).length(2).validate(&two).is_ok());
    assert!(array(string()).length(2).validate(&one).is_err());

    assert!(array(string()).non_empty().validate(&one).is_ok());
    assert!(array(string()).non_empty().validate(&[]).is_err());

    assert!(array(string()).unique().validate(&two).is_ok());
    assert!(array(string()).unique().validate(&twice).is_err());
}

#[test]
fn the_unknown_key_policy_is_all_three() {
    let value = Value::object([("a", Value::text("x")), ("b", Value::text("y"))]);
    let base = || object().field("a", string());

    assert!(base().validate(&value).is_err());
    assert!(base().unknown_keys_ignored().validate(&value).is_ok());
    assert!(base()
        .unknown_keys_passed_through()
        .validate(&value)
        .is_ok());
    assert!(base()
        .unknown_keys(UnknownKeys::Reject)
        .validate(&value)
        .is_err());
}

#[test]
fn optional_nullable_and_default_differ_where_they_should() {
    let optional = object().field("a", string().optional());
    let nullable = object().field("a", string().nullable());
    let defaulted = object().field("a", string().with_default("x"));

    let absent = Value::Object(Vec::new());
    let null = Value::object([("a", Value::Null)]);

    assert!(optional.validate(&absent).is_ok());
    assert!(defaulted.validate(&absent).is_ok());
    assert!(
        nullable.validate(&absent).is_err(),
        "nullable means the key is there and its value may be null"
    );

    assert!(optional.validate(&null).is_ok());
    assert!(nullable.validate(&null).is_ok());
    assert!(defaulted.validate(&null).is_ok());
}

#[test]
fn union_takes_any_branch_and_intersection_takes_all() {
    let either = union().or(int().min(1)).or(string().uuid());
    assert!(either.validate(&Value::Int(7)).is_ok());
    assert!(either
        .validate(&Value::text("3f2504e0-4f89-11d3-9a0c-0305e82c3301"))
        .is_ok());
    assert!(either.validate(&Value::Int(0)).is_err());
    assert!(either.validate(&Value::text("nope")).is_err());

    let both = intersection()
        .and(object().field("a", string()).unknown_keys_ignored())
        .and(object().field("b", int()).unknown_keys_ignored());
    let ok = Value::object([("a", Value::text("x")), ("b", Value::Int(1))]);
    let half = Value::object([("a", Value::text("x"))]);
    assert!(both.validate(&ok).is_ok());
    assert!(both.validate(&half).is_err());
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

#[test]
fn refine_and_transform_compose() {
    let slug = string()
        .min_length(3)
        .refine("no_spaces", "must not contain a space", |s: &str| {
            !s.contains(' ')
        })
        .transform(|s: String| s.to_lowercase());

    assert_eq!(slug.parse("Hello").unwrap(), "hello");

    let refused = slug.parse("a b").unwrap_err();
    assert_eq!(refused.messages_at(""), vec!["must not contain a space"]);
    assert_eq!(refused.iter().next().unwrap().code.as_str(), "no_spaces");

    // The predicate does not run when the inner schema already failed, so a
    // value that is not even a slug reports one issue, not two.
    let too_short = slug.parse("a").unwrap_err();
    assert_eq!(too_short.len(), 1);
    assert_eq!(too_short.iter().next().unwrap().code, IssueCode::TooShort);
}

#[test]
fn transform_chains() {
    let length = string()
        .non_empty()
        .transform(|s: String| s.len())
        .transform(|n: usize| n * 2);
    assert_eq!(length.parse("abc").unwrap(), 6);
    assert!(length.parse("").is_err());
}

#[test]
fn refine_survives_a_transform_in_front_of_it() {
    let schema =
        int()
            .min(0)
            .transform(|n: i64| n.to_string())
            .refine("even", "must be even", |n: &i64| n % 2 == 0);

    assert_eq!(schema.parse(&4).unwrap(), "4");
    assert!(schema.parse(&3).is_err());
}

// ---------------------------------------------------------------------------
// Every issue, at the right place
// ---------------------------------------------------------------------------

#[test]
fn all_issues_are_reported_not_the_first() {
    let schema = object()
        .field("name", string().min_length(2).max_length(3))
        .field("age", int().min(18).multiple_of(5))
        .field("tags", array(string().non_empty()).min_items(3));

    let value = Value::object([
        ("name", Value::text("")),
        ("age", Value::Int(7)),
        ("tags", Value::array([Value::text(""), Value::text("")])),
    ]);

    let issues = schema.validate(&value).unwrap_err();

    assert_eq!(
        issues.paths(),
        vec!["name", "age", "tags", "tags[0]", "tags[1]"]
    );
    assert_eq!(issues.len(), 6, "{}", issues);
    assert!(issues.has_code("age", &IssueCode::TooSmall));
    assert!(issues.has_code("age", &IssueCode::NotMultipleOf));
}

#[test]
fn a_path_survives_three_levels_of_nesting() {
    let schema = object().field(
        "boards",
        array(object().field(
            "notes",
            array(object().field("kind", one_of(["todo", "done"]))),
        )),
    );

    let value = Value::object([(
        "boards",
        Value::array([Value::object([(
            "notes",
            Value::array([
                Value::object([("kind", Value::text("todo"))]),
                Value::object([("kind", Value::text("maybe"))]),
            ]),
        )])]),
    )]);

    let issues = schema.validate(&value).unwrap_err();
    assert_eq!(issues.paths(), vec!["boards[0].notes[1].kind"]);
}

#[test]
fn the_two_renderings_say_the_same_thing_differently() {
    let schema = object()
        .field("email", string().email())
        .field("age", int().min(18));
    let value = Value::object([("email", Value::text("nope")), ("age", Value::Int(7))]);
    let issues = schema.validate(&value).unwrap_err();

    assert_eq!(
        issues.form_report(),
        vec![
            ("email", vec!["must be an email address"]),
            ("age", vec!["must be at least 18"]),
        ]
    );

    let json = issues.to_json();
    assert!(json.starts_with(r#"{"error":"validation_failed","issues":["#));
    assert!(json.contains(r#""path":"email""#));
    assert!(json.contains(r#""code":"format""#));
    assert!(json.contains(r#""expected":"an email address""#));
    assert!(json.contains(r#""received":"\"nope\"""#));
    assert!(json.contains(r#""code":"too_small""#));
}

// ---------------------------------------------------------------------------
// The description covers the vocabulary
// ---------------------------------------------------------------------------

/// One schema holding every vocabulary item, so the walk below can prove the
/// description reaches all of them.
fn everything() -> ObjectSchema {
    object()
        .described_field("s", string().min_length(1), Some("some text"))
        .field("i", int().min(1))
        .field("f", float().min(1.0))
        .field("b", boolean())
        .field("d", date())
        .field("t", time())
        .field("dt", datetime())
        .field("e", one_of(["a", "b"]))
        .field("lit", literal("x"))
        .field("arr", array(string()).unique())
        .field("obj", object().field("inner", string()))
        .field("opt", string().optional())
        .field("nul", string().nullable())
        .field("def", string().with_default("x"))
        .field("uni", union().or(string()).or(int()))
        .field("inter", intersection().and(object().unknown_keys_ignored()))
        .field(
            "ref",
            string().refine("not_blank", "must not be blank", |s: &str| !s.is_empty()),
        )
        .field("trans", string().transform(|s: String| s.len()))
}

#[test]
fn the_description_reaches_every_vocabulary_item() {
    let mut kinds = Vec::new();
    everything().describe().walk(&mut |node| {
        let kind = node.kind();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    });
    kinds.sort_unstable();

    let mut expected = vec![
        "string",
        "integer",
        "number",
        "boolean",
        "date",
        "time",
        "date-time",
        "enum",
        "literal",
        "array",
        "object",
        "optional",
        "nullable",
        "default",
        "union",
        "intersection",
        "refined",
        "transformed",
    ];
    expected.sort_unstable();

    assert_eq!(kinds, expected);
}

#[test]
fn the_description_carries_what_a_consumer_needs() {
    let SchemaNode::Object(object) = everything().describe() else {
        panic!("an object schema describes itself as an object");
    };

    assert_eq!(object.unknown_keys, UnknownKeys::Reject);
    assert_eq!(
        object.field("s").unwrap().description.as_deref(),
        Some("some text")
    );

    // Required-ness is derived from the schema, once.
    assert!(object.field("s").unwrap().required);
    assert!(!object.field("opt").unwrap().required);
    assert!(!object.field("def").unwrap().required);
    assert!(
        object.field("nul").unwrap().required,
        "a nullable key must be present"
    );
    assert!(object.required().contains(&"nul"));

    // The constraints survive the trip.
    let SchemaNode::String(node) = object.field("s").unwrap().schema.clone() else {
        panic!("a string field describes itself as a string");
    };
    assert_eq!(node.min_length, Some(1));

    let SchemaNode::Array(array) = object.field("arr").unwrap().schema.clone() else {
        panic!("an array field describes itself as an array");
    };
    assert!(array.unique);
    assert_eq!(array.items.kind(), "string");

    // `unwrapped` reaches the widget a form builder has to draw.
    assert_eq!(
        object.field("opt").unwrap().schema.unwrapped().kind(),
        "string"
    );
    assert_eq!(
        object.field("ref").unwrap().schema.unwrapped().kind(),
        "string"
    );
}

#[test]
fn describing_twice_gives_the_same_description() {
    assert_eq!(everything().describe(), everything().describe());
}
