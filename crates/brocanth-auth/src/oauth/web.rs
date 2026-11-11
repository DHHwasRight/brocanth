//! The provider sign-in as routes: the button page, the redirect, the
//! callback and account linking, with no web framework assumed.
//!
//! [`OauthRoutes::answer`] takes the parts of a request it reads
//! ([`OauthRequest`]) and returns what to send ([`OauthAnswer`]). The
//! application turns one into the other in a few lines, and renders an
//! [`OauthPage`] with [`OauthPage::html`] or with its own templates.
//!
//! # The routes, under a prefix such as `/oauth`
//!
//! | Route | Does |
//! |---|---|
//! | `GET /oauth/{provider}` | a page with one button, "Continue with GitHub" |
//! | `POST /oauth/{provider}` | starts the flow: a flow cookie, then 303 to the provider |
//! | `GET` or `POST /oauth/{provider}/callback` | finishes it; `POST` is Apple's `form_post` |
//! | `GET /oauth/{provider}/link` | asks a signed-in person to confirm a link |
//! | `POST /oauth/{provider}/link` | links the provider to the signed-in account |
//!
//! **The flow starts with a POST.** A link that starts a sign-in can be
//! fired by any page on the web, so the button is a form, and a CSRF hook
//! protects it like any other form. The GET page exists so a plain link
//! still works without script.
//!
//! # State and PKCE
//!
//! [`OauthClient::begin`] makes the `state`, the `nonce` and the PKCE
//! verifier. They travel in a signed cookie (`HttpOnly`, ten minutes,
//! `SameSite=Lax` because the callback is a cross-site navigation), never in
//! a table: a row per started sign-in would give a stranger a write on the
//! database. The callback checks the state in constant time before anything
//! leaves the process. See [`crate::oauth`].
//!
//! # Linking, and the one rule
//!
//! A provider joins an existing account only by a POST from a session that
//! is already signed in as that account (see [`crate::linking`]). So:
//!
//! * A callback whose address belongs to an account, from somebody who is
//!   not signed in, keeps the provider identity in a second signed cookie
//!   and answers [`OauthPage::LinkFirst`]: sign in the usual way, then come
//!   back to `/oauth/{provider}/link`.
//! * A callback from somebody who is signed in and whose provider is not
//!   linked yet answers [`OauthPage::Link`], a page with a "Connect" button.
//!   The callback never links on its own: a page that tricked a signed-in
//!   person into finishing a flow would otherwise attach the trickster's
//!   provider account to the victim's.
//!
//! # Strict session cookies
//!
//! The callback is the answer to a cross-site navigation, so a
//! `SameSite=Strict` session cookie set there is stored and not sent on the
//! redirect that follows. The callback therefore answers a page,
//! [`OauthPage::SignedIn`], that moves on by itself: that navigation starts
//! on this site, and the cookie goes with it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::http_client::HttpClient;
use crate::identity::{EmailAddress, ProviderIdentity};
use crate::linking::SignInOutcome;
use crate::mfa::{Admission, FactorPolicy, Mfa, Verified};
use crate::oauth::{parse_pairs, AuthorizeOptions, CallbackParams, FlowState, OauthClient};
use crate::plugin::{Auth, ConfiguredProvider};
use crate::providers::Quirk;
use crate::random::{b64, ct_eq, unb64};
use crate::session::{Privilege, SameSite, SetCookie, SigningKey};
use crate::storage::{MfaStore, SessionRecord};

/// How long a started flow and a pending link last, in seconds.
const TEN_MINUTES: i64 = 600;

/// The parts of a request the routes read.
#[derive(Clone, Copy, Debug, Default)]
pub struct OauthRequest<'a> {
    /// Whether the method is POST. Anything else is read as GET.
    pub post: bool,
    /// The path, with the prefix: `/oauth/github/callback`.
    pub path: &'a str,
    /// The query string, without the `?`.
    pub query: &'a str,
    /// The body, for a POST: an urlencoded form.
    pub body: &'a str,
    /// The `Cookie` header, or `""`.
    pub cookies: &'a str,
}

