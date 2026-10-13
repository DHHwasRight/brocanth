//! Every way this crate says no, and the reason each one is separate.

use thiserror::Error;

/// A predicate that cannot be turned into SQL.
///
/// All of these are programming errors rather than request errors. They are
/// returned rather than panicked because a policy can be assembled at run time
/// from configuration, and a framework that panics on a bad policy takes the
/// process down for one misconfigured table.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CompileError {
    /// A table or column name was the empty string.
    #[error("an identifier was empty")]
    EmptyIdentifier,

    /// A name was not `[A-Za-z_][A-Za-z0-9_]*`.
    ///
    /// This crate quotes identifiers, so a name with a space would survive
    /// quoting — and it is still rejected, because a name that *needs* the
    /// quoting to be safe is a name that arrived from somewhere it should not
    /// have. See [`crate::Ident`].
    #[error("`{name}` is not a bare SQL identifier")]
    BadIdentifier {
        /// The offending name.
        name: &'static str,
    },

    /// An [`crate::Opaque`] fragment's placeholder count did not match the
    /// number of values it carries.
    ///
    /// Caught here rather than by the database, because the database would
    /// report it against a statement nobody wrote by hand.
    #[error("opaque fragment `{sql}` has {placeholders} placeholder(s) but {binds} bind(s)")]
    PlaceholderMismatch {
        /// The fragment.
        sql: &'static str,
        /// How many `?` it contains.
        placeholders: usize,
        /// How many values it carries.
        binds: usize,
    },

    /// An [`crate::Opaque`] fragment used a placeholder this crate does not
    /// renumber.
    ///
    /// A fragment writes `?` and only `?`. Dialect placeholder syntax is
    /// [`crate::sql`]'s job, and a fragment that writes `$1` itself would
    /// break the moment the dialect changed.
    #[error("opaque fragment `{sql}` must use `?` placeholders, not `$n`")]
    PlaceholderSyntax {
        /// The fragment.
        sql: &'static str,
    },

    /// An [`crate::Opaque`] fragment contained a quote.
    ///
    /// A fragment must carry no literals at all: every value binds. The rule
    /// is not fussiness — the compiler counts `?` to renumber placeholders for
    /// a dialect that needs `$n`, and a `?` inside a string literal would make
    /// that count wrong. Forbidding literals outright is both the simpler rule
    /// and the one that keeps "a value is never SQL text" true inside the
    /// escape hatch too.
    #[error("opaque fragment `{sql}` contains a quote; every value must bind")]
    LiteralInFragment {
        /// The fragment.
        sql: &'static str,
    },

    /// A condition checked for one dialect was about to be written into a
    /// statement for another.
    ///
    /// A witness proved something against one engine's semantics. Re-targeting
    /// it silently would change `IS` into `IS NOT DISTINCT FROM` under a proof
    /// that never saw it, so the conversion is refused rather than performed.
    #[error("the condition was checked for {checked:?} and the write is for {writing:?}")]
    DialectMismatch {
        /// The dialect the condition was compiled and checked for.
        checked: crate::Dialect,
        /// The dialect the write asked for.
        writing: crate::Dialect,
    },

    /// An `UPDATE` with nothing to set.
    ///
    /// `UPDATE t SET WHERE …` is a syntax error, and a write with no
    /// assignments is a caller bug rather than a no-op worth hiding.
    #[error("an update must set at least one column")]
    EmptyUpdate,
}

/// Why a row could not be evaluated in memory.
///
/// Every variant is a limit of the single-row interpretation, which is the
/// honest reason PLAN.md §9 obligation 1 makes `eval()` opt-in: it can be
/// *absent*, where `condition()` always exists.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum EvalError {
    /// The predicate asks a question about another table.
    ///
    /// `EXISTS (SELECT 1 FROM team_members …)` is answerable by a database and
    /// not by a row you already hold. Compile the predicate instead, or hold
    /// the membership in a column.
    #[error("`{table}` membership cannot be decided from one row")]
    NeedsDatabase {
        /// The related table the predicate reaches into.
        table: &'static str,
    },

    /// The predicate contains an [`crate::Opaque`] fragment with no
    /// evaluator.
    ///
    /// An opaque fragment is SQL this crate does not understand. Supplying
    /// `Opaque::with_eval` is what makes it evaluable; omitting it is a
    /// deliberate choice to be `condition()`-only.
    #[error("opaque fragment ({reason}) has no in-memory evaluator")]
    Opaque {
        /// The reason the fragment's author recorded.
        reason: &'static str,
    },
}

/// Authorisation refused.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Denied {
    /// The admission query matched no row.
    ///
    /// Either the row does not exist or the subject may not have it, and this
    /// crate does not distinguish the two — the distinction is itself a
    /// disclosure, and PLAN.md §9's whole subject is rules that leak.
    #[error("not authorised")]
    NotAuthorised,

    /// The write ran and changed nothing.
    ///
    /// PLAN.md §9 obligation 2: the write is `UPDATE … WHERE id = ? AND
    /// <condition>`, so zero rows affected means the row stopped matching the
    /// condition between the check and the write. **That is an authorisation
    /// failure, not a no-op**, and it is the TOCTOU window closing on someone.
    #[error("the row stopped matching the condition between the check and the write")]
    RaceLost,
}
