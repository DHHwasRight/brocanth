//! Widgets per column type, and the rule that a rejected form keeps what was
//! typed.

use std::sync::Arc;

use brocanth_admin::form::{widget_for, Choice, Form, Submission, Widget};
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{
    Admin, AdminConfig, ColumnDescriptor, ColumnKind, Discovery, Fixture, ModelAdmin,
    ModelDescriptor, Principal, Row,
};

fn column(kind: ColumnKind) -> ColumnDescriptor {
    ColumnDescriptor::new("field", kind)
}

#[test]
fn every_column_type_has_a_widget() {
    let expected = [
        (ColumnKind::Text, Widget::Text),
        (ColumnKind::LongText, Widget::Textarea),
        (ColumnKind::Integer, Widget::Number),
        (ColumnKind::Float, Widget::Number),
        (ColumnKind::Decimal, Widget::Number),
        (ColumnKind::Boolean, Widget::Checkbox),
        (ColumnKind::Date, Widget::Date),
        (ColumnKind::Time, Widget::Time),
        (ColumnKind::DateTime, Widget::DateTime),
        (ColumnKind::Uuid, Widget::Text),
        (ColumnKind::Json, Widget::Json),
        (ColumnKind::Binary, Widget::ReadOnly),
        (ColumnKind::Enumeration, Widget::Select),
        (ColumnKind::Unknown, Widget::Text),
    ];

    for (kind, widget) in expected {
        assert_eq!(widget_for(&column(kind)), widget, "for {kind:?}");
    }

    // And the table above covers the enum, so a new column type cannot be
    // added without this test noticing.
    assert_eq!(expected.len(), ColumnKind::ALL.len());
    for kind in ColumnKind::ALL {
        assert!(expected.iter().any(|(k, _)| k == kind), "missing {kind:?}");
    }
}

#[test]
fn a_generated_key_is_hidden_and_a_foreign_key_is_a_picker() {
    let key = ColumnDescriptor::new("id", ColumnKind::Integer).primary_key();
    assert_eq!(widget_for(&key), Widget::Hidden);

    // An integer column that points at another table must never get a number
    // input, whatever its type says.
    let fk = ColumnDescriptor::new("author_id", ColumnKind::Integer).references("user", "id");
    assert_eq!(widget_for(&fk), Widget::ForeignKey);

    let role = ColumnDescriptor::new("role", ColumnKind::Text).variants(["author", "editor"]);
    assert_eq!(widget_for(&role), Widget::Select);
}

#[test]
fn seaorm_type_spellings_fold_onto_kinds() {
    assert_eq!(ColumnKind::from_type_name("string"), ColumnKind::Text);
    assert_eq!(ColumnKind::from_type_name("text"), ColumnKind::LongText);
    assert_eq!(
        ColumnKind::from_type_name("big_integer"),
        ColumnKind::Integer
    );
    assert_eq!(ColumnKind::from_type_name("DateTime"), ColumnKind::DateTime);
    assert_eq!(
        ColumnKind::from_type_name("timestamp_with_time_zone"),
        ColumnKind::DateTime
    );
    assert_eq!(ColumnKind::from_type_name("json_binary"), ColumnKind::Json);
    // A spelling from a SeaORM this crate has not met is a text input, not a
    // panic.
    assert_eq!(ColumnKind::from_type_name("cidr"), ColumnKind::Unknown);
}

fn user_model() -> ModelDescriptor {
    ModelDescriptor::new("user", "user")
        .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
        .with(ColumnDescriptor::new("name", ColumnKind::Text).max_length(5))
        .with(ColumnDescriptor::new("age", ColumnKind::Integer))
        .with(ColumnDescriptor::new("bio", ColumnKind::LongText).nullable())
        .with(ColumnDescriptor::new("active", ColumnKind::Boolean))
}

#[test]
fn a_rejected_form_keeps_every_submitted_value() {
    let model = user_model();
    let admin = ModelAdmin::new("user");
    let mut form = Form::build(&model, &admin, None, &[]);

    let submission = Submission::new()
        .with("name", "Ada Lovelace") // too long for max_length(5)
        .with("age", "not a number")
        .with("bio", "Wrote the first program.")
        .with("active", "on");

    form.accept(&submission);
    assert!(!form.validate(), "the submission is not valid");

    // Every value survives the rejection, including the ones that were fine.
    assert_eq!(form.field("name").unwrap().value, "Ada Lovelace");
    assert_eq!(form.field("age").unwrap().value, "not a number");
    assert_eq!(form.field("bio").unwrap().value, "Wrote the first program.");
    assert!(form.field("active").unwrap().checked);

    // And the messages land on the fields that caused them.
    assert!(form.field("name").unwrap().has_errors());
    assert!(form.field("age").unwrap().has_errors());
    assert!(!form.field("bio").unwrap().has_errors());
}

