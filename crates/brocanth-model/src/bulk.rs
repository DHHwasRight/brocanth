//! Set-based writes, and the one place hooks are allowed to be skipped.
//!
//! # The decision, and why
//!
//! SeaORM's `update_many`, `delete_many` and `insert_many` build one statement
//! and never touch `ActiveModelBehavior`. A validation written as a
//! `before_save` therefore does not run, and neither does a timestamp, a
//! counter cache, or an audit row.
//!
//! **brocanth fans out to per-row writes by default.** Rails draws the same
//! line — `update` runs callbacks, `update_all` does not — and puts the safe
//! behaviour on the shorter name. The functions here load the affected rows,
//! write them one at a time inside a transaction, and run every hook. The cost
//! is one statement per row; the benefit is that a framework rule holds
//! everywhere, which is the only way a rule is worth writing.
//!
//! The bypass is still here, because a backfill over a million rows is a real
//! thing to want. It lives in [`unchecked`], and every function there takes a
//! [`HooksBypassed`] that has to be constructed with a reason:
//!
//! ```ignore
//! let witness = HooksBypassed::because("nightly backfill, no hooks on this table");
//! unchecked::update_many::<post::Entity>(witness)
//!     .col_expr(post::Column::Archived, Expr::value(true))
//!     .exec(db)
//!     .await?;
//! ```
//!
//! It is one line longer than the safe call and it says in the source why the
//! rules were suspended, so a reviewer sees it. Reaching it by accident is not
//! possible: the token cannot be defaulted or inferred.
//!
//! The remaining hole is `sea_orm::EntityTrait` itself, which a user can import
//! and call directly. Deny it in `clippy.toml`:
//!
//! ```toml
//! disallowed-methods = [
//!   { path = "sea_orm::EntityTrait::update_many", reason = "use brocanth_model::bulk" },
//!   { path = "sea_orm::EntityTrait::delete_many", reason = "use brocanth_model::bulk" },
//!   { path = "sea_orm::EntityTrait::insert_many", reason = "use brocanth_model::bulk" },
//! ]
//! ```

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, Condition, ConnectionTrait, EntityTrait,
    IntoActiveModel, QueryFilter, TransactionSession, TransactionTrait,
};

use crate::{Error, Write};

/// Written proof that a caller meant to skip every lifecycle hook.
///
/// There is no `Default` and no `new()`. The only way to make one is to say
/// why.
#[derive(Debug, Clone, Copy)]
pub struct HooksBypassed {
    reason: &'static str,
}

impl HooksBypassed {
    /// Declare that the hooks are being skipped, and why.
    pub fn because(reason: &'static str) -> Self {
        Self { reason }
    }

    /// The reason given.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

/// Insert many records, running every hook, in one transaction.
pub async fn insert_each<A, I, C>(
    rows: I,
    db: &C,
) -> Result<Vec<<A::Entity as EntityTrait>::Model>, Error>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
    I: IntoIterator<Item = A>,
    C: ConnectionTrait + TransactionTrait,
{
    let txn = db.begin().await?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.insert_checked(&txn).await?);
    }
    txn.commit().await?;
    Ok(out)
}

/// Update many records, running every hook, in one transaction.
pub async fn update_each<A, I, C>(
    rows: I,
    db: &C,
) -> Result<Vec<<A::Entity as EntityTrait>::Model>, Error>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
    I: IntoIterator<Item = A>,
    C: ConnectionTrait + TransactionTrait,
{
    let txn = db.begin().await?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.update_checked(&txn).await?);
    }
    txn.commit().await?;
    Ok(out)
}

/// Delete many records, running every hook, in one transaction.
pub async fn delete_each<A, I, C>(rows: I, db: &C) -> Result<u64, Error>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
    I: IntoIterator<Item = A>,
    C: ConnectionTrait + TransactionTrait,
{
    let txn = db.begin().await?;
    let mut removed = 0;
    for row in rows {
        removed += row.delete_checked(&txn).await?;
    }
    txn.commit().await?;
    Ok(removed)
}

/// Load every row matching `filter`, apply `edit` to each, and write them back
/// one at a time so hooks run.
///
/// This is the hook-safe reading of `update_many().filter(..)`.
pub async fn update_where<E, F, C>(
    filter: Condition,
    edit: F,
    db: &C,
) -> Result<Vec<E::Model>, Error>
where
    E: EntityTrait,
    E::Model: IntoActiveModel<E::ActiveModel>,
    E::ActiveModel: ActiveModelBehavior + Send,
    F: Fn(&mut E::ActiveModel) + Send,
    C: ConnectionTrait + TransactionTrait,
{
    let txn = db.begin().await?;
    let found = E::find().filter(filter).all(&txn).await?;
    let mut out = Vec::with_capacity(found.len());
    for model in found {
        let mut active = model.into_active_model();
        edit(&mut active);
        out.push(active.update_checked(&txn).await?);
    }
    txn.commit().await?;
    Ok(out)
}

/// Load every row matching `filter` and delete them one at a time so hooks run.
pub async fn delete_where<E, C>(filter: Condition, db: &C) -> Result<u64, Error>
where
    E: EntityTrait,
    E::Model: IntoActiveModel<E::ActiveModel>,
    E::ActiveModel: ActiveModelBehavior + Send,
    C: ConnectionTrait + TransactionTrait,
{
    let txn = db.begin().await?;
    let found = E::find().filter(filter).all(&txn).await?;
    let mut removed = 0;
    for model in found {
        removed += model.into_active_model().delete_checked(&txn).await?;
    }
    txn.commit().await?;
    Ok(removed)
}

/// Set-based writes that skip every lifecycle hook.
///
/// Read the module documentation before reaching for these.
pub mod unchecked {
    use super::HooksBypassed;
    use sea_orm::{ActiveModelTrait, DeleteMany, EntityTrait, InsertMany, UpdateMany};

    /// `UPDATE … WHERE …` as one statement. No `before_save`, no validation.
    pub fn update_many<E: EntityTrait>(_: HooksBypassed) -> UpdateMany<E> {
        E::update_many()
    }

    /// `DELETE FROM … WHERE …` as one statement. No `before_delete`.
    pub fn delete_many<E: EntityTrait>(_: HooksBypassed) -> DeleteMany<E> {
        E::delete_many()
    }

    /// One multi-row `INSERT`. No `before_save`, no validation.
    pub fn insert_many<E, A, I>(_: HooksBypassed, models: I) -> InsertMany<A>
    where
        E: EntityTrait,
        A: ActiveModelTrait<Entity = E>,
        I: IntoIterator<Item = A>,
    {
        E::insert_many(models)
    }
}
