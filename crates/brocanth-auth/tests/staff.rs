//! `Staff` read off a request, and a permission as a row predicate.
//!
//! The admin's principal comes from `Staff::from_cookie_header`, so what it
//! answers for each kind of session is what the admin opens to.

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::http_client::StubHttp;
use brocanth_auth::mfa::{Admission, Enrolled, FactorPolicy};
use brocanth_auth::plugin::AuthConfig;
use brocanth_auth::session::{SessionConfig, SigningKey};
use brocanth_auth::staff::{ensure_superuser, SuperuserOutcome};
use brocanth_auth::storage::{AuthStore as _, Role, StaffStore as _};
use brocanth_auth::{
    AccountId, Auth, EmailAddress, FactorSet, MemoryStore, PasswordCost, Passwords, Staff,
};

type Services = Auth<MemoryStore, StubHttp, TestClock>;

fn services() -> Services {
    Auth::build(AuthConfig {
        store: Arc::new(MemoryStore::new()),
        http: Arc::new(StubHttp::new()),
        clock: Arc::new(TestClock::at(1_700_000_000)),
        signing_key: SigningKey::from_bytes([5; 32]),
        session: SessionConfig {
            cookie_name: "session".to_string(),
            secure: false,
            ..SessionConfig::default()
        },
        passwords: Arc::new(Passwords::new(PasswordCost::insecure_for_tests()).unwrap()),
        providers: Vec::new(),
    })
    .unwrap()
}

fn header(cookie: &brocanth_auth::session::SetCookie) -> String {
    cookie.as_str().split(';').next().unwrap().to_string()
}

async fn account(auth: &Services, email: &str) -> AccountId {
    auth.accounts()
        .register(email, "a long passphrase")
        .await
        .unwrap();
    auth.store()
        .account_by_email(&EmailAddress::parse(email).unwrap())
        .await
        .unwrap()
        .unwrap()
        .id
}

/// A signed-in session for `email`, opened with its password.
async fn signed_in(auth: &Services, email: &str) -> String {
    let proof = auth
        .accounts()
        .login(email, "a long passphrase")
        .await
        .unwrap()
        .into_proof();
    let enrolled = Enrolled::without_asking_the_store(proof.account().clone(), FactorSet::empty());
    let Admission::Admit(admitted) = FactorPolicy::recommended().admit(&enrolled, proof) else {
        panic!("no second factor, so the password admits");
    };
    let started = auth
        .sessions()
        .begin_with_factors(admitted, None)
        .await
        .unwrap();
    header(&started.cookie)
}

/// No cookie, a forged one, an anonymous session on a superuser's account,
/// and a signed-in account that is not staff are all `None`. A signed-in
/// superuser or role holder is `Some`.
#[tokio::test]
async fn only_a_signed_in_member_of_staff_is_staff() {
    let auth = services();
    let ops = account(&auth, "ops@example.com").await;
    auth.store().set_superuser(&ops, true).await.unwrap();
    let ed = account(&auth, "ed@example.com").await;
    account(&auth, "plain@example.com").await;

    assert_eq!(Staff::from_cookie_header(&auth, "").await.unwrap(), None);
    assert_eq!(
        Staff::from_cookie_header(&auth, "session=forged.value")
            .await
            .unwrap(),
        None
    );

    // A pre-login session names the superuser's account and proves nothing.
    let anonymous = auth.sessions().begin_anonymous(&ops).await.unwrap();
    assert_eq!(
        Staff::from_cookie_header(&auth, &header(&anonymous.cookie))
            .await
            .unwrap(),
        None,
        "an anonymous session is not staff"
    );

    let plain = signed_in(&auth, "plain@example.com").await;
    assert_eq!(
        Staff::from_cookie_header(&auth, &plain).await.unwrap(),
        None
    );

    let session = signed_in(&auth, "ops@example.com").await;
    let staff = Staff::from_cookie_header(&auth, &session)
        .await
        .unwrap()
        .expect("a superuser is staff");
    assert!(staff.is_superuser());

    auth.store()
        .put_role(&Role::new("editor", ["post.change"]))
        .await
        .unwrap();
    auth.store().assign_role(&ed, "editor").await.unwrap();
    let session = signed_in(&auth, "ed@example.com").await;
    let staff = Staff::from_cookie_header(&auth, &session)
        .await
        .unwrap()
        .expect("a role holder is staff");
    assert_eq!(staff.roles(), ["editor"]);

    // Revoked mid-session: the next request reads the store again.
    auth.store().unassign_role(&ed, "editor").await.unwrap();
    assert_eq!(
        Staff::from_cookie_header(&auth, &session).await.unwrap(),
        None
    );
}

/// The superuser at start runs at every boot: the first makes it, the next
/// leave it alone, and none resets the password somebody changed.
#[tokio::test]
async fn the_superuser_at_start_is_idempotent_across_boots() {
    let auth = services();
    let store = auth.store().clone();
    let mut outcomes = Vec::new();
    for _ in 0..3 {
        outcomes.push(
            ensure_superuser(
                auth.accounts(),
                &*store,
                "ops@example.com",
                "first passphrase",
            )
            .await
            .unwrap(),
        );
    }
    let id = outcomes[0].account().clone();
    assert_eq!(
        outcomes,
        [
            SuperuserOutcome::Created(id.clone()),
            SuperuserOutcome::Unchanged(id.clone()),
            SuperuserOutcome::Unchanged(id.clone()),
        ]
    );
    assert_eq!(store.superusers().await.unwrap(), std::slice::from_ref(&id));

    // The operator turned the flag off by hand; the next boot puts it back,
    // and still keeps the password.
    store.set_superuser(&id, false).await.unwrap();
    let again = ensure_superuser(auth.accounts(), &*store, "OPS@example.com", "other")
        .await
        .unwrap();
    assert_eq!(again, SuperuserOutcome::Promoted(id.clone()));
    assert!(auth
        .accounts()
        .login("ops@example.com", "first passphrase")
        .await
        .is_ok());
}

/// A permission is `Always` when held, `Never` when not, and a superuser
/// holds every one: a role check goes into a `condition()` as a constant.
#[cfg(feature = "policy")]
#[tokio::test]
async fn gate_is_a_constant_predicate() {
    use brocanth_policy::{Column, Pred};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Col {
        Id,
    }
    impl Column for Col {
        const ALL: &'static [Self] = &[Self::Id];
        fn name(&self) -> &'static str {
            "id"
        }
    }

    let auth = services();
    let ops = account(&auth, "ops@example.com").await;
    auth.store().set_superuser(&ops, true).await.unwrap();
    let ed = account(&auth, "ed@example.com").await;
    auth.store()
        .put_role(&Role::new("editor", ["post.change"]))
        .await
        .unwrap();
    auth.store().assign_role(&ed, "editor").await.unwrap();
    let ops = Staff::load(&**auth.store(), &ops).await.unwrap().unwrap();
    let ed = Staff::load(&**auth.store(), &ed).await.unwrap().unwrap();

    assert_eq!(ed.gate::<Col>("post.change"), Pred::Always);
    assert_eq!(ed.gate::<Col>("post.delete"), Pred::Never);
    assert_eq!(ops.gate::<Col>("post.delete"), Pred::Always);
    assert_eq!(
        ed.gate::<Col>("post.delete").or(Pred::Always),
        Pred::Always,
        "a row rule still opens what the role does not"
    );
}
