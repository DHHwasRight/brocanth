//! One proof, one session. `Verified` is not `Clone` and `admit` takes it by
//! value, so a captured password cannot be replayed into a second admission.

use brocanth_auth::identity::AccountId;
use brocanth_auth::mfa::{Enrolled, FactorKind, FactorPolicy, FactorSet, Verified};

fn main() {
    let account = AccountId::new("ada");
    let proof = Verified::without_a_credential_check(
        account.clone(),
        FactorSet::of([FactorKind::Password]),
    );
    let enrolled = Enrolled::without_asking_the_store(account, FactorSet::empty());
    let policy = FactorPolicy::recommended();

    let _first = policy.admit(&enrolled, proof);
    let _second = policy.admit(&enrolled, proof);
}