/// What to send back.
#[derive(Debug)]
pub enum OauthAnswer {
    /// `303 See Other` to `location`, with these cookies.
    Redirect {
        /// Where to.
        location: String,
        /// `Set-Cookie` values.
        cookies: Vec<SetCookie>,
    },
    /// A page, with this status and these cookies.
    Page {
        /// The HTTP status.
        status: u16,
        /// What to say.
        page: OauthPage,
        /// `Set-Cookie` values.
        cookies: Vec<SetCookie>,
    },
}

impl OauthAnswer {
    fn page(status: u16, page: OauthPage) -> Self {
        Self::Page {
            status,
            page,
            cookies: Vec::new(),
        }
    }

    fn refused(status: u16, message: impl Into<String>) -> Self {
        Self::page(
            status,
            OauthPage::Refused {
                message: message.into(),
            },
        )
    }
}

/// A page the routes answer. Every string is plain text; [`OauthPage::html`]
/// escapes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OauthPage {
    /// One button that starts the flow.
    Continue {
        /// "GitHub".
        provider: String,
        /// Where the button posts.
        action: String,
    },
    /// Signed in: go on to `location`. See the module documentation for why
    /// this is a page and not a redirect.
    SignedIn {
        /// Where to go.
        location: String,
    },
    /// The provider's address belongs to an account, and nobody is signed
    /// in. Sign in the usual way, then confirm at `link`.
    LinkFirst {
        /// "GitHub".
        provider: String,
        /// The application's sign-in page, with `next` pointing at `link`,
        /// when the application has one.
        sign_in: Option<String>,
        /// Where the confirmation is.
        link: String,
    },
    /// Signed in, with a provider account that is not linked yet: confirm.
    Link {
        /// "GitHub".
        provider: String,
        /// The provider account's address, when it gave one.
        email: Option<String>,
        /// Where the button posts.
        action: String,
    },
    /// The flow failed, or the request makes no sense.
    Refused {
        /// What went wrong, in a sentence a visitor may read.
        message: String,
    },
    /// No provider has that id, or no route has that path.
    NotFound,
}

impl OauthPage {
    /// The page as a small HTML document. Replace it with your own templates
    /// by matching on the variants.
    pub fn html(&self) -> String {
        let (title, body) = match self {
            OauthPage::Continue { provider, action } => (
                format!("Continue with {}", escape(provider)),
                format!(
                    "<form method=\"post\" action=\"{}\"><button type=\"submit\">Continue with {}</button></form>",
                    escape(action),
                    escape(provider)
                ),
            ),
            OauthPage::SignedIn { location } => (
                "Signed in".to_string(),
                format!(
                    "<meta http-equiv=\"refresh\" content=\"0;url={0}\"><p>You are signed in. <a href=\"{0}\">Continue</a></p>",
                    escape(location)
                ),
            ),
            OauthPage::LinkFirst {
                provider,
                sign_in,
                link,
            } => (
                format!("Connect {}", escape(provider)),
                format!(
                    "<p>An account already uses the address {provider} gave. To use {provider} with it, sign in the usual way first, then connect {provider}.</p><p>{}<a href=\"{}\">Connect {provider}</a></p>",
                    sign_in.as_ref().map_or_else(String::new, |href| format!(
                        "<a href=\"{}\">Sign in</a> · ",
                        escape(href)
                    )),
                    escape(link),
                    provider = escape(provider),
                ),
            ),
            OauthPage::Link {
                provider,
                email,
                action,
            } => (
                format!("Connect {}", escape(provider)),
                format!(
                    "<p>Connect the {provider} account{} to the account you are signed in to? You can then sign in with {provider}.</p><form method=\"post\" action=\"{}\"><button type=\"submit\">Connect {provider}</button></form>",
                    email.as_ref().map_or_else(String::new, |e| format!(" {}", escape(e))),
                    escape(action),
                    provider = escape(provider),
                ),
            ),
            OauthPage::Refused { message } => (
                "Sign-in failed".to_string(),
                format!("<p>{}</p>", escape(message)),
            ),
            OauthPage::NotFound => ("Not found".to_string(), "<p>Not found.</p>".to_string()),
        };
        format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title></head><body><h1>{title}</h1>{body}</body></html>\n"
        )
    }
}

