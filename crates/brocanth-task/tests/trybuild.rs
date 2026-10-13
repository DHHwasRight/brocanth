//! The macro's errors, asserted against the compiler's own output.
//!
//! A macro that merely *fails* on bad input is not enough: the failure is the
//! only thing the author will see, so it has to name what is wrong, point at
//! it, and say what is allowed instead. Snapshots are the only way to keep
//! that true as the macro changes.

#[test]
fn the_macro_accepts_what_it_should_and_explains_what_it_should_not() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_task.rs");
    t.compile_fail("tests/ui/fail_on_enum.rs");
    t.compile_fail("tests/ui/fail_unknown_argument.rs");
}
