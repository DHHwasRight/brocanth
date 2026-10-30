//! `MemoryStore` against the storage conformance suite.
//!
//! The same cases run against `PgStore` in `tests/postgres_store.rs`. The two
//! stores pass one suite, so a test that passes on `MemoryStore` says
//! something about Postgres too.

use std::sync::Arc;

use brocanth_auth::storage::AccountDirectory;
use brocanth_auth::MemoryStore;

async fn fresh() -> Option<Arc<MemoryStore>> {
    Some(Arc::new(MemoryStore::new()))
}

brocanth_auth::auth_store_conformance!(fresh);
brocanth_auth::account_directory_conformance!(fresh);
brocanth_auth::mfa_store_conformance!(fresh);
brocanth_auth::otp_store_conformance!(fresh);
brocanth_auth::sign_in_location_store_conformance!(fresh);

#[test]
fn a_memory_store_says_it_is_not_durable() {
    assert!(!MemoryStore::new().is_durable());
}
