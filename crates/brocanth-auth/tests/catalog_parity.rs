//! The sign-in catalog and `brocanth-oauth-client` hold the same OAuth
//! endpoints for GitHub, Google, GitLab and Microsoft (the `common` tenant and
//! a pinned one). Each crate keeps its own list
//! (this crate does not link the client unless `reqwest-client` is on), so
//! this test is what keeps the two from drifting: change a URL in one and
//! it fails until the other matches.

use brocanth_auth::providers::catalog::{microsoft_tenant, GITHUB, GITLAB, GOOGLE, MICROSOFT};
use brocanth_oauth_client::provider::endpoints;

#[test]
fn the_shared_endpoints_are_the_same_in_both_crates() {
    for (sign_in, connect) in [
        (GITHUB, endpoints::github()),
        (GOOGLE, endpoints::google()),
        (GITLAB, endpoints::gitlab()),
        (MICROSOFT, endpoints::microsoft()),
        (
            microsoft_tenant("contoso.onmicrosoft.com"),
            endpoints::microsoft_tenant("contoso.onmicrosoft.com"),
        ),
    ] {
        assert_eq!(
            sign_in.authorize_url.as_ref(),
            connect.auth_url,
            "{} authorize URL",
            sign_in.id
        );
        assert_eq!(
            sign_in.token_url.as_ref(),
            connect.token_url,
            "{} token URL",
            sign_in.id
        );
    }
}
