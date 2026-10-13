//! A resource: an entity, a policy, a data source, and the few decisions a
//! framework cannot make for you.
//!
//! # The assertion
//!
//! PLAN.md §12 asks that *a build of `brocanth-resource` with no policy
//! registered must not compile*. [`Resource::Policy`] is an associated type
//! with **no default**, bound to [`Policy`]. Three consequences, and the first
//! is the one that matters:
//!
//! 1. A `Resource` impl that omits `type Policy` does not compile. There is
//!    nothing to forget it in favour of and nothing to fall back to, because
//!    every route in this crate is built from a `Resource` and from nothing
//!    else.
//! 2. A `Resource` impl that names a type which is not a `Policy` does not
//!    compile either, so the gap cannot be papered over with `()`.
//! 3. The policy is bound to this resource's own entity and subject
//!    (`Policy<Entity = Self::Entity, Subject = Self::Subject>`), so a policy
//!    borrowed from another table does not compile and the compile error names
//!    both types.
//!
//! `tests/ui/` holds one case for each and `tests/trybuild.rs` compares the
//! compiler's own output against a snapshot, because an error that merely
//! fires is not enough — it has to name what is missing.
//!
//! The weakest policy that compiles still answers "who may list these rows",
//! because [`Policy::list`] has no default either. Everything else defaults to
//! [`Pred::Never`](brocanth_policy::Pred::Never) and
//! [`FieldMask::deny_all`](brocanth_policy::FieldMask::deny_all): a resource whose author wrote
//! only `list` has no create, update or delete, and answers `403` on all
//! three.
//!
//! # What a resource has to decide, and why this crate will not
//!
//! * **How an id in a path becomes a [`Value`]** — because `"42"` is an
//!   integer key in one table and a text key in the next, and guessing is a
//!   type confusion that a policy would then compare the wrong way.
//! * **How a body becomes columns** — because the body format is the
//!   application's, and a framework with an opinion about it links a
//!   serialiser. [`Resource::parse_write`] returns typed
//!   `(Column, Value)` pairs, which is also the point at which an unknown
//!   field name stops existing: there is no string left by the time a policy
//!   sees it.
//! * **How a row becomes bytes** — same reason, in reverse.
//!
//! Everything else has a default that denies or a default that is obviously
//! right.

use brocanth_events::ChangeSink;
use brocanth_policy::{Column, Dialect, Entity, Policy, Value};

use crate::data::ResourceData;
use crate::error::Failure;

/// The row type a resource's data source returns.
pub type RowOf<R> = <<R as Resource>::Data as ResourceData<<R as Resource>::Entity>>::Row;

/// The column enum a resource's entity is written in.
pub type ColumnOf<R> = <<R as Resource>::Entity as Entity>::Column;

/// Which way a listing is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// `ASC`
    Ascending,
    /// `DESC`
    Descending,
}

impl Direction {
    /// The SQL keyword.
    pub fn as_sql(self) -> &'static str {
        match self {
            Direction::Ascending => "ASC",
            Direction::Descending => "DESC",
        }
    }
}

/// One column and a direction.
///
/// Typed, like everything else a policy touches: there is no sort string, and
/// [`Resource::sort_column`] is the only way a name from a query string
/// becomes a column. A resource that does not implement it accepts no `sort`
/// parameter at all, which is the safe default and not a limitation anyone
/// discovers in production.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<C: Column> {
    /// The column to order by.
    pub column: C,
    /// Which way.
    pub direction: Direction,
}

impl<C: Column> Sort<C> {
    /// Ascending.
    pub fn ascending(column: C) -> Self {
        Sort {
            column,
            direction: Direction::Ascending,
        }
    }

    /// Descending.
    pub fn descending(column: C) -> Self {
        Sort {
            column,
            direction: Direction::Descending,
        }
    }
}

/// How the create rule is decided for a row that is not in the database yet.
///
/// Every other operation asks the database, because the row is there to ask
/// about. A create has no row, so PLAN.md §9's two interpretations become
/// visible as a choice, and the choice is written down rather than assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateCheck {
    /// Synthesise the row as a one-row CTE and let the database decide.
    ///
    /// The default, and what PocketBase's own create path does. PLAN.md §9
    /// obligation 1: *"Default to `condition()` only"* — one interpretation
    /// cannot disagree with itself, and the text the database evaluates here
    /// is character for character the text it evaluates for a list.
    /// [`crate::sql::create_admission`] builds it.
    Database,

    /// Evaluate the predicate against the proposed row in memory.
    ///
    /// Opt-in, and opting in takes on PLAN.md §9 obligation 1: the resource
    /// owes a proof that `eval(row) == (row in query(condition))`, and
    /// `brocanth_policy::agreement` is the harness for it. Worth taking when
    /// the create rule is a plain comparison and the round trip is not, and
    /// not worth taking otherwise.
    ///
    /// A predicate that cannot be decided in memory — one that reaches into
    /// another table — is refused loudly as a `500` rather than quietly as a
    /// denial, because a quiet denial is a bug that reaches production.
    InMemory,
}