/// The routes, over one set of providers.
pub struct OauthRoutes<H: HttpClient, C: Clock> {
    client: OauthClient<H, C>,
    providers: Vec<ConfiguredProvider>,
    prefix: String,
    after_sign_in: String,
    sign_in: Option<String>,
}

impl<H: HttpClient, C: Clock> OauthRoutes<H, C> {
    /// The routes at `/oauth`, sending a signed-in person to `/`.
    pub fn new(http: Arc<H>, clock: Arc<C>, providers: Vec<ConfiguredProvider>) -> Self {
        Self {
            client: OauthClient::new(http, clock),
            providers,
            prefix: "/oauth".to_string(),
            after_sign_in: "/".to_string(),
            sign_in: None,
        }
    }

    /// Mounts the routes under another prefix. No trailing slash.
    #[must_use]
    pub fn at(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into().trim_end_matches('/').to_string();
        self
    }

    /// Where a person goes after signing in or linking.
    #[must_use]
    pub fn after_sign_in(mut self, path: impl Into<String>) -> Self {
        self.after_sign_in = path.into();
        self
    }

    /// The application's own sign-in page, for a person who must sign in
    /// before a link. The routes add `?next=<the link page>`.
    #[must_use]
    pub fn sign_in_page(mut self, path: impl Into<String>) -> Self {
        self.sign_in = Some(path.into());
        self
    }

    /// The configured providers, for the buttons on a sign-in page.
    pub fn providers(&self) -> &[ConfiguredProvider] {
        &self.providers
    }

    /// The path of a provider's button page, which is also where it posts.
    pub fn start_path(&self, provider: &str) -> String {
        format!("{}/{provider}", self.prefix)
    }

    /// The redirect URI to register at the provider's console: `base_url`
    /// (scheme and host) and the callback path.
    pub fn callback_url(base_url: &str, prefix: &str, provider: &str) -> String {
        format!(
            "{}{}/{provider}/callback",
            base_url.trim_end_matches('/'),
            prefix.trim_end_matches('/')
        )
    }

    /// Answers one request under the prefix.
    ///
    /// `auth` is the application's services: its sessions, its linking and
    /// its store. The store must hold second factors too, so an account with
    /// a TOTP secret is not signed in by a provider alone.
    ///
    /// # Errors
    /// Fails when the store fails. A failed flow (a refusal at the provider,
    /// a wrong state, an expired flow) is a page, not an error.
    pub async fn answer<S, H2, C2>(
        &self,
        auth: &Auth<S, H2, C2>,
        request: OauthRequest<'_>,
    ) -> Result<OauthAnswer, AuthError>
    where
        S: MfaStore,
        H2: HttpClient,
        C2: Clock,
    {
        let Some(rest) = request
            .path
            .strip_prefix(self.prefix.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            return Ok(OauthAnswer::page(404, OauthPage::NotFound));
        };
        let (id, step) = rest.split_once('/').unwrap_or((rest, ""));
        let Some(configured) = self.providers.iter().find(|p| p.provider.id == id) else {
            return Ok(OauthAnswer::page(404, OauthPage::NotFound));
        };
        let cookies = Cookies::new(auth);
        match (step, request.post) {
            ("", false) => Ok(OauthAnswer::page(
                200,
                OauthPage::Continue {
                    provider: configured.provider.display_name.to_string(),
                    action: self.start_path(id),
                },
            )),
            ("", true) => self.begin(configured, &cookies, auth),
            ("callback", _) => self.callback(auth, configured, &cookies, request).await,
            ("link", post) => self.link(auth, configured, &cookies, request, post).await,
            _ => Ok(OauthAnswer::page(404, OauthPage::NotFound)),
        }
    }

    fn begin<S: MfaStore, H2: HttpClient, C2: Clock>(
        &self,
        configured: &ConfiguredProvider,
        cookies: &Cookies,
        auth: &Auth<S, H2, C2>,
    ) -> Result<OauthAnswer, AuthError> {
        let started = self.client.begin(
            &configured.provider,
            &configured.client,
            &AuthorizeOptions::default(),
        )?;
        let sealed = started.flow.seal(auth.sessions().key())?;
        // Apple answers with a cross-site POST, which a Lax cookie does not
        // ride on. `None` is legal only with `Secure`.
        let same_site =
            if configured.provider.has(Quirk::NameOnFirstAuthorizationOnly) && cookies.secure {
                SameSite::None
            } else {
                SameSite::Lax
            };
        Ok(OauthAnswer::Redirect {
            location: started.url,
            cookies: vec![cookies.set(&cookies.flow, &sealed, same_site)],
        })
    }

