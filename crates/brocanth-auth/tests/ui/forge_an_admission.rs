//! `Admitted` has no public constructor and no public fields, so there is no
//! way to assert "the policy said yes" without the policy having said yes.

use brocanth_auth::identity::AccountId;
use brocanth_auth::mfa::{Admitted, FactorKind, FactorSet};

fn main() {
    let _ = Admitted {
        account: AccountId::new("ada"),
        factors: FactorSet::of([FactorKind::Password]),
    };
}
