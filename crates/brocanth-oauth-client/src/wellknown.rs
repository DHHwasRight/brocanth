//! The OAuth endpoints of well-known providers, as plain data.
//!
//! This module is the one place that writes these URLs down. It needs no
//! feature and no dependency, so `brocanth-auth` reads it for its sign-in
//! catalog without linking reqwest, and [`crate::endpoints`] builds the
//! connect endpoints from it. Change a URL here and both follow.
//!
//! A `const` is a URL that never changes. A function builds a URL that
//! depends on a value: a Microsoft tenant, a self-managed GitLab.

/// GitHub's consent page.
pub const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
/// GitHub's token endpoint.
pub const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";

/// Google's consent page.
pub const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Google's token endpoint.
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
/// Google's RFC 7009 revocation endpoint.
pub const GOOGLE_REVOKE: &str = "https://oauth2.googleapis.com/revoke";

/// GitLab.com's base URL. [`gitlab_authorize`] and [`gitlab_token`] add the
/// paths.
pub const GITLAB_BASE: &str = "https://gitlab.com";
/// GitLab.com's consent page.
pub const GITLAB_AUTHORIZE: &str = "https://gitlab.com/oauth/authorize";
/// GitLab.com's token endpoint.
pub const GITLAB_TOKEN: &str = "https://gitlab.com/oauth/token";

/// Microsoft's consent page for the `common` tenant.
pub const MICROSOFT_AUTHORIZE: &str =
    "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
/// Microsoft's token endpoint for the `common` tenant.
pub const MICROSOFT_TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// The consent page of a self-managed GitLab at `base`.
pub fn gitlab_authorize(base: &str) -> String {
    format!("{}/oauth/authorize", base.trim_end_matches('/'))
}

/// The token endpoint of a self-managed GitLab at `base`.
pub fn gitlab_token(base: &str) -> String {
    format!("{}/oauth/token", base.trim_end_matches('/'))
}

/// The revocation endpoint of a self-managed GitLab at `base`.
pub fn gitlab_revoke(base: &str) -> String {
    format!("{}/oauth/revoke", base.trim_end_matches('/'))
}

/// Microsoft's consent page for one tenant: `common`, `organizations`,
/// `consumers` or a tenant id.
pub fn microsoft_authorize(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize")
}

/// Microsoft's token endpoint for one tenant.
pub fn microsoft_token(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `const` forms and the function forms are the same URLs.
    #[test]
    fn the_constants_match_the_functions() {
        assert_eq!(GITLAB_AUTHORIZE, gitlab_authorize(GITLAB_BASE));
        assert_eq!(GITLAB_TOKEN, gitlab_token(GITLAB_BASE));
        assert_eq!(MICROSOFT_AUTHORIZE, microsoft_authorize("common"));
        assert_eq!(MICROSOFT_TOKEN, microsoft_token("common"));
    }
}
