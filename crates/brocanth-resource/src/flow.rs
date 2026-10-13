//! The five operations, with no HTTP in them.
//!
//! Each function takes a resource, a subject and what the request carried, and
//! returns an outcome or a [`Failure`]. [`crate::routes`] turns those into
//! responses; [`crate::status`] is the table. Keeping the two apart is what
//! lets every authorisation test in `tests/` run without a server, and what
//! lets a server that is not brocanth drive the same logic.
//!
//! # The order of the checks, which is load-bearing
//!
//! ```text
//! 1. is the rule Never?        -> 403, and no row is read
//! 2. parse the request         -> 400
//! 3. the field mask            -> 403, and still no row is read
//! 4. the admission query       -> 404 / 400
//! 5. the write, re-applying    -> 409 on zero rows
//! 6. publish                   -> only after 5 succeeded
//! ```
//!
//! Steps 1 and 3 come before step 4 so that their answers cannot be used to
//! learn whether a row exists. Step 6 comes after step 5 because a change
//! nobody made must not be announced; `tests/events.rs` asserts both halves.

use brocanth_events::{RowChange, RowId};
use brocanth_policy::{Column, MapRow, Policy, Value};
use std::borrow::Cow;

use crate::data::{FetchQuery, ListQuery, ResourceData};
use crate::error::Failure;
use crate::page::{Listing, PageRequest, Totals};
use crate::resource::{ColumnOf, CreateCheck, Resource, RowOf, Sort};
use crate::sql::{create_admission, Statement};

/// What a create produced.
#[derive(Clone, Debug)]
pub struct Created<Row> {
    /// The key the data source assigned.
    pub id: Value,
    /// The row as stored.
    pub row: Row,
}

/// What an update produced.
#[derive(Clone, Debug)]
pub struct Updated<Row> {
    /// The key that was written.
    pub id: Value,
    /// The row, re-read under the **view** predicate.
    ///
    /// `None` when the write moved the row out of the subject's own sight —
    /// unpublishing your own post under a policy that lists only published
    /// ones. The write stands; there is simply nothing to show, and
    /// [`crate::routes`] answers `204`.
    pub row: Option<Row>,
    /// The columns the write set, for the change event and for a caller that
    /// wants to log them.
    pub changed: Vec<&'static str>,
}

/// One page of rows, filtered by the policy **in the query**.
///
/// The condition goes to [`ResourceData::list`] and the same condition goes to
/// [`ResourceData::count`], so the page and the total describe one set. This
/// function never looks at a row it was given.
///
/// A list rule of [`Pred::Never`](brocanth_policy::Pred::Never) is [`Failure::NoRule`] and a `403`; a rule
/// that simply matches nothing is an empty page and a `200`. See
/// [`crate::status`] for why those differ.
///
/// # Errors
///
/// [`Failure::NoRule`], [`Failure::Compile`] or [`Failure::Data`].
pub fn list<R: Resource>(
    resource: &R,
    subject: &R::Subject,
    request: &PageRequest,
    sort: Option<Sort<ColumnOf<R>>>,
) -> Result<Listing<RowOf<R>>, Failure> {
    let pred = resource.policy().list(subject);
    if pred.is_never() {
        return Err(Failure::NoRule);
    }
    let condition = pred.condition::<R::Entity>(R::DIALECT)?;
    let (offset, limit) = request.window::<R>();
    let sort = sort.unwrap_or_else(|| resource.default_sort());
    let query = ListQuery::new(&condition, offset, limit, sort);

    // The count first, so that a total and a page read in that order can only
    // disagree by rows written in between — never by this crate's own
    // filtering, because there is none.
    let total = if request.skip_total {
        Totals::Skipped
    } else {
        Totals::Counted(resource.data().count(&query)?)
    };
    let items = resource.data().list(&query)?;

    Ok(Listing {
        items,
        page: request.page.max(1),
        per_page: limit,
        total,
    })
}