/// An entity, its policy, its data source, and five routes.
pub trait Resource: Send + Sync + 'static {
    /// The table.
    type Entity: Entity;

    /// **The rules. No default: this is the assertion of PLAN.md §12.**
    type Policy: Policy<Entity = Self::Entity, Subject = Self::Subject>;

    /// Whoever is asking. The application's type; this crate never looks
    /// inside one.
    type Subject;

    /// Where the rows are.
    type Data: ResourceData<Self::Entity>;

    /// The name of the resource, used for the route names (`posts.list`) and
    /// as the default plugin instance name.
    const NAME: &'static str;

    /// Where this resource would put itself if it were asked. A suggestion:
    /// the application mounts the bundle at a prefix it chooses, per ADR 3.
    const PREFIX: &'static str = "/";

    /// Which SQL this resource's policy is compiled for.
    const DIALECT: Dialect = Dialect::Postgres;

    /// The page size when the client does not ask for one.
    const DEFAULT_PER_PAGE: u64 = 30;

    /// The largest page size a client can ask for. A larger request is clamped
    /// to it rather than refused.
    const MAX_PER_PAGE: u64 = 500;

    /// How the create rule is decided. See [`CreateCheck`].
    const CREATE_CHECK: CreateCheck = CreateCheck::Database;

    /// The rules.
    fn policy(&self) -> &Self::Policy;

    /// The rows.
    fn data(&self) -> &Self::Data;

    /// Where a successful write is announced.
    ///
    /// Defaults to a sink that drops everything, so a resource in an
    /// application with no realtime layer costs one virtual call that returns
    /// `false` per write. `ChangeSink::wants` is checked before the change is
    /// built, so nothing is allocated for a collection nobody listens to.
    fn sink(&self) -> &dyn ChangeSink {
        &brocanth_events::NoopSink
    }

    /// The name a [`RowChange`](brocanth_events::RowChange) carries. The table
    /// name, unless the application's realtime layer knows it as something
    /// else.
    fn collection(&self) -> &str {
        <Self::Entity as Entity>::TABLE.as_str()
    }

    /// Turn the `{id}` path segment into a primary key value.
    ///
    /// # Errors
    ///
    /// [`Failure::BadRequest`] when the segment is not a key of this entity.
    /// It reaches the client as `400`, which is correct: a malformed id is a
    /// malformed request and reveals nothing, where a `404` would quietly
    /// turn a client bug into "no such record".
    fn parse_id(&self, raw: &str) -> Result<Value, Failure>;

    /// Turn a request body into the columns it sets.
    ///
    /// This is where field names stop being strings. Everything downstream —
    /// the field mask, the create admission, the `UPDATE` — works in the
    /// entity's own column enum.
    ///
    /// # Errors
    ///
    /// [`Failure::BadRequest`] when the body is not usable, which includes a
    /// field this resource does not accept from a client at all.
    fn parse_write(&self, body: &[u8]) -> Result<Vec<(ColumnOf<Self>, Value)>, Failure>;

    /// Append the encoded form of one row.
    ///
    /// Called once per row in a page, into one buffer, so a listing is one
    /// allocation that grows rather than one per row. [`crate::json`] has the
    /// few helpers this needs if the encoding is JSON.
    ///
    /// # Errors
    ///
    /// [`Failure::BadRequest`] is not a sensible answer here; raise
    /// [`Failure::Data`] or [`Failure::Compile`] if a row cannot be rendered,
    /// both of which are `500`.
    fn encode_row(&self, row: &RowOf<Self>, out: &mut Vec<u8>) -> Result<(), Failure>;

    /// The order of a listing when the client does not ask for one.
    ///
    /// The primary key ascending. It is unique, so it is a total order, so two
    /// pages of one listing cannot overlap.
    fn default_sort(&self) -> Sort<ColumnOf<Self>> {
        Sort::ascending(<Self::Entity as Entity>::PRIMARY_KEY)
    }

    /// Map a `sort` query-string name to a column.
    ///
    /// `None` refuses the name, and the default refuses every name. A resource
    /// that wants client-chosen ordering writes the match itself, which is
    /// also the list of columns it is willing to have indexed.
    fn sort_column(&self, _name: &str) -> Option<ColumnOf<Self>> {
        None
    }
}
