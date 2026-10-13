//! Field-level write authorisation, checked in `brocanth-model`'s own
//! vocabulary.
//!
//! `Tracked::changed()` returns `Vec<&'static str>` — the database's column
//! names, from the entity's column enum, with a column set to the value it
//! already holds not counted. Every `changed` list here is written as that
//! method would return it.

mod common;

use brocanth_policy::{FieldMask, MaskViolation};
use common::PostColumn as C;

/// What `Tracked::changed()` returns after a form touched the status.
const CHANGED_STATUS: [&str; 1] = ["status"];
/// Two columns at once.
const CHANGED_TWO: [&str; 2] = ["status", "author_id"];
/// Rails and `dirty.rs` agree: setting a column to the value it already holds
/// is not a change, so an untouched save produces this.
const CHANGED_NOTHING: [&str; 0] = [];

#[test]
fn the_default_denies() {
    assert_eq!(FieldMask::<C>::default(), FieldMask::deny_all());
    assert!(!FieldMask::<C>::default().permits(C::Status));
}

#[test]
fn a_mask_rejects_a_write_to_a_masked_field() {
    // PocketBase's `status:changed = false || role = "admin"`, as a value: a
    // non-admin may write the body and never the status.
    let mask = FieldMask::only([C::Score]);
    assert_eq!(
        mask.check(CHANGED_STATUS),
        Err(MaskViolation::Forbidden {
            fields: vec!["status".to_owned()]
        })
    );
    assert_eq!(FieldMask::only([C::Status]).check(CHANGED_STATUS), Ok(()));
}

#[test]
fn a_write_that_changed_nothing_passes_even_the_deny_all_mask() {
    // This is why the mask is checked against `changed()` rather than against
    // the form's field list: a round trip that touched nothing is not a write
    // to anything, and `dirty.rs` already says so.
    assert_eq!(FieldMask::<C>::deny_all().check(CHANGED_NOTHING), Ok(()));
}

#[test]
fn every_rejected_field_is_reported_not_just_the_first() {
    let mask = FieldMask::only([C::Score]);
    assert_eq!(
        mask.check(CHANGED_TWO),
        Err(MaskViolation::Forbidden {
            fields: vec!["status".to_owned(), "author_id".to_owned()]
        })
    );
}

#[test]
fn except_is_the_convenient_shape_and_only_is_the_safe_one() {
    let except = FieldMask::except([C::Id]);
    assert!(except.permits(C::Status));
    assert!(!except.permits(C::Id));
    assert_eq!(except.check(CHANGED_TWO), Ok(()));

    let only = FieldMask::only([C::Status]);
    assert!(only.permits(C::Status));
    assert!(!only.permits(C::Score));
}

#[test]
fn all_permits_everything() {
    let mask = FieldMask::<C>::all();
    for column in <C as brocanth_policy::Column>::ALL {
        assert!(mask.permits(*column));
    }
    assert_eq!(mask.check(CHANGED_TWO), Ok(()));
}

#[test]
fn a_changed_field_that_is_not_a_column_is_an_error_in_both_directions() {
    // Under `Except` the permissive reading would let an unrecognised name
    // through, which is PLAN.md §9's vulnerability class wearing a different
    // hat: a name nobody checked, resolved in the generous direction.
    for mask in [
        FieldMask::<C>::all(),
        FieldMask::deny_all(),
        FieldMask::only([C::Status]),
        FieldMask::except([C::Id]),
    ] {
        assert_eq!(
            mask.check(["statuss"]),
            Err(MaskViolation::UnknownField {
                fields: vec!["statuss".to_owned()]
            }),
            "an unknown field must not be decided by the mask's shape"
        );
    }
}

#[test]
fn masks_intersect_rather_than_union() {
    // Composition has to take the stricter of two rules. A union would let the
    // looser one win, which is the wrong direction for the same reason every
    // other default here points at denial.
    let role = FieldMask::only([C::Status, C::Score]);
    let lifecycle = FieldMask::except([C::Status]);
    let both = role.intersect(&lifecycle);
    assert!(both.permits(C::Score));
    assert!(!both.permits(C::Status));
    assert!(!both.permits(C::AuthorId));

    assert_eq!(
        FieldMask::<C>::all().intersect(&FieldMask::only([C::Score])),
        FieldMask::only([C::Score])
    );
    assert_eq!(
        FieldMask::<C>::deny_all().intersect(&FieldMask::all()),
        FieldMask::deny_all()
    );
}

#[test]
fn a_policys_mask_reads_the_way_section_nine_writes_it() {
    use brocanth_policy::{Bound, EntityPred, Policy, Pred};
    use common::Post;

    struct Visitor {
        user_id: Option<i64>,
        admin: bool,
    }

    struct PostPolicy;

    impl Policy for PostPolicy {
        type Entity = Post;
        type Subject = Visitor;

        fn list(&self, who: &Visitor) -> EntityPred<Post> {
            Pred::any([
                Pred::eq(C::Status, "published"),
                Pred::eq(C::AuthorId, Bound::or_deny(who.user_id)),
            ])
        }

        fn writable(&self, who: &Visitor) -> FieldMask<C> {
            if who.admin {
                FieldMask::except([C::Id])
            } else {
                FieldMask::only([C::Score])
            }
        }
    }

    let author = Visitor {
        user_id: Some(7),
        admin: false,
    };
    let admin = Visitor {
        user_id: Some(1),
        admin: true,
    };

    assert!(PostPolicy.writable(&author).check(CHANGED_STATUS).is_err());
    assert_eq!(PostPolicy.writable(&admin).check(CHANGED_STATUS), Ok(()));

    // The defaults deny, and only `list` was written.
    assert!(PostPolicy.update(&admin).is_never());
    assert!(PostPolicy.delete(&admin).is_never());
    assert!(PostPolicy.create(&admin).is_never());
    // `view` follows `list`, because a row you may see in a list is a row you
    // may see.
    assert_eq!(PostPolicy.view(&author), PostPolicy.list(&author));
}
