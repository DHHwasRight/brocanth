//! Dirty tracking with the names Rails uses.
//!
//! `ActiveValue`'s `Set` / `Unchanged` / `NotSet` is most of the machinery
//! already. What it does not keep is the previous value: once a column is
//! `Set`, what was there before is gone. [`Tracked`] keeps the loaded row
//! beside the `ActiveModel`, which is what `was()` and `changes()` need.

use std::ops::{Deref, DerefMut};

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, IdenStatic,
    IntoActiveModel, Iterable, ModelTrait, Value,
};

use crate::{Error, Write};

/// One column that differs from the loaded row.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Column name as the database spells it.
    pub field: &'static str,
    /// Value in the loaded row, or `None` for a record that was never loaded.
    pub from: Option<Value>,
    /// Value that will be written.
    pub to: Value,
}

/// An `ActiveModel` beside the row it was loaded from.
///
/// Deref gives the `ActiveModel` itself, so assignment reads the same as it
/// does in SeaORM:
///
/// ```ignore
/// let mut user = Tracked::load(row);
/// user.name = Set("Ada".to_owned());
/// assert_eq!(user.changed(), ["name"]);
/// assert_eq!(user.was("name"), Some("Grace".into()));
/// ```
#[derive(Debug, Clone)]
pub struct Tracked<A: ActiveModelTrait> {
    active: A,
    original: Option<<A::Entity as EntityTrait>::Model>,
}

impl<A: ActiveModelTrait> Tracked<A> {
    /// Track a row that came from the database.
    pub fn load(model: <A::Entity as EntityTrait>::Model) -> Self
    where
        <A::Entity as EntityTrait>::Model: Clone + IntoActiveModel<A>,
    {
        Self {
            active: model.clone().into_active_model(),
            original: Some(model),
        }
    }

    /// Track a record that is not in the database yet. Everything set on it
    /// counts as a change, and `was()` is always `None`.
    pub fn fresh(active: A) -> Self {
        Self {
            active,
            original: None,
        }
    }

    /// The row as it was loaded, if it was.
    pub fn original(&self) -> Option<&<A::Entity as EntityTrait>::Model> {
        self.original.as_ref()
    }

    /// The value of a column in the loaded row.
    pub fn was(&self, field: &str) -> Option<Value> {
        let column = Self::column(field)?;
        self.original.as_ref().map(|m| m.get(column))
    }

    /// `true` when this column will be written with a different value.
    pub fn changed_field(&self, field: &str) -> bool {
        self.changes().iter().any(|c| c.field == field)
    }

    /// `true` when anything will be written.
    pub fn is_changed(&self) -> bool {
        !self.changes().is_empty()
    }

    /// Names of the columns that will be written with a different value.
    pub fn changed(&self) -> Vec<&'static str> {
        self.changes().into_iter().map(|c| c.field).collect()
    }

    /// Every column that will be written with a different value, with both
    /// values.
    ///
    /// Setting a column to the value it already holds is not a change, which
    /// matches Rails and keeps `changed()` honest after a form round trip that
    /// touched nothing.
    pub fn changes(&self) -> Vec<Change> {
        let mut out = Vec::new();
        for column in <A::Entity as EntityTrait>::Column::iter() {
            let ActiveValue::Set(to) = self.active.get(column) else {
                continue;
            };
            let from = self.original.as_ref().map(|m| m.get(column));
            if from.as_ref() == Some(&to) {
                continue;
            }
            out.push(Change {
                field: column.as_str(),
                from,
                to,
            });
        }
        out
    }

    /// The `ActiveModel`, dropping the tracking.
    pub fn into_active_model(self) -> A {
        self.active
    }

    /// Write the record, running hooks and validations.
    pub async fn save<C>(self, db: &C) -> Result<<A::Entity as EntityTrait>::Model, Error>
    where
        A: ActiveModelBehavior + Send,
        <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
        C: ConnectionTrait,
    {
        if self.original.is_some() {
            self.active.update_checked(db).await
        } else {
            self.active.insert_checked(db).await
        }
    }

    fn column(field: &str) -> Option<<A::Entity as EntityTrait>::Column> {
        <A::Entity as EntityTrait>::Column::iter().find(|c| c.as_str() == field)
    }
}

impl<A: ActiveModelTrait> Deref for Tracked<A> {
    type Target = A;
    fn deref(&self) -> &A {
        &self.active
    }
}

impl<A: ActiveModelTrait> DerefMut for Tracked<A> {
    fn deref_mut(&mut self) -> &mut A {
        &mut self.active
    }
}
