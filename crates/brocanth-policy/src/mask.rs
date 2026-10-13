//! Field-level write authorisation, in the same language as the row-level
//! kind.
//!
//! PLAN.md §9, stealing PocketBase's `:changed`:
//!
//! > `@request.body.status:changed = false || @request.auth.role = "admin"` is
//! > *field-level* write authorization in the same language as row-level.
//! > Pundit keeps `permitted_attributes` as a separate concept; Django uses
//! > form fields. Here it becomes `Policy::writable() -> FieldMask`, checked
//! > against the dirty tracking the model layer already has. Nearly free, and
//! > almost nobody has it.
//!
//! # The vocabulary is `brocanth-model`'s, exactly
//!
//! `Tracked::changed()` returns `Vec<&'static str>` — the column names the
//! database spells, taken from the entity's own column enum, with a column set
//! to the value it already holds **not** counted as a change. [`FieldMask`] is
//! built from typed [`Column`] values and checked against those strings:
//!
//! ```ignore
//! let mut post = Tracked::load(row);
//! post.status = Set("published".to_owned());
//! policy.writable(&subject).check(post.changed())?;
//! ```
//!
//! The author never writes a field name and the model layer never learns about
//! policies. The only coupling is `&'static str`, which both sides already
//! derive from the same enum.
//!
//! # A name the mask does not recognise is an error
//!
//! Not "permitted", and not "denied". [`MaskViolation::UnknownField`], because
//! a changed-field name that is not one of [`Column::ALL`] means the caller
//! paired a mask with the wrong entity — and under [`FieldMask::except`] the
//! permissive reading would let an unknown name through, which is PLAN.md §9's
//! vulnerability class wearing a different hat.

use crate::entity::Column;

/// Which columns a subject may write.
///
/// The default is [`FieldMask::deny_all`], matching `brocanth-admin`'s
/// `DenyAll`: a mask nobody configured permits nothing, because a default is
/// what everyone's first deploy runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FieldMask<C: Column> {
    /// Every column.
    All,
    /// No column. The default.
    #[default]
    None,
    /// These columns and no others.
    ///
    /// The safe shape: a column added to the entity later is not writable
    /// until somebody says so.
    Only(Vec<C>),
    /// Every column but these.
    ///
    /// The convenient shape, and the one with the sharp edge: a column added
    /// to the entity later *is* writable, silently. Prefer [`FieldMask::Only`]
    /// for anything that matters.
    Except(Vec<C>),
}

impl<C: Column> FieldMask<C> {
    /// Every column.
    pub fn all() -> Self {
        FieldMask::All
    }

    /// No column.
    pub fn deny_all() -> Self {
        FieldMask::None
    }

    /// These and no others.
    pub fn only(columns: impl IntoIterator<Item = C>) -> Self {
        FieldMask::Only(columns.into_iter().collect())
    }

    /// Everything but these.
    pub fn except(columns: impl IntoIterator<Item = C>) -> Self {
        FieldMask::Except(columns.into_iter().collect())
    }

    /// May this column be written?
    pub fn permits(&self, column: C) -> bool {
        match self {
            FieldMask::All => true,
            FieldMask::None => false,
            FieldMask::Only(allowed) => allowed.contains(&column),
            FieldMask::Except(denied) => !denied.contains(&column),
        }
    }

    /// The intersection of two masks: a column has to pass both.
    ///
    /// What a policy that composes — a role mask and a lifecycle mask, say —
    /// needs, and the only composition that is safe to offer. A union would
    /// let the looser of two rules win.
    pub fn intersect(&self, other: &FieldMask<C>) -> FieldMask<C> {
        match (self, other) {
            (FieldMask::None, _) | (_, FieldMask::None) => FieldMask::None,
            (FieldMask::All, rest) | (rest, FieldMask::All) => rest.clone(),
            _ => FieldMask::Only(
                C::ALL
                    .iter()
                    .copied()
                    .filter(|c| self.permits(*c) && other.permits(*c))
                    .collect(),
            ),
        }
    }

    /// Check a list of changed field names — exactly what
    /// `brocanth_model::dirty::Tracked::changed()` returns.
    ///
    /// Reports **every** rejected field rather than the first, because a form
    /// that touched four forbidden columns should say so once.
    pub fn check<'a>(
        &self,
        changed: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), MaskViolation> {
        let mut rejected = Vec::new();
        let mut unknown = Vec::new();
        for field in changed {
            match C::ALL.iter().copied().find(|c| c.name() == field) {
                Some(column) => {
                    if !self.permits(column) {
                        rejected.push(field.to_owned());
                    }
                }
                None => unknown.push(field.to_owned()),
            }
        }
        if !unknown.is_empty() {
            return Err(MaskViolation::UnknownField { fields: unknown });
        }
        if !rejected.is_empty() {
            return Err(MaskViolation::Forbidden { fields: rejected });
        }
        Ok(())
    }
}

/// A write that touched a column it was not allowed to touch.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MaskViolation {
    /// These columns are not writable by this subject.
    #[error("not writable: {}", fields.join(", "))]
    Forbidden {
        /// The rejected column names, as the database spells them.
        fields: Vec<String>,
    },

    /// A changed field is not a column of this entity.
    ///
    /// The mask and the tracked record disagree about which table they are
    /// looking at. An error rather than a guess, in either direction.
    #[error("not a column of this entity: {}", fields.join(", "))]
    UnknownField {
        /// The unrecognised names.
        fields: Vec<String>,
    },
}
