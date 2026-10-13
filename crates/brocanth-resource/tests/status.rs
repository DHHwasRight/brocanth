//! The whole status table, in one place, so a change to it is a change to this
//! file and a reviewer sees it.
//!
//! The rule the table is written against: **a status code must not tell you
//! about a record you are not allowed to see.** `crate::status`'s module
//! documentation argues every line; this asserts them.

use brocanth_policy::{CompileError, Denied, EvalError, MaskViolation};
use brocanth_resource::data::DataError;
use brocanth_resource::status::{outcome, Action};
use brocanth_resource::Failure;

fn status(action: Action, failure: Failure) -> u16 {
    outcome(action, &failure).status
}

#[test]
fn a_rule_of_never_is_403_everywhere_because_it_names_no_record() {
    for action in [
        Action::List,
        Action::View,
        Action::Create,
        Action::Update,
        Action::Delete,
    ] {
        assert_eq!(status(action, Failure::NoRule), 403, "{action:?}");
    }
}

#[test]
fn an_unsatisfied_rule_hides_the_record_everywhere_it_has_one_to_hide() {
    // A list rule is a filter: an empty page, not a refusal.
    assert_eq!(status(Action::List, Failure::NoMatch), 200);
    // Nothing exists yet, so there is nothing to conceal.
    assert_eq!(status(Action::Create, Failure::NoMatch), 400);
    // The existence of the row is the secret, so 403 and 404 are one answer.
    assert_eq!(status(Action::View, Failure::NoMatch), 404);
    assert_eq!(status(Action::Update, Failure::NoMatch), 404);
    assert_eq!(status(Action::Delete, Failure::NoMatch), 404);
}

#[test]
fn a_lost_race_is_409_because_the_caller_already_knows_the_row_is_there() {
    assert_eq!(status(Action::Update, Failure::RaceLost), 409);
    assert_eq!(status(Action::Delete, Failure::RaceLost), 409);
    // `Denied::RaceLost` reaches here through the same conversion the flow
    // uses, so a change to `brocanth-policy` cannot quietly turn a lost race
    // into a refusal.
    assert_eq!(status(Action::Update, Denied::RaceLost.into()), 409);
    assert_eq!(status(Action::Update, Denied::NotAuthorised.into()), 404);
}

#[test]
fn a_forbidden_column_is_403_and_an_unknown_one_is_500() {
    let forbidden = Failure::NotWritable(MaskViolation::Forbidden {
        fields: vec!["status".to_owned()],
    });
    let answer = outcome(Action::Update, &forbidden);
    assert_eq!(answer.status, 403);
    assert!(
        answer.explain,
        "the client's mistake is in the body it sent, so the body says which column"
    );
    assert!(!answer.internal);

    // The mask and the record disagree about which table they describe. That
    // is this side of the wire, and sending the client to debug it would be a
    // lie about whose bug it is.
    let unknown = Failure::NotWritable(MaskViolation::UnknownField {
        fields: vec!["nope".to_owned()],
    });
    let answer = outcome(Action::Update, &unknown);
    assert_eq!(answer.status, 500);
    assert!(answer.internal);
}

#[test]
fn a_refusal_that_hides_a_record_never_explains_itself() {
    for (action, failure) in [
        (Action::View, Failure::NoMatch),
        (Action::Update, Failure::NoMatch),
        (Action::Delete, Failure::NoMatch),
        (Action::Create, Failure::NoMatch),
        (Action::Update, Failure::NoRule),
    ] {
        let answer = outcome(action, &failure);
        assert!(
            !answer.explain,
            "{action:?} must answer with the generic message: an explanation here \
             is the oracle the status code exists to close"
        );
    }
}

#[test]
fn the_application_s_own_failures_go_to_its_error_hooks_rather_than_to_a_body() {
    for failure in [
        Failure::Data(DataError::backend("the pool is gone")),
        Failure::Compile(CompileError::EmptyUpdate),
        Failure::Eval(EvalError::NeedsDatabase {
            table: "team_members",
        }),
    ] {
        let answer = outcome(Action::List, &failure);
        assert_eq!(answer.status, 500);
        assert!(answer.internal, "{failure:?}");
        assert!(failure.is_internal());
    }
}

#[test]
fn a_record_the_model_layer_rejects_is_the_clients_answer_and_says_why() {
    let failure = Failure::Data(DataError::invalid("title is too long"));
    let answer = outcome(Action::Create, &failure);

    assert_eq!(answer.status, 400);
    assert!(answer.explain);
    assert!(!answer.internal);
    assert!(!failure.is_internal());
}

#[test]
fn a_request_with_no_subject_at_all_is_401() {
    assert_eq!(status(Action::List, Failure::NoSubject), 401);
    // And anonymity is not this: an anonymous visitor is a subject with a
    // `None` inside, which the policy decides about. `tests/list.rs` has that
    // case and it answers 200.
}