/// One row by id, with the policy's condition in the same statement.
///
/// PocketBase's by-id guard. There is no read-then-check, so there is no
/// window between the two and no way to forget the second half.
///
/// # Errors
///
/// [`Failure::NoRule`] when the view rule is [`Pred::Never`](brocanth_policy::Pred::Never),
/// [`Failure::NoMatch`] when the row is absent or refused — the two are one
/// answer — and [`Failure::Compile`] or [`Failure::Data`].
pub fn view<R: Resource>(
    resource: &R,
    subject: &R::Subject,
    raw_id: &str,
) -> Result<RowOf<R>, Failure> {
    let pred = resource.policy().view(subject);
    if pred.is_never() {
        return Err(Failure::NoRule);
    }
    let id = resource.parse_id(raw_id)?;
    let condition = pred.condition::<R::Entity>(R::DIALECT)?;
    resource
        .data()
        .fetch(&FetchQuery::new(&condition, &id))?
        .ok_or(Failure::NoMatch)
}

/// Create a row the policy admits.
///
/// There is no [`Verified`](brocanth_policy::Verified) here and there cannot
/// be: a witness is a proof about a row that exists, and this row does not yet.
/// What replaces it is that the admission test runs against the **proposed**
/// row — PLAN.md §9's synthesised one-row CTE — so the predicate that will
/// govern the row for the rest of its life is the predicate that let it in.
///
/// The field mask applies here too. "May this subject write this column" has
/// the same answer whether the row is new, and it is the mask that keeps a
/// client from setting the owner column on a row it is creating for somebody
/// else.
///
/// # Errors
///
/// [`Failure::NoRule`], [`Failure::BadRequest`], [`Failure::NotWritable`],
/// [`Failure::NoMatch`] when the policy refuses the proposed row, and the
/// internal kinds.
pub fn create<R: Resource>(
    resource: &R,
    subject: &R::Subject,
    body: &[u8],
) -> Result<Created<RowOf<R>>, Failure> {
    let pred = resource.policy().create(subject);
    if pred.is_never() {
        return Err(Failure::NoRule);
    }
    let values = resource.parse_write(body)?;
    if values.is_empty() {
        return Err(Failure::bad_request(
            "a create must set at least one column",
        ));
    }
    check_mask::<R>(resource, subject, &values)?;

    // `Always` admits every row, so the query could only have one answer.
    if !pred.is_always() {
        match R::CREATE_CHECK {
            CreateCheck::Database => {
                let condition = pred.condition::<R::Entity>(R::DIALECT)?;
                let statement = create_admission::<R::Entity>(R::DIALECT, &condition, &values)?;
                if resource.data().admit(&statement)? == 0 {
                    return Err(Failure::NoMatch);
                }
            }
            CreateCheck::InMemory => {
                let mut row = MapRow::new();
                for (column, value) in &values {
                    row = row.with(*column, value.clone());
                }
                if !pred.admits(&row)? {
                    return Err(Failure::NoMatch);
                }
            }
        }
    }

    let (id, row) = resource.data().insert(&values)?;
    announce(resource, &id, Change::Created);
    Ok(Created { id, row })
}

/// Change a row, through a witness that carries its condition.
///
/// PLAN.md §9 obligation 2, end to end: the admission test produces a
/// [`Verified`](brocanth_policy::Verified), the `UPDATE` re-applies that
/// witness's own condition in its `WHERE` clause, and zero rows affected is
/// [`Failure::RaceLost`] rather than a no-op. There is no path through this
/// function that writes without a witness, because
/// `brocanth_policy::Verified::update` takes one by value and nothing else
/// builds the statement.
///
/// # Errors
///
/// [`Failure::NoRule`], [`Failure::BadRequest`], [`Failure::NotWritable`],
/// [`Failure::NoMatch`], [`Failure::RaceLost`], and the internal kinds.
pub fn update<R: Resource>(
    resource: &R,
    subject: &R::Subject,
    raw_id: &str,
    body: &[u8],
) -> Result<Updated<RowOf<R>>, Failure> {
    let pred = resource.policy().update(subject);
    if pred.is_never() {
        return Err(Failure::NoRule);
    }
    let id = resource.parse_id(raw_id)?;
    let values = resource.parse_write(body)?;
    if values.is_empty() {
        // Caught here rather than by `CompileError::EmptyUpdate`, so that a
        // body that changes nothing is a `400` about the body and not a `500`
        // about the policy.
        return Err(Failure::bad_request(
            "an update must change at least one column",
        ));
    }

    // Before the admission query, and the status table explains why: a 403 for
    // a forbidden column must not also mean "and the row is there".
    check_mask::<R>(resource, subject, &values)?;

    let changed: Vec<&'static str> = values.iter().map(|(c, _)| c.name()).collect();

    let admission = pred.admission::<R::Entity>(R::DIALECT, id.clone())?;
    let rows = resource.data().admit(&Statement::from(admission.query()))?;
    // Zero here is `Denied::NotAuthorised`, which this crate reports as the
    // same answer as "no such row".
    let verified = admission.resolve(rows)?;

    let write = verified.update(R::DIALECT, values)?;
    let affected = resource.data().write(&Statement::from(write.query()))?;
    // Zero here is `Denied::RaceLost`. The condition was re-applied inside the
    // write, so the row stopped matching between the check and the write.
    write.applied(affected)?;

    announce(resource, &id, Change::Updated(&changed));

    // Re-read under the view predicate. A write may legally move a row out of
    // its author's own sight, and this is where that is noticed.
    let view_pred = resource.policy().view(subject);
    let row = if view_pred.is_never() {
        None
    } else {
        let condition = view_pred.condition::<R::Entity>(R::DIALECT)?;
        resource.data().fetch(&FetchQuery::new(&condition, &id))?
    };

    Ok(Updated { id, row, changed })
}

