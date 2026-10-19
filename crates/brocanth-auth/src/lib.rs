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
//! There is no database here, no SQL, no ORM and no pool. [`AuthStore`] is the
//! whole storage contract — find an identity, create one, link a provider,
//! store and revoke a session, store and consume a single-use token — and
//! [`MemoryStore`] implements it for tests. Second factors, one-time codes and
//! login alerts each add a *separate* trait on top of it ([`MfaStore`],
//! [`OtpStore`], [`SignInLocationStore`]), so a product that wants none of them
//! implements none of them.
//!
//! [`MfaStore`]: storage::MfaStore
//! [`OtpStore`]: storage::OtpStore
//! [`SignInLocationStore`]: storage::SignInLocationStore An adapter over `brocanth-model`
//! is a later, separate crate, and it changes nothing above this line.
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

pub mod accounts;
pub mod alerts;
pub mod apple;
pub mod clock;
pub mod error;
pub mod http_client;
pub mod identity;
pub mod impersonation;
pub mod linking;
pub mod memory;
pub mod mfa;
pub mod oauth;
pub mod otp;
pub mod password;
pub mod plugin;
pub mod providers;
pub mod random;
pub mod recovery;

/// The `brocanth create` module that writes this crate into a new project.
///
/// Behind the `scaffold` feature, which is off: the generator is the CLI's
/// business and not an application's.
#[cfg(feature = "scaffold")]
pub mod scaffold;

pub mod session;
pub mod storage;
pub mod tokens;
pub mod totp;

pub use accounts::{Accounts, Authenticated, LoginRejection, RegistrationAccepted, ResetRequested};
pub use alerts::{LoginAlert, LoginAlerts};
pub use clock::{Clock, SystemClock, TestClock, UnixTime};
pub use error::AuthError;
pub use http_client::{HttpClient, HttpRequest, HttpResponse};
pub use identity::{Account, AccountId, EmailAddress, ProviderIdentity, ProviderLink};
pub use impersonation::{AccountHolder, Impersonation, Restricted};
pub use linking::{LinkRefusal, Linking, SignInOutcome};
pub use memory::MemoryStore;
pub use mfa::{
    Admission, Admitted, Elevation, Enrolled, FactorClass, FactorKind, FactorPolicy, FactorSet,
    Mfa, RecoveryAccepted, SecondFactorRequirement, TotpEnrolment, Verified,
};
pub use oauth::{
    AuthorizationRequest, CallbackParams, ClientConfig, ClientSecret, FlowState, OauthClient,
    TokenResponse,
};
pub use otp::{Otp, OtpChallengeId, OtpCode, OtpDelivery, OtpRequested};
pub use password::{PasswordCost, Passwords};
pub use plugin::{Auth, AuthConfig, AuthPlugin};
pub use providers::{Provider, ProviderKind};
pub use recovery::{RecoveryCode, RecoveryCodes};
pub use session::{Privilege, SameSite, SessionConfig, SessionId, Sessions, SetCookie, SigningKey};
pub use storage::{
    AuthStore, MfaStore, NewAccount, NewProviderLink, OtpChallengeRecord, OtpStore, SessionRecord,
    SignInLocationStore, StoreError, TokenRecord, TotpSecretRecord,
};
pub use tokens::{IssuedToken, TokenPurpose, Tokens};
pub use totp::{Totp, TotpSecret};