    async fn callback<S, H2, C2>(
        &self,
        auth: &Auth<S, H2, C2>,
        configured: &ConfiguredProvider,
        cookies: &Cookies,
        request: OauthRequest<'_>,
    ) -> Result<OauthAnswer, AuthError>
    where
        S: MfaStore,
        H2: HttpClient,
        C2: Clock,
    {
        let clear_flow = cookies.clear(&cookies.flow);
        let refused = |message: &str| OauthAnswer::Page {
            status: 400,
            page: OauthPage::Refused {
                message: message.to_string(),
            },
            cookies: vec![clear_flow.clone()],
        };
        let Some(sealed) = crate::session::cookie(request.cookies, &cookies.flow) else {
            return Ok(refused(
                "This sign-in did not start in this browser, or it took longer than ten minutes. Start again.",
            ));
        };
        let Ok(flow) = FlowState::unseal(auth.sessions().key(), sealed) else {
            return Ok(refused("This sign-in is not valid. Start again."));
        };
        let params = parse_pairs(if request.post {
            request.body
        } else {
            request.query
        });
        let callback = CallbackParams::from_pairs(params);
        let signed_in = match self
            .client
            .complete(&configured.provider, &configured.client, &flow, &callback)
            .await
        {
            Ok(signed_in) => signed_in,
            Err(AuthError::ProviderRefused { .. }) => {
                return Ok(refused("The provider did not let you in."))
            }
            Err(AuthError::FlowExpired) => {
                return Ok(refused(
                    "This sign-in took longer than ten minutes. Start again.",
                ))
            }
            Err(AuthError::StateMismatch | AuthError::NonceMismatch | AuthError::InvalidToken) => {
                return Ok(refused("This sign-in is not valid. Start again."))
            }
            Err(other) => return Err(other),
        };
        let identity = signed_in.identity;
        let provider = configured.provider.display_name.to_string();

        // Somebody is signed in: this is a link, and a link needs a POST.
        if let Some(session) = signed_in_session(auth, request.cookies).await? {
            let already = auth
                .linking()
                .links(&session.account)
                .await?
                .iter()
                .any(|l| l.provider == identity.provider && l.subject == identity.subject);
            if already {
                return Ok(OauthAnswer::Page {
                    status: 200,
                    page: OauthPage::SignedIn {
                        location: self.after_sign_in.clone(),
                    },
                    cookies: vec![clear_flow],
                });
            }
            let pending = cookies.pending(&identity, auth.clock().now())?;
            return Ok(OauthAnswer::Page {
                status: 200,
                page: OauthPage::Link {
                    provider,
                    email: identity.email.as_ref().map(ToString::to_string),
                    action: self.link_path(&identity.provider),
                },
                cookies: vec![clear_flow, pending],
            });
        }

        let proof = match auth.linking().sign_in(identity).await? {
            SignInOutcome::SignedIn { proof, .. } | SignInOutcome::Registered { proof, .. } => {
                proof
            }
            SignInOutcome::LinkRequiresAuthentication { identity, .. } => {
                let link = self.link_path(&identity.provider);
                let pending = cookies.pending(&identity, auth.clock().now())?;
                return Ok(OauthAnswer::Page {
                    status: 409,
                    page: OauthPage::LinkFirst {
                        provider,
                        sign_in: self
                            .sign_in
                            .as_ref()
                            .map(|page| format!("{page}?next={}", encode(&link))),
                        link,
                    },
                    cookies: vec![clear_flow, pending],
                });
            }
        };
        let Some(started) = self.admit(auth, proof, request.cookies).await? else {
            return Ok(OauthAnswer::Page {
                status: 403,
                page: OauthPage::Refused {
                    message: "This account has a second factor. Sign in with your password and your code."
                        .to_string(),
                },
                cookies: vec![clear_flow],
            });
        };
        Ok(OauthAnswer::Page {
            status: 200,
            page: OauthPage::SignedIn {
                location: self.after_sign_in.clone(),
            },
            cookies: vec![started, clear_flow],
        })
    }

