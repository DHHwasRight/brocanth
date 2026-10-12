//! The `Validate` contract and the write path that enforces it.

use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, IntoActiveModel,
};

use crate::{Error, Errors};

/// Rules a record must pass before it is written.
///
/// `#[validates(..)]` implements this for the `ActiveModel`. Implement it by
/// hand when a rule needs logic the attribute cannot express; the two compose,
/// because the attribute only ever writes one impl per type.
#[async_trait]
pub trait Validate {
    /// Rules that need no database access.
    fn validate(&self, errors: &mut Errors);

    /// Rules that query, such as uniqueness.
    ///
    /// SeaORM 2.0 hands `before_save` a connection, which is what makes this
    /// enforceable rather than advisory.
    async fn validate_db<C>(&self, db: &C, errors: &mut Errors) -> Result<(), DbErr>
    where
        C: ConnectionTrait,
    {
        let _ = db;
        let _ = errors;
        Ok(())
    }
}

/// Run every rule and report. Implemented for everything that is [`Validate`].
#[async_trait]
pub trait Validated: Validate + Sync {
    /// Collect every failure.
    async fn errors<C>(&self, db: &C) -> Result<Errors, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut errors = Errors::new();
        self.validate(&mut errors);
        self.validate_db(db, &mut errors).await?;
        Ok(errors)
    }

    /// `true` when the record would be accepted.
    async fn valid<C>(&self, db: &C) -> Result<bool, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self.errors(db).await?.is_empty())
    }
}

impl<T: Validate + Sync + ?Sized> Validated for T {}

/// The `before_save` bridge.
///
/// A generated hook calls this, so a write cannot reach SQL with an invalid
/// record. Failure is encoded into the `DbErr` the hook must return and decoded
/// again by [`Error::from`], which is why even a direct
/// `active_model.insert(db)` still yields field-level errors.
pub async fn enforce<A, C>(model: A, db: &C) -> Result<A, DbErr>
where
    A: Validate + Send + Sync,
    C: ConnectionTrait,
{
    let errors = model.errors(db).await?;
    if errors.is_empty() {
        Ok(model)
    } else {
        Err(errors.into_db_err())
    }
}

/// The write verbs, with [`Error`] instead of [`DbErr`] so a caller can branch
/// on "the form was wrong" without string matching.
///
/// Every one of these goes through SeaORM's per-row path, so hooks run. For the
/// set-based verbs see [`crate::bulk`].
#[async_trait]
pub trait Write: ActiveModelTrait + ActiveModelBehavior
where
    <Self::Entity as EntityTrait>::Model: IntoActiveModel<Self>,
{
    /// `INSERT`, returning the stored row.
    async fn insert_checked<C>(self, db: &C) -> Result<<Self::Entity as EntityTrait>::Model, Error>
    where
        C: ConnectionTrait,
        Self: Send,
    {
        self.insert(db).await.map_err(Error::from)
    }

    /// `UPDATE`, returning the stored row.
    async fn update_checked<C>(self, db: &C) -> Result<<Self::Entity as EntityTrait>::Model, Error>
    where
        C: ConnectionTrait,
        Self: Send,
    {
        self.update(db).await.map_err(Error::from)
    }

    /// `INSERT` or `UPDATE`, whichever the primary key calls for.
    async fn save_checked<C>(self, db: &C) -> Result<Self, Error>
    where
        C: ConnectionTrait,
        Self: Send,
    {
        self.save(db).await.map_err(Error::from)
    }

    /// `DELETE`, returning the number of rows removed.
    async fn delete_checked<C>(self, db: &C) -> Result<u64, Error>
    where
        C: ConnectionTrait,
        Self: Send,
    {
        self.delete(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(Error::from)
    }
}

impl<A> Write for A
where
    A: ActiveModelTrait + ActiveModelBehavior,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
}
