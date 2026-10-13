//! A rotation re-mints an identifier; it is not a place to acquire an identity.
//! There is no privilege argument anywhere in `Sessions`, so an anonymous
//! session cannot be rotated into an authenticated one.

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::session::{Privilege, SessionConfig, SessionId, Sessions, SigningKey};

fn main() {
    let sessions: Sessions<MemoryStore, TestClock> = Sessions::new(
        Arc::new(MemoryStore::new()),
        Arc::new(TestClock::default()),
        SigningKey::from_bytes([0u8; 32]),
        SessionConfig::default(),
    );

    let _ = sessions.rotate(&SessionId::new("whatever"), Privilege::Authenticated);
}