    /// The policy's answer for a provider sign-in, and the session when it
    /// admits. `None` when the account owes a second factor.
    async fn admit<S, H2, C2>(
        &self,
        auth: &Auth<S, H2, C2>,
        proof: Verified,
        cookies: &str,
    ) -> Result<Option<SetCookie>, AuthError>
    where
        S: MfaStore,
        H2: HttpClient,
        C2: Clock,
    {
        let mfa = Mfa::new(auth.store().clone(), auth.clock().clone())?;
        let enrolled = mfa.second_factors(&proof).await?;
        let Admission::Admit(admitted) = FactorPolicy::recommended().admit(&enrolled, proof) else {
            return Ok(None);
        };
        // The pre-login session this browser arrived with, if any, goes: the
        // session-fixation defence for this door.
        let before = match auth.sessions().from_cookie_header(cookies).await {
            Ok(session) => Some(session.id),
            Err(AuthError::NoSession) => None,
            Err(other) => return Err(other),
        };
        let started = auth
            .sessions()
            .begin_with_factors(admitted, before.as_ref())
            .await?;
        Ok(Some(started.cookie))
    }

    async fn link<S, H2, C2>(
        &self,
        auth: &Auth<S, H2, C2>,
        configured: &ConfiguredProvider,
        cookies: &Cookies,
        request: OauthRequest<'_>,
        post: bool,
    ) -> Result<OauthAnswer, AuthError>
    where
        S: MfaStore,
        H2: HttpClient,
        C2: Clock,
    {
        let provider = configured.provider.display_name.to_string();
        let pending = crate::session::cookie(request.cookies, &cookies.link)
            .and_then(|sealed| cookies.unseal_pending(sealed, auth.clock().now()))
            .filter(|identity| identity.provider == configured.provider.id);
        let Some(identity) = pending else {
            return Ok(OauthAnswer::refused(
                400,
                format!("Nothing is waiting to be connected. Sign in with {provider} again."),
            ));
        };
        let Some(session) = signed_in_session(auth, request.cookies).await? else {
            let link = self.link_path(&identity.provider);
            return Ok(OauthAnswer::page(
                401,
                OauthPage::LinkFirst {
                    provider,
                    sign_in: self
                        .sign_in
                        .as_ref()
                        .map(|page| format!("{page}?next={}", encode(&link))),
                    link,
                },
            ));
        };
        if !post {
            return Ok(OauthAnswer::page(
                200,
                OauthPage::Link {
                    provider,
                    email: identity.email.as_ref().map(ToString::to_string),
                    action: self.link_path(&identity.provider),
                },
            ));
        }
        match auth.linking().link(&session.account, &identity).await {
            Ok(_) => Ok(OauthAnswer::Redirect {
                location: self.after_sign_in.clone(),
                cookies: vec![cookies.clear(&cookies.link)],
            }),
            Err(AuthError::AlreadyLinked) => Ok(OauthAnswer::refused(
                409,
                format!("That {provider} account is connected to another account."),
            )),
            Err(other) => Err(other),
        }
    }

    fn link_path(&self, provider: &str) -> String {
        format!("{}/{provider}/link", self.prefix)
    }
}

/// The session a request carries, when it is signed in. A pre-login session
/// is not.
async fn signed_in_session<S: MfaStore, H: HttpClient, C: Clock>(
    auth: &Auth<S, H, C>,
    cookies: &str,
) -> Result<Option<SessionRecord>, AuthError> {
    match auth.sessions().from_cookie_header(cookies).await {
        Ok(session) if session.privilege != Privilege::Anonymous => Ok(Some(session)),
        Ok(_) | Err(AuthError::NoSession) => Ok(None),
        Err(other) => Err(other),
    }
}

/// The two cookies the routes write, named after the session cookie.
struct Cookies {
    flow: String,
    link: String,
    secure: bool,
    key: SigningKey,
}

