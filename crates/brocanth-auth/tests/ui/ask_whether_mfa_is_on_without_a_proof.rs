//! "Does this address have two-factor authentication?" says the address is
//! registered and tells a phisher which prompt to build. Asking needs a proof
//! that a primary factor already landed, not an account id.

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::identity::AccountId;
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::mfa::Mfa;

fn main() {
    let mfa: Mfa<MemoryStore, TestClock> =
        Mfa::new(Arc::new(MemoryStore::new()), Arc::new(TestClock::default())).unwrap();

    let _ = mfa.second_factors(&AccountId::new("ada"));
}
