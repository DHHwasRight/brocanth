//! `#[derive(Validate)]`: the whole grammar, the paths it produces, and the
//! description it writes.

use brocanth_schema::{
    IssueCode, Issues, Path, SchemaNode, StringFormat, UnknownKeys, Validate,
};

// ---------------------------------------------------------------------------
// The worked example
// ---------------------------------------------------------------------------

fn is_not_reserved(name: &str) -> bool {
    !matches!(name, "admin" | "root")
}

#[derive(Validate)]
#[validate(unknown_keys = "reject")]
struct SignUp {
    #[validate(
        string(min_length = 3, max_length = 32, pattern = "^[a-z0-9_]+$"),
        refine(is_not_reserved, code = "reserved", message = "is already taken"),
        describe = "Lowercase letters, digits and underscores."
    )]
    username: String,

    #[validate(string(email))]
    email: String,

    #[validate(int(min = 13, max = 130))]
    age: i64,

    #[validate(string(url))]
    website: Option<String>,

    #[validate(bool)]
    terms: bool,

    #[validate(date(min = "1800-01-01"))]
    born_on: String,

    #[validate(array(min_items = 1, max_items = 3, unique), each(string(non_empty)))]
    tags: Vec<String>,

    #[validate(each(nested))]
    notes: Vec<Note>,

    #[validate(nested)]
    address: Address,

    #[validate(rename = "ref")]
    referral: Option<String>,

    #[validate(skip)]
    internal: Vec<u8>,
}

#[derive(Validate)]
struct Note {
    #[validate(one_of("todo", "doing", "done"))]
    kind: String,
    #[validate(string(max_length = 140))]
    body: String,
}

#[derive(Validate)]
struct Address {
    #[validate(string(length = 2))]
    country: String,
    #[validate(float(min = -180.0, max = 180.0))]
    longitude: f64,
}

fn good() -> SignUp {
    SignUp {
        username: "ada_l".into(),
        email: "ada@example.com".into(),
        age: 36,
        website: Some("https://example.com".into()),
        terms: true,
        born_on: "1815-12-10".into(),
        tags: vec!["maths".into()],
        notes: vec![Note {
            kind: "todo".into(),
            body: "write the notes".into(),
        }],
        address: Address {
            country: "GB".into(),
            longitude: 0.0,
        },
        referral: None,
        internal: vec![1, 2, 3],
    }
}

#[test]
fn a_good_value_passes() {
    let value = good();
    assert!(value.validate().is_ok());
    // A field with no constraint and a field that is skipped are both still
    // ordinary fields of the struct.
    assert_eq!(value.referral, None);
    assert_eq!(value.internal.len(), 3);
}

#[test]
fn every_bad_field_is_reported_at_its_own_path() {
    let bad = SignUp {
        username: "AB".into(),
        email: "nope".into(),
        age: 9,
        website: Some("example.com".into()),
        born_on: "1815-13-10".into(),
        tags: vec![String::new(), "a".into(), "a".into(), "b".into()],
        notes: vec![
            Note {
                kind: "todo".into(),
                body: "fine".into(),
            },
            Note {
                kind: "maybe".into(),
                body: "x".repeat(200),
            },
        ],
        address: Address {
            country: "GBR".into(),
            longitude: 200.0,
        },
        ..good()
    };

    let issues = bad.validate().unwrap_err();

    assert_eq!(
        issues.paths(),
        vec![
            "username",
            "email",
            "age",
            "website",
            "born_on",
            "tags",
            "tags[0]",
            "tags[2]",
            "notes[1].kind",
            "notes[1].body",
            "address.country",
            "address.longitude",
        ],
        "{issues}"
    );

    // Three constraints on one field, all three reported.
    assert_eq!(
        issues.messages_at("username"),
        vec![
            "must be at least 3 characters",
            "must match ^[a-z0-9_]+$",
        ]
    );
    assert!(issues.has_code("tags", &IssueCode::TooLong));
    assert!(issues.has_code("tags[2]", &IssueCode::NotUnique));
    assert!(issues.has_code("born_on", &IssueCode::Format));
}

#[test]
fn a_refine_on_a_struct_field_reports_its_own_code() {
    let bad = SignUp {
        username: "admin".into(),
        ..good()
    };
    let issues = bad.validate().unwrap_err();
    assert_eq!(issues.messages_at("username"), vec!["is already taken"]);
    assert_eq!(
        issues.iter().next().unwrap().code,
        IssueCode::Refused("reserved".into())
    );
}

#[test]
fn an_absent_option_is_not_checked_and_a_present_one_is() {
    let absent = SignUp {
        website: None,
        ..good()
    };
    assert!(absent.validate().is_ok());

    let present = SignUp {
        website: Some("not a url".into()),
        ..good()
    };
    assert!(present.validate().unwrap_err().has("website"));
}

// ---------------------------------------------------------------------------
// The description the derive writes
// ---------------------------------------------------------------------------

