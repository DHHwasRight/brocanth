//! The macro's errors, asserted against the compiler's own output.
//!
//! PLAN.md §9's vulnerability class is a typo that grants access. With the
//! macro, each of these is a compile error on the author's own token:
//!
//! * a column the entity does not have, in a rule or in a field mask;
//! * a value of the wrong type for its column;
//! * an `Option` value, with no word for what absence means;
//! * a column that does not match the model struct it names;
//! * `!` over `else deny` or `else allow`, which turns the absent case around.
//!
//! The snapshots keep the wording and the spans from drifting.

#[test]
fn the_macro_accepts_what_it_should_and_refuses_the_typo_class() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_policy.rs");
    t.compile_fail("tests/ui/fail_unknown_column.rs");
    t.compile_fail("tests/ui/fail_unknown_mask_column.rs");
    t.compile_fail("tests/ui/fail_type_mismatch.rs");
    t.compile_fail("tests/ui/fail_option_without_absence.rs");
    t.compile_fail("tests/ui/fail_unsupported_column_type.rs");
    t.compile_fail("tests/ui/fail_model_mismatch.rs");
    t.compile_fail("tests/ui/fail_unknown_rule.rs");
    t.compile_fail("tests/ui/fail_no_key.rs");
    t.compile_fail("tests/ui/fail_negated_absence.rs");
}

/// `#[db]`: the shapes of docs/policy-rls-bridge.md decision 3 are compile
/// errors with the doc's `help:` line, and so are `exists` mistakes and
/// the `#[db]`-only syntax without `#[db]`.
#[test]
fn db_refuses_what_the_database_cannot_check() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_db.rs");
    t.compile_fail("tests/ui/fail_db_block.rs");
    t.compile_fail("tests/ui/fail_db_guard.rs");
    t.compile_fail("tests/ui/fail_db_who_value.rs");
    t.compile_fail("tests/ui/fail_db_is_who.rs");
    t.compile_fail("tests/ui/fail_db_in_who.rs");
    t.compile_fail("tests/ui/fail_db_else_allow.rs");
    t.compile_fail("tests/ui/fail_db_not_attr.rs");
    t.compile_fail("tests/ui/fail_db_literal.rs");
    t.compile_fail("tests/ui/fail_db_only_without_db.rs");
    t.compile_fail("tests/ui/fail_exists_link.rs");
    t.compile_fail("tests/ui/fail_exists_negated.rs");
    t.compile_fail("tests/ui/fail_exists_names.rs");
}

/// `#[derive(Subject)]`: an `#[attr]` field of a type the database cannot
/// hold as a setting, and a shape the derive cannot read, are compile errors
/// that say what to write instead.
#[test]
fn the_subject_derive_refuses_what_a_setting_cannot_hold() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_subject.rs");
    t.compile_fail("tests/ui/fail_subject_attr_type.rs");
    t.compile_fail("tests/ui/fail_subject_shape.rs");
}
