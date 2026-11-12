//! The witness: how it is made, what it carries, and that it cannot be made
//! any other way.
//!
//! PLAN.md §9 obligations 2 and 3.

mod common;

use brocanth_policy::{Denied, Dialect, Pred, Value};
use common::{Post, PostColumn as C};

fn policy() -> Pred<C> {
    Pred::all([Pred::eq(C::AuthorId, 7i64), Pred::eq(C::Status, "draft")])
}

#[test]
fn a_verified_comes_only_from_a_resolved_admission() {
    // The constructive proof is this file compiling: there is no public
    // constructor, no public field, no `From`, no `Default`. The only
    // function in the crate returning a `Verified` is `Admission::resolve`,
    // and the only way to hold an `Admission` is `Pred::admission`.
    let verified = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    assert_eq!(verified.id(), &Value::Int(42));
    let _ = verified.release();
}

// The "cannot be constructed" and "cannot be cloned" halves are `compile_fail`
// doctests on `Verified` itself, in `src/verified.rs`. Doctests in an
// integration test file are not run, so putting them here would assert
// nothing.

#[test]
fn zero_rows_from_the_admission_query_is_a_refusal() {
    let denied = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(0);
    assert_eq!(denied.unwrap_err(), Denied::NotAuthorised);
}

#[test]
fn the_witness_carries_the_condition_into_the_write() {
    // §9 obligation 2. The condition the admission query tested is the
    // condition the UPDATE re-applies, so the TOCTOU window has nothing to
    // open onto.
    let admission = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles");
    let checked = admission.condition().sql().to_owned();
    let checked_binds = admission.condition().binds().to_vec();

    let write = admission
        .resolve(1)
        .expect("one row")
        .update(
            Dialect::Postgres,
            [(C::Status, Value::Text("published".into()))],
        )
        .expect("compiles");

    // The condition's binds come last, so its placeholders move up by the
    // number of binds before it: `$1` in the condition is `$3` in the UPDATE.
    let binds = write.query().binds();
    let offset = binds.len() - checked_binds.len();
    let spliced = renumber(&checked, offset);
    assert!(
        write.query().sql().contains(&spliced),
        "the UPDATE must contain the condition that was checked:\n  checked: {spliced}\n  wrote:   {}",
        write.query().sql()
    );
    assert_eq!(&binds[offset..], checked_binds.as_slice());
}

/// `sql` with each `$n` placeholder moved to `$(n + offset)`.
fn renumber(sql: &str, offset: usize) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c != '$' {
            continue;
        }
        let mut digits = String::new();
        while let Some(d) = chars.next_if(char::is_ascii_digit) {
            digits.push(d);
        }
        let n: usize = digits.parse().expect("a placeholder number");
        out.push_str(&(n + offset).to_string());
    }
    out
}

#[test]
fn zero_rows_affected_by_the_write_is_an_authorisation_failure() {
    // §9 obligation 2, the half everybody gets wrong: an ORM reports this as
    // `rows_affected: 0` and almost every application treats it as "nothing to
    // do".
    let write = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row")
        .delete(Dialect::Postgres)
        .expect("compiles");
    assert_eq!(write.applied(0).unwrap_err(), Denied::RaceLost);

    let write = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row")
        .delete(Dialect::Postgres)
        .expect("compiles");
    assert_eq!(write.applied(1), Ok(1));
}

#[test]
fn an_update_that_sets_nothing_is_refused() {
    use brocanth_policy::CompileError;

    let verified = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    assert_eq!(
        verified.update(Dialect::Postgres, []).unwrap_err(),
        CompileError::EmptyUpdate
    );
}

#[test]
#[should_panic(expected = "was dropped without being used")]
fn dropping_a_witness_without_using_it_fails_a_debug_build() {
    // `#[must_use]` catches an ignored return value and nothing else — binding
    // it to a name silences the lint. The drop check is what catches the rest.
    // Debug only, which is where tests run.
    let verified = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    drop(verified);
}

#[test]
fn releasing_a_witness_is_a_choice_rather_than_an_accident() {
    // For a caller whose write is not a statement this crate can build. The
    // condition comes back, because it still has to guard whatever they do.
    let verified = policy()
        .admission::<Post>(Dialect::Postgres, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    let (id, condition) = verified.release();
    assert_eq!(id, Value::Int(42));
    assert!(condition.sql().contains("author_id"));
}
