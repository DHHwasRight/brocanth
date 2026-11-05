#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! Authentication for brocanth: one interface, many providers, and a local
//! account path that does not leak who has one.
//!
//! # Shape
//!
//! The Go reference points are `golang.org/x/oauth2` — the protocol, with a
//! per-provider endpoint configuration — and `markbates/goth`, which puts many
//! providers behind one interface. This crate copies that shape and keeps the
//! better half of each: a [`Provider`] is **data**, not code. Endpoints,
//! scopes, and a mapping from the provider's profile JSON to a common
//! [`ProviderIdentity`]. Adding a provider is a struct literal
//! ([`providers`]), not a module, and the flow code never learns a provider's
//! name.
//!
//! One provider does not fit, and it is [`apple`]. Apple signs its client
//! secret with an ES256 JWT you generate from a private key, has no userinfo
//! endpoint, and returns the user's name exactly once — on the first
//! authorization, in a form field, never again. That is handled, and the
//! reasons are written next to the code.
//!
//! # Security decisions, each with its reason
//!
//! * **PKCE is mandatory.** There is no option to turn it off and no code path
//!   that omits it. A public client without PKCE is an authorization-code
//!   interception waiting to happen, and since OAuth 2.1 there is no remaining
//!   reason to support the alternative. See [`oauth::pkce`].
//! * **`state` is compared in constant time** and the comparison is the first
//!   thing [`oauth::OauthClient::complete`] does. CSRF on the callback is the
//!   attack; an early return on the first differing byte is the leak.
//! * **`nonce` is required for every OIDC provider** and is checked against
//!   the flow state, which defeats id_token replay.
//! * **argon2id, and no choice is offered.** A configurable hash function is a
//!   configuration mistake waiting to happen — the only question a deployment
//!   gets to answer is cost, and even that is a named constructor rather than
//!   a free-form string. See [`password`].
//! * **Sessions rotate on privilege change.** The identifier a user arrives
//!   with is never the identifier they keep after they authenticate or
//!   elevate. That is the session-fixation defence, and it is the whole of it.
//! * **A matching email never links automatically.** A provider that says
//!   `alice@example.com` has not proved Alice consented; several real products
//!   have shipped exactly that account takeover. See [`linking`].
//! * **Login, registration, password reset and one-time codes do not reveal
//!   whether an address exists** — not in the response shape and not in the
//!   timing. See [`accounts`] and [`otp`].
//! * **Authentication is a set of factors, not a boolean.** [`FactorSet`] says
//!   what was proved, [`FactorPolicy`] says what is enough, and the session
//!   keeps the set. Adding passkeys later is a new variant rather than a new
//!   flow. See [`mfa`].
//! * **A sign-in that skipped the second factor does not compile.** Checking a
//!   credential mints a [`Verified`]; the policy turns one into an [`Admitted`],
//!   which has no public constructor; and opening a session consumes the
//!   [`Admitted`] by value. §9's rule for `Verified` — *"must be consumed, not
//!   produced"* — one layer down, because a second factor a handler can forget
//!   is a second factor some handler eventually will forget. See [`mfa`].
//! * **A TOTP code is spent once, not once per thirty seconds.** The step the
//!   code belongs to is claimed atomically, which is the half of RFC 6238 most
//!   implementations leave out. See [`totp`].
//! * **An impersonated session says so, permanently.** Both identities are on
//!   the session row, an operator cannot change the password, manage the second
//!   factors, elevate, or impersonate again — each refused by a type and not by
//!   a comment. See [`impersonation`].
//!
//! [`FactorSet`]: mfa::FactorSet
//! [`FactorPolicy`]: mfa::FactorPolicy
//! [`Verified`]: mfa::Verified
//! [`Admitted`]: mfa::Admitted
//!
//! # Nothing is assumed about storage
//!
//! [`AuthStore`] is the whole storage contract — find an identity, create one,
//! link a provider, store and revoke a session, store and consume a single-use
//! token. Second factors, one-time codes and login alerts each add a
//! *separate* trait on top of it ([`MfaStore`], [`OtpStore`],
//! [`SignInLocationStore`]), so a product that wants none of them implements
//! none of them.
//!
//! Two stores ship:
//!
//! * [`MemoryStore`], a `Mutex<HashMap>`, for tests and a first run. Nothing
//!   in it survives the process.
//! * `PgStore`, behind the `postgres` feature, which is off. It implements
//!   every trait over Postgres, so accounts, sessions, second factors,
//!   one-time codes, impersonation markers and login-alert history survive a
//!   restart. The `postgres` module says what each row holds and how each
//!   single-use operation stays atomic.
//!
//! Both pass the same suite: the `conformance` feature turns it on, and an
//! adapter of your own can run it too.
//!
//! [`MfaStore`]: storage::MfaStore
//! [`OtpStore`]: storage::OtpStore
//! [`SignInLocationStore`]: storage::SignInLocationStore
//!
//! # Nothing is assumed about HTTP either
//!
//! [`HttpClient`] is a two-method trait. `reqwest` implements it behind the
//! default `reqwest-client` feature; an application that already holds a
//! client turns the feature off and implements the trait, and reqwest then
//! appears nowhere in its lock file.
//!
//! [`AuthStore`]: storage::AuthStore
//! [`HttpClient`]: http_client::HttpClient
//! [`MemoryStore`]: memory::MemoryStore
//! [`Provider`]: providers::Provider
//! [`ProviderIdentity`]: identity::ProviderIdentity

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
pub mod accounts;