#[test]
fn a_cleared_field_stays_cleared() {
    // The other half of the same bug: restoring the stored value over an
    // emptied input loses the edit just as surely.
    let model = user_model();
    let admin = ModelAdmin::new("user");
    let row = Row::new()
        .with("id", 1)
        .with("name", "Ada")
        .with("age", 36)
        .with("bio", "Something")
        .with("active", true);
    let mut form = Form::build(&model, &admin, Some(&row), &[]);
    assert_eq!(form.field("bio").unwrap().value, "Something");

    form.accept(
        &Submission::new()
            .with("name", "Ada")
            .with("age", "36")
            .with("bio", ""),
    );

    assert_eq!(form.field("bio").unwrap().value, "");
    assert!(
        !form.field("active").unwrap().checked,
        "an absent checkbox is off"
    );
}

#[test]
fn errors_from_the_data_source_land_beside_their_fields() {
    let model = user_model();
    let admin = ModelAdmin::new("user");
    let mut form = Form::build(&model, &admin, None, &[]);
    form.accept(&Submission::new().with("name", "Ada").with("age", "36"));
    assert!(form.validate());

    let mut errors = brocanth_admin::FieldErrors::new();
    errors.add("name", "has already been taken");
    errors.add_base("the record could not be saved");
    form.reject(&errors);

    assert!(!form.is_valid());
    assert_eq!(
        form.field("name").unwrap().errors,
        ["has already been taken"]
    );
    assert_eq!(
        form.field("name").unwrap().value,
        "Ada",
        "still what was typed"
    );
    assert_eq!(form.errors, ["the record could not be saved"]);
}

#[test]
fn a_read_only_field_is_never_written() {
    let model = user_model();
    let admin = ModelAdmin::new("user").readonly(["age"]);
    let mut form = Form::build(&model, &admin, None, &[]);

    // A crafted post naming a read-only column must not reach the values.
    form.accept(&Submission::new().with("name", "Ada").with("age", "999"));

    let values = form.values();
    let written: Vec<&str> = values.iter().map(|(n, _)| n.as_str()).collect();
    assert!(!written.contains(&"age"), "got {written:?}");
}

#[test]
fn a_foreign_key_field_offers_labels_rather_than_numbers() {
    let model = ModelDescriptor::new("post", "post")
        .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
        .with(ColumnDescriptor::new("author_id", ColumnKind::Integer).references("user", "id"));
    let admin = ModelAdmin::new("post");
    let choices = vec![(
        "author_id".to_string(),
        vec![
            Choice::new("1", "Ada Lovelace"),
            Choice::new("2", "Grace Hopper"),
        ],
    )];

    let form = Form::build(&model, &admin, None, &choices);
    let field = form.field("author_id").unwrap();

    assert_eq!(field.widget, Widget::ForeignKey);
    assert_eq!(field.choices[0].label, "Ada Lovelace");
    assert_eq!(field.choices[0].value, "1");
}

#[test]
fn a_rejected_create_re_renders_the_form_with_what_was_typed() {
    // The same rule, end to end, through the screen rather than the form.
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment),
    );

    let submission = Submission::new()
        .with("name", "Ada The Second")
        .with("email", "ada@example.com") // already taken in the fixture
        .with("bio", "A biography that must not be lost.")
        .with("role", "author")
        .with("active", "on");

    let response =
        brocanth_admin::rt::block_on(admin.create(Principal::anonymous(), "user", &submission))
            .unwrap();

    assert_eq!(response.status(), 422, "rejected, not redirected");
    let html = response.html().unwrap();
    assert!(html.contains("has already been taken"), "{html}");
    assert!(html.contains("Ada The Second"), "the name survived");
    assert!(
        html.contains("A biography that must not be lost."),
        "the textarea survived"
    );
    assert!(
        html.contains("ada@example.com"),
        "even the bad value survived"
    );
}