impl Cookies {
    fn new<S: MfaStore, H: HttpClient, C: Clock>(auth: &Auth<S, H, C>) -> Self {
        let config = auth.sessions().config();
        // A `__Host-` session cookie gets `__Host-` siblings: the browser
        // then enforces `Secure`, a path of `/` and no `Domain` on them too.
        let (flow, link) = match config.cookie_name.strip_prefix("__Host-") {
            Some(_) => ("__Host-oauth".to_string(), "__Host-oauth_link".to_string()),
            None => (
                format!("{}_oauth", config.cookie_name),
                format!("{}_oauth_link", config.cookie_name),
            ),
        };
        Self {
            flow,
            link,
            secure: config.secure,
            key: auth.sessions().key().clone(),
        }
    }

    fn attributes(&self, same_site: SameSite) -> String {
        format!(
            "Path=/; HttpOnly{}; SameSite={}",
            if self.secure { "; Secure" } else { "" },
            same_site.as_str()
        )
    }

    fn set(&self, name: &str, value: &str, same_site: SameSite) -> SetCookie {
        SetCookie::raw(format!(
            "{name}={value}; Max-Age={TEN_MINUTES}; {}",
            self.attributes(same_site)
        ))
    }

    fn clear(&self, name: &str) -> SetCookie {
        SetCookie::raw(format!(
            "{name}=; Max-Age=0; {}",
            self.attributes(SameSite::Lax)
        ))
    }

    /// The provider identity, sealed into the pending-link cookie.
    fn pending(&self, identity: &ProviderIdentity, now: UnixTime) -> Result<SetCookie, AuthError> {
        let pending = Pending {
            provider: identity.provider.clone(),
            subject: identity.subject.clone(),
            email: identity.email.as_ref().map(ToString::to_string),
            email_verified: identity.email_verified,
            name: identity.name.clone(),
            avatar_url: identity.avatar_url.clone(),
            at: now,
        };
        let json = serde_json::to_vec(&pending)
            .map_err(|e| AuthError::crypto(format!("could not serialise a pending link: {e}")))?;
        let payload = b64(&json);
        let signature = b64(&self.key.sign_message(&tagged(&payload)));
        Ok(self.set(&self.link, &format!("{payload}.{signature}"), SameSite::Lax))
    }

    fn unseal_pending(&self, sealed: &str, now: UnixTime) -> Option<ProviderIdentity> {
        let (payload, signature) = sealed.rsplit_once('.')?;
        let presented = unb64(signature).ok()?;
        if !ct_eq(&presented, &self.key.sign_message(&tagged(payload))) {
            return None;
        }
        let pending: Pending = serde_json::from_slice(&unb64(payload).ok()?).ok()?;
        if now - pending.at > TEN_MINUTES {
            return None;
        }
        Some(ProviderIdentity {
            provider: pending.provider,
            subject: pending.subject,
            email: pending.email.and_then(|raw| EmailAddress::parse(&raw).ok()),
            email_verified: pending.email_verified,
            name: pending.name,
            avatar_url: pending.avatar_url,
            raw: serde_json::Value::Null,
        })
    }
}

/// What the pending-link cookie carries. The raw profile stays behind: a
/// cookie has four kilobytes, and linking reads none of it.
#[derive(Serialize, Deserialize)]
struct Pending {
    provider: String,
    subject: String,
    email: Option<String>,
    email_verified: bool,
    name: Option<String>,
    avatar_url: Option<String>,
    at: UnixTime,
}

/// The signed message of a pending link. The tag keeps a sealed flow state,
/// signed with the same key over its bare payload, from passing as one.
fn tagged(payload: &str) -> Vec<u8> {
    let mut message = b"oauth-link.".to_vec();
    message.extend_from_slice(payload.as_bytes());
    message
}

/// Percent-encodes a path for a query value.
fn encode(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

/// Escapes text for HTML, in an element or a quoted attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_escapes_what_it_prints() {
        let html = OauthPage::Refused {
            message: "<script>alert(1)</script>".to_string(),
        }
        .html();
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }

    #[test]
    fn the_callback_url_joins_without_a_double_slash() {
        assert_eq!(
            OauthRoutes::<crate::http_client::StubHttp, crate::clock::SystemClock>::callback_url(
                "https://example.com/",
                "/oauth/",
                "github"
            ),
            "https://example.com/oauth/github/callback"
        );
    }
}