/// The **Accounts** tab in `brocanth-admin`.
///
/// Behind the `admin` feature, which is off. An application that has accounts
/// and no backoffice links neither the admin nor rsx, and a crate that keeps
/// its dependency list to four does not grow a password hasher.
#[cfg(feature = "admin")]
#[cfg(feature = "runtime")]
pub mod admin;

/// The tab's screens, compiled in by rsx at macro expansion: nothing is read
/// from disk at run time.
///
/// At the crate root and not inside [`admin`], because a template's own `use`
/// line names this path. Private, like the admin's own template module.
#[cfg(feature = "admin")]
#[cfg(feature = "runtime")]
mod admin_templates {
    rsx::load_dir!("src/admin_templates");
}

#[cfg(feature = "runtime")]
pub mod alerts;
#[cfg(feature = "runtime")]
pub mod apple;
#[cfg(feature = "runtime")]
pub mod clock;

/// The storage contract as an executable suite, for `MemoryStore`, `PgStore`
/// and any other adapter.
///
/// Behind the `conformance` feature, which only a test binary turns on.
#[cfg(feature = "conformance")]
#[cfg(feature = "runtime")]
pub mod conformance;

#[cfg(feature = "runtime")]
pub mod error;
#[cfg(feature = "runtime")]
pub mod http_client;
#[cfg(feature = "runtime")]
pub mod identity;
#[cfg(feature = "runtime")]
pub mod impersonation;
#[cfg(feature = "runtime")]
pub mod linking;
#[cfg(feature = "runtime")]
pub mod memory;
#[cfg(feature = "runtime")]
pub mod mfa;
#[cfg(feature = "runtime")]
pub mod oauth;
#[cfg(feature = "runtime")]
pub mod otp;
#[cfg(feature = "runtime")]
pub mod password;
#[cfg(feature = "runtime")]
pub mod plugin;

/// [`PgStore`], every storage trait over Postgres.
///
/// Behind the `postgres` feature, which is off. With it off, neither this
/// module nor `sqlx` is in the build.
#[cfg(feature = "postgres")]
#[cfg(feature = "runtime")]
pub mod postgres;

#[cfg(feature = "runtime")]
pub mod providers;
#[cfg(feature = "runtime")]
pub mod random;
#[cfg(feature = "runtime")]
pub mod recovery;

/// The `brocanth create` module that writes this crate into a new project.
///
/// Behind the `scaffold` feature, which is off: the generator is the CLI's
/// business and not an application's.
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub mod session;
#[cfg(feature = "runtime")]
pub mod storage;
#[cfg(feature = "runtime")]
pub mod tokens;
#[cfg(feature = "runtime")]
pub mod totp;

#[cfg(feature = "runtime")]
pub use accounts::{Accounts, Authenticated, LoginRejection, RegistrationAccepted, ResetRequested};
#[cfg(feature = "runtime")]
pub use alerts::{LoginAlert, LoginAlerts};
#[cfg(feature = "runtime")]
pub use clock::{Clock, SystemClock, TestClock, UnixTime};
#[cfg(feature = "runtime")]
pub use error::AuthError;
#[cfg(feature = "runtime")]
pub use http_client::{HttpClient, HttpRequest, HttpResponse};
#[cfg(feature = "runtime")]
pub use identity::{Account, AccountId, EmailAddress, ProviderIdentity, ProviderLink};
#[cfg(feature = "runtime")]
pub use impersonation::{AccountHolder, Impersonation, Restricted};
#[cfg(feature = "runtime")]
pub use linking::{LinkRefusal, Linking, SignInOutcome};
#[cfg(feature = "runtime")]
pub use memory::MemoryStore;
#[cfg(feature = "runtime")]
pub use mfa::{
    Admission, Admitted, Elevation, Enrolled, FactorClass, FactorKind, FactorPolicy, FactorSet,
    Mfa, RecoveryAccepted, SecondFactorRequirement, TotpEnrolment, Verified,
};
#[cfg(feature = "runtime")]
pub use oauth::{
    AuthorizationRequest, CallbackParams, ClientConfig, ClientSecret, FlowState, OauthClient,
    TokenResponse,
};
#[cfg(feature = "runtime")]
pub use otp::{Otp, OtpChallengeId, OtpCode, OtpDelivery, OtpRequested};
#[cfg(feature = "runtime")]
pub use password::{PasswordCost, Passwords};
#[cfg(feature = "runtime")]
pub use plugin::{Auth, AuthConfig, AuthPlugin};
#[cfg(feature = "postgres")]
#[cfg(feature = "runtime")]
pub use postgres::{PgStore, Swept};
#[cfg(feature = "runtime")]
pub use providers::{Provider, ProviderKind};
#[cfg(feature = "runtime")]
pub use recovery::{RecoveryCode, RecoveryCodes};
#[cfg(feature = "runtime")]
pub use session::{Privilege, SameSite, SessionConfig, SessionId, Sessions, SetCookie, SigningKey};
#[cfg(feature = "runtime")]
pub use storage::{
    AccountDirectory, AuthStore, MfaStore, NewAccount, NewProviderLink, OtpChallengeRecord,
    OtpStore, SessionRecord, SignInLocationStore, StoreError, TokenRecord, TotpSecretRecord,
};
#[cfg(feature = "runtime")]
pub use tokens::{IssuedToken, TokenPurpose, Tokens};
#[cfg(feature = "runtime")]
pub use totp::{Totp, TotpSecret};