#[test]
fn the_derive_describes_every_field_it_did_not_skip() {
    let SchemaNode::Object(object) = SignUp::schema() else {
        panic!("a derived schema is an object");
    };

    assert_eq!(
        object
            .fields
            .iter()
            .map(|f| f.name.as_ref())
            .collect::<Vec<_>>(),
        vec![
            "username", "email", "age", "website", "terms", "born_on", "tags", "notes",
            "address", "ref",
        ],
        "`skip` leaves a field out and `rename` changes its key"
    );
    assert_eq!(object.unknown_keys, UnknownKeys::Reject);

    // An `Option<T>` field is optional unless it says `required`.
    assert!(!object.field("website").unwrap().required);
    assert!(object.field("email").unwrap().required);
    assert_eq!(object.required().len(), 8);

    // A refine wraps, and keeps its message as the description.
    let username = &object.field("username").unwrap().schema;
    let SchemaNode::Refined { code, .. } = username else {
        panic!("a refined field describes itself as refined");
    };
    assert_eq!(code, "reserved");
    assert_eq!(username.unwrapped().kind(), "string");
    assert_eq!(
        object.field("username").unwrap().description.as_deref(),
        Some("Lowercase letters, digits and underscores.")
    );

    // Constraints survive.
    let SchemaNode::String(email) = object.field("email").unwrap().schema.clone() else {
        panic!("a string field describes itself as a string");
    };
    assert_eq!(email.format, Some(StringFormat::Email));

    let SchemaNode::Array(tags) = object.field("tags").unwrap().schema.clone() else {
        panic!("an array field describes itself as an array");
    };
    assert_eq!(tags.min_items, Some(1));
    assert_eq!(tags.max_items, Some(3));
    assert!(tags.unique);
    assert_eq!(tags.items.kind(), "string");

    // A nested struct's description is its own.
    let SchemaNode::Object(address) = object.field("address").unwrap().schema.clone() else {
        panic!("a nested field describes itself as an object");
    };
    assert_eq!(address.fields.len(), 2);

    // `each(nested)` reaches the element type's description.
    let SchemaNode::Array(notes) = object.field("notes").unwrap().schema.clone() else {
        panic!("a sequence of nested structs describes itself as an array");
    };
    assert_eq!(notes.items.kind(), "object");

    // A field with no constraints still has a description.
    let SchemaNode::Optional(referral) = object.field("ref").unwrap().schema.clone() else {
        panic!("an Option field describes itself as optional");
    };
    assert_eq!(referral.kind(), "string");
}

// ---------------------------------------------------------------------------
// The rest of the grammar
// ---------------------------------------------------------------------------

#[derive(Validate)]
#[validate(unknown_keys = "ignore")]
struct Loose {
    #[validate(string(starts_with = "x", ends_with = "z", contains = "y"), required)]
    code: Option<String>,

    #[validate(int(exclusive_min = 0, exclusive_max = 10, multiple_of = 2))]
    small: u8,

    #[validate(time(min = "09:00", max = "17:00:00"))]
    at: String,

    #[validate(datetime(max = "2030-01-01T00:00:00Z"))]
    when: String,

    #[validate(string(uuid))]
    id: Box<str>,
}

#[test]
fn the_rest_of_the_grammar_works() {
    let good = Loose {
        code: Some("xyz".into()),
        small: 2,
        at: "09:30".into(),
        when: "2026-10-12T09:00:00Z".into(),
        id: "3f2504e0-4f89-11d3-9a0c-0305e82c3301".into(),
    };
    assert!(good.validate().is_ok());

    let bad = Loose {
        code: None,
        small: 11,
        at: "18:00".into(),
        when: "2031-01-01T00:00:00Z".into(),
        id: "nope".into(),
    };
    let issues = bad.validate().unwrap_err();
    assert_eq!(issues.paths(), vec!["code", "small", "at", "when", "id"]);
    assert!(issues.has_code("code", &IssueCode::Missing));
    assert_eq!(issues.messages_at("code"), vec!["is required"]);

    let SchemaNode::Object(object) = Loose::schema() else {
        panic!("a derived schema is an object");
    };
    assert_eq!(object.unknown_keys, UnknownKeys::Ignore);
    assert!(
        object.field("code").unwrap().required,
        "`required` makes an Option field required"
    );
}

// ---------------------------------------------------------------------------
// Hand-written and derived impls meet
// ---------------------------------------------------------------------------

struct Hand(String);

impl Validate for Hand {
    fn schema() -> SchemaNode {
        SchemaNode::String(Default::default())
    }

    fn validate_at(&self, at: &mut Path, issues: &mut Issues) {
        if self.0.is_empty() {
            issues.report(at, IssueCode::TooShort, |i| {
                i.expected("something").received("nothing")
            });
        }
    }
}

#[derive(Validate)]
struct Mixed {
    #[validate(nested)]
    hand: Hand,
}

#[test]
fn a_hand_written_impl_nests_like_any_other() {
    let issues = Mixed {
        hand: Hand(String::new()),
    }
    .validate()
    .unwrap_err();
    assert_eq!(issues.paths(), vec!["hand"]);
}
