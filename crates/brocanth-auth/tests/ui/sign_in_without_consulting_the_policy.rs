//! The bypass this whole chain exists to prevent: check a password, then open a
//! session, never asking whether the account owed a second factor.
//!
//! Under the old signature — `begin(&account, Privilege::Authenticated)` —
//! this compiled, shipped, and silently turned MFA off for everybody.

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::identity::AccountId;
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::mfa::{FactorKind, FactorSet, Verified};
use brocanth_auth::session::{SessionConfig, Sessions, SigningKey};

fn main() {
    let sessions: Sessions<MemoryStore, TestClock> = Sessions::new(
        Arc::new(MemoryStore::new()),
        Arc::new(TestClock::default()),
        SigningKey::from_bytes([0u8; 32]),
        SessionConfig::default(),
    );
    // Stands in for whatever `Accounts::login` just handed back.
    let proof = Verified::without_a_credential_check(
        AccountId::new("ada"),
        FactorSet::of([FactorKind::Password]),
    );

    // A verified password is not an admission. `FactorPolicy::admit` is the
    // only thing that turns one into the other.
    let _ = sessions.begin_with_factors(proof, None);
}