/// Remove a row, through the same witness and the same re-applied condition.
///
/// # Errors
///
/// As [`update`], without the mask: a delete writes no column.
pub fn delete<R: Resource>(
    resource: &R,
    subject: &R::Subject,
    raw_id: &str,
) -> Result<(), Failure> {
    let pred = resource.policy().delete(subject);
    if pred.is_never() {
        return Err(Failure::NoRule);
    }
    let id = resource.parse_id(raw_id)?;

    let admission = pred.admission::<R::Entity>(R::DIALECT, id.clone())?;
    let rows = resource.data().admit(&Statement::from(admission.query()))?;
    let verified = admission.resolve(rows)?;

    let write = verified.delete(R::DIALECT)?;
    let affected = resource.data().write(&Statement::from(write.query()))?;
    write.applied(affected)?;

    announce(resource, &id, Change::Deleted);
    Ok(())
}

/// `Policy::writable`, checked against the columns the request sets.
///
/// PLAN.md §9: *"`Policy::writable() -> FieldMask`, checked against the dirty
/// tracking the model layer already has. Nearly free, and almost nobody has
/// it."* The names come from the entity's own column enum, which is the same
/// place `brocanth_model::dirty::Tracked::changed()` gets them, so the two
/// sides agree without either one knowing about the other.
fn check_mask<R: Resource>(
    resource: &R,
    subject: &R::Subject,
    values: &[(ColumnOf<R>, Value)],
) -> Result<(), Failure> {
    let mask = resource.policy().writable(subject);
    let names: Vec<&'static str> = values.iter().map(|(c, _)| c.name()).collect();
    mask.check(names.iter().copied())?;
    Ok(())
}

enum Change<'a> {
    Created,
    Updated(&'a [&'static str]),
    Deleted,
}

/// Publish a [`RowChange`] after a write that really happened.
///
/// `ChangeSink::wants` is asked first, so an application with no realtime
/// layer builds nothing: no change, no owned collection name, no allocation
/// for the field list.
fn announce<R: Resource>(resource: &R, id: &Value, change: Change<'_>) {
    let sink = resource.sink();
    let collection = resource.collection();
    if !sink.wants(collection) {
        return;
    }
    let row = row_id(id);
    let change = match change {
        Change::Created => RowChange::created(collection, row),
        Change::Updated(fields) => RowChange::updated(collection, row, fields),
        Change::Deleted => RowChange::deleted(collection, row),
    };
    sink.publish(&change);
}

/// A primary key, as the events crate names rows.
///
/// An integer key and a text key map across exactly. Anything else is a key
/// this crate has no honest encoding for, and its rendered form is better than
/// dropping the change.
fn row_id(id: &Value) -> RowId<'static> {
    match id {
        Value::Int(n) => RowId::I64(*n),
        Value::Text(text) => RowId::Text(Cow::Owned(text.clone())),
        other => RowId::Text(Cow::Owned(other.to_string())),
    }
}
