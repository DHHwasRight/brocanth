//! The macro's errors, asserted against the compiler's own output.
//!
//! PLAN.md §9's vulnerability class is a typo that grants access. With the
//! macro, each of these is a compile error on the author's own token:
//!
//! * a column the entity does not have, in a rule or in a field mask;
//! * a value of the wrong type for its column;
//! * an `Option` value, with no word for what absence means;
//! * a column that does not match the model struct it names.
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
}
