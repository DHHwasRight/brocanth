//! The provider sign-in as routes: the button, the redirect, the callback,
//! and the link that needs a signed-in session.

mod common;

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::http_client::{HttpResponse, StubHttp};
use brocanth_auth::mfa::{Admission, FactorPolicy};
use brocanth_auth::oauth::web::{OauthAnswer, OauthPage, OauthRequest, OauthRoutes};
use brocanth_auth::oauth::ClientConfig;
use brocanth_auth::plugin::{AuthConfig, ConfiguredProvider};
use brocanth_auth::providers::catalog::{GITHUB, GITLAB, GOOGLE};
use brocanth_auth::session::{SessionConfig, SigningKey};
use brocanth_auth::storage::AuthStore as _;
use brocanth_auth::{Auth, MemoryStore, PasswordCost, Passwords};
use serde_json::json;

type Services = Auth<MemoryStore, StubHttp, TestClock>;

fn services() -> Services {
    Auth::build(services_config()).unwrap()
}

fn services_config() -> AuthConfig<MemoryStore, StubHttp, TestClock> {
    AuthConfig {
        store: Arc::new(MemoryStore::new()),
        http: Arc::new(StubHttp::new()),
        clock: Arc::new(TestClock::at(1_700_000_000)),
        signing_key: SigningKey::from_bytes([3; 32]),
        session: SessionConfig {
            cookie_name: "session".to_string(),
            secure: false,
            ..SessionConfig::default()
        },
        passwords: Arc::new(Passwords::new(PasswordCost::insecure_for_tests()).unwrap()),
        providers: Vec::new(),
    }
}

fn json_answer(body: serde_json::Value) -> HttpResponse {
    HttpResponse::json(body.to_string())
}

/// GitHub, answering as the account with id `subject` and the primary
/// address `email`, verified or not.
fn github(subject: u64, email: &str, verified: bool) -> StubHttp {
    StubHttp::new()
        .on(
            http::Method::POST,
            "https://github.com/login/oauth/access_token",
            json_answer(json!({"access_token": "gho_x", "token_type": "bearer"})),
        )
        .on(
            http::Method::GET,
            "https://api.github.com/user/emails",
            json_answer(json!([{"email": email, "primary": true, "verified": verified}])),
        )
        .on(
            http::Method::GET,
            "https://api.github.com/user",
            json_answer(json!({"id": subject, "login": "octocat"})),
        )
}

/// The routes over one provider, a stubbed HTTP client and a clock.
fn routes_on(
    provider: brocanth_auth::providers::Provider,
    http: Arc<StubHttp>,
    clock: Arc<TestClock>,
) -> OauthRoutes<StubHttp, TestClock> {
    let callback = format!("https://app.test/oauth/{}/callback", provider.id);
    OauthRoutes::new(
        http,
        clock,
        vec![ConfiguredProvider {
            provider,
            client: ClientConfig::new("client-id", "client-secret", callback),
        }],
    )
    .sign_in_page("/sign-in")
}

/// GitHub, answering as the account with id `subject` and address `email`.
fn routes(subject: u64, email: &str) -> OauthRoutes<StubHttp, TestClock> {
    routes_on(
        GITHUB,
        Arc::new(github(subject, email, true)),
        Arc::new(TestClock::at(1_700_000_000)),
    )
}

fn get<'a>(path: &'a str, query: &'a str, cookies: &'a str) -> OauthRequest<'a> {
    OauthRequest {
        post: false,
        path,
        query,
        body: "",
        cookies,
    }
}

fn post<'a>(path: &'a str, cookies: &'a str) -> OauthRequest<'a> {
    OauthRequest {
        post: true,
        path,
        query: "",
        body: "",
        cookies,
    }
}

/// `name=value` out of each `Set-Cookie`, joined as a `Cookie` header, for
/// the cookies that are not being cleared.
fn jar(answer: &OauthAnswer) -> String {
    let cookies = match answer {
        OauthAnswer::Redirect { cookies, .. } | OauthAnswer::Page { cookies, .. } => cookies,
    };
    cookies
        .iter()
        .map(|c| c.as_str().split(';').next().unwrap().to_string())
        .filter(|pair| !pair.ends_with('='))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Presses the button and comes back from GitHub with a matching state.
async fn round_trip(
    auth: &Services,
    routes: &OauthRoutes<StubHttp, TestClock>,
    cookies: &str,
) -> OauthAnswer {
    let started = routes
        .answer(auth, post("/oauth/github", cookies))
        .await
        .unwrap();
    let OauthAnswer::Redirect { location, .. } = &started else {
        panic!("the button redirects to the provider: {started:?}");
    };
    assert!(location.starts_with("https://github.com/login/oauth/authorize?"));
    assert!(location.contains("code_challenge_method=S256"));
    let state = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let flow = jar(&started);
    assert!(flow.starts_with("session_oauth="), "{flow}");
    let both = if cookies.is_empty() {
        flow
    } else {
        format!("{cookies}; {flow}")
    };
    let query = format!("code=the-code&state={state}");
    routes
        .answer(auth, get("/oauth/github/callback", &query, &both))
        .await
        .unwrap()
}

/// A session opened with a password, as a `Cookie` header.
async fn password_session(auth: &Services, email: &str) -> String {
    auth.accounts()
        .register(email, "a long passphrase")
        .await
        .unwrap();
    let proof = auth
        .accounts()
        .login(email, "a long passphrase")
        .await
        .unwrap()
        .into_proof();
    let enrolled = brocanth_auth::mfa::Enrolled::without_asking_the_store(
        proof.account().clone(),
        brocanth_auth::FactorSet::empty(),
    );
    let Admission::Admit(admitted) = FactorPolicy::recommended().admit(&enrolled, proof) else {
        panic!("a password admits an account with no second factor");
    };
    let started = auth
        .sessions()
        .begin_with_factors(admitted, None)
        .await
        .unwrap();
    started
        .cookie
        .as_str()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_button_page_names_the_provider_and_posts_to_itself() {
    let auth = services();
    let answer = routes(1, "ada@example.com")
        .answer(&auth, get("/oauth/github", "", ""))
        .await
        .unwrap();
    let OauthAnswer::Page { status, page, .. } = answer else {
        panic!("a page");
    };
    assert_eq!(status, 200);
    assert_eq!(
        page,
        OauthPage::Continue {
            provider: "GitHub".to_string(),
            action: "/oauth/github".to_string()
        }
    );
    let html = page.html();
    assert!(
        html.contains("<form method=\"post\" action=\"/oauth/github\">"),
        "{html}"
    );
}

#[tokio::test]
async fn an_unknown_provider_is_not_found() {
    let auth = services();
    let answer = routes(1, "ada@example.com")
        .answer(&auth, get("/oauth/myspace", "", ""))
        .await
        .unwrap();
    assert!(matches!(
        answer,
        OauthAnswer::Page {
            status: 404,
            page: OauthPage::NotFound,
            ..
        }
    ));
}

#[tokio::test]
async fn a_new_person_is_registered_and_signed_in() {
    let auth = services();
    let answer = round_trip(&auth, &routes(4242, "new@example.com"), "").await;
    let OauthAnswer::Page {
        status,
        page,
        ref cookies,
    } = answer
    else {
        panic!("a page");
    };
    assert_eq!(status, 200);
    assert_eq!(
        page,
        OauthPage::SignedIn {
            location: "/".to_string()
        }
    );
    // The page moves on by itself, so a Strict session cookie rides along.
    assert!(page.html().contains("http-equiv=\"refresh\""));
    let session = cookies
        .iter()
        .find(|c| c.as_str().starts_with("session="))
        .expect("a session cookie");
    let header = session.as_str().split(';').next().unwrap();
    let loaded = auth.sessions().from_cookie_header(header).await.unwrap();
    let links = auth.linking().links(&loaded.account).await.unwrap();
    assert_eq!(links[0].provider, "github");
    assert_eq!(links[0].subject, "4242");
    assert!(
        cookies
            .iter()
            .any(|c| c.as_str().starts_with("session_oauth=;")),
        "the flow cookie is cleared"
    );
}

#[tokio::test]
async fn a_callback_without_the_flow_cookie_is_refused() {
    let auth = services();
    let answer = routes(1, "ada@example.com")
        .answer(&auth, get("/oauth/github/callback", "code=x&state=y", ""))
        .await
        .unwrap();
    assert!(matches!(
        answer,
        OauthAnswer::Page {
            status: 400,
            page: OauthPage::Refused { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn a_callback_with_another_state_is_refused() {
    let auth = services();
    let routes = routes(1, "ada@example.com");
    let started = routes
        .answer(&auth, post("/oauth/github", ""))
        .await
        .unwrap();
    let flow = jar(&started);
    let answer = routes
        .answer(
            &auth,
            get("/oauth/github/callback", "code=x&state=forged", &flow),
        )
        .await
        .unwrap();
    assert!(matches!(
        answer,
        OauthAnswer::Page {
            status: 400,
            page: OauthPage::Refused { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn a_taken_address_waits_for_a_signed_in_link() {
    let auth = services();
    let routes = routes(7, "ada@example.com");
    // Ada has a password account with the address GitHub reports.
    let ada = password_session(&auth, "ada@example.com").await;

    // Signed out, the callback does not sign in and does not link.
    let answer = round_trip(&auth, &routes, "").await;
    let pending = jar(&answer);
    let OauthAnswer::Page { status, page, .. } = answer else {
        panic!("a page");
    };
    assert_eq!(status, 409);
    assert_eq!(
        page,
        OauthPage::LinkFirst {
            provider: "GitHub".to_string(),
            sign_in: Some("/sign-in?next=%2Foauth%2Fgithub%2Flink".to_string()),
            link: "/oauth/github/link".to_string(),
        }
    );
    assert!(pending.starts_with("session_oauth_link="), "{pending}");

    // Signed in, the link page asks; the POST links.
    let both = format!("{ada}; {pending}");
    let asked = routes
        .answer(&auth, get("/oauth/github/link", "", &both))
        .await
        .unwrap();
    assert!(matches!(
        asked,
        OauthAnswer::Page {
            status: 200,
            page: OauthPage::Link { .. },
            ..
        }
    ));
    let linked = routes
        .answer(&auth, post("/oauth/github/link", &both))
        .await
        .unwrap();
    let OauthAnswer::Redirect { location, .. } = linked else {
        panic!("a redirect after the link: {linked:?}");
    };
    assert_eq!(location, "/");
    let account = auth
        .sessions()
        .from_cookie_header(&ada)
        .await
        .unwrap()
        .account;
    let links = auth.linking().links(&account).await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].subject, "7");
}

#[tokio::test]
async fn a_link_without_a_session_is_refused() {
    let auth = services();
    let routes = routes(7, "ada@example.com");
    password_session(&auth, "ada@example.com").await;
    let answer = round_trip(&auth, &routes, "").await;
    let pending = jar(&answer);
    let refused = routes
        .answer(&auth, post("/oauth/github/link", &pending))
        .await
        .unwrap();
    assert!(matches!(
        refused,
        OauthAnswer::Page {
            status: 401,
            page: OauthPage::LinkFirst { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn a_signed_in_callback_asks_before_it_links() {
    let auth = services();
    let routes = routes(9, "other@example.com");
    let ada = password_session(&auth, "ada@example.com").await;
    let answer = round_trip(&auth, &routes, &ada).await;
    assert!(
        matches!(
            answer,
            OauthAnswer::Page {
                status: 200,
                page: OauthPage::Link { .. },
                ..
            }
        ),
        "the callback must not link on its own: {answer:?}"
    );
    let account = auth
        .sessions()
        .from_cookie_header(&ada)
        .await
        .unwrap()
        .account;
    assert!(auth.linking().links(&account).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// The flow cookie: tampering, expiry, replay, refusal at the provider
// ---------------------------------------------------------------------------

/// Presses the button: the provider URL and the flow cookie, as a `Cookie`
/// header.
async fn start(auth: &Services, routes: &OauthRoutes<StubHttp, TestClock>) -> (url::Url, String) {
    let started = routes
        .answer(auth, post("/oauth/github", ""))
        .await
        .unwrap();
    let OauthAnswer::Redirect { location, .. } = &started else {
        panic!("the button redirects to the provider: {started:?}");
    };
    (url::Url::parse(location).unwrap(), jar(&started))
}

fn param(url: &url::Url, name: &str) -> String {
    url.query_pairs()
        .find(|(k, _)| k == name)
        .unwrap_or_else(|| panic!("no `{name}` in {url}"))
        .1
        .into_owned()
}

/// The status and the refusal message of an answer that must be a refusal,
/// after checking that it opens no session.
fn refusal(answer: &OauthAnswer) -> (u16, String) {
    let OauthAnswer::Page {
        status,
        page: OauthPage::Refused { message },
        cookies,
    } = answer
    else {
        panic!("a refusal: {answer:?}");
    };
    assert!(
        !cookies.iter().any(|c| c.as_str().starts_with("session=")),
        "a refusal opens no session: {cookies:?}"
    );
    (*status, message.clone())
}

fn clears_the_flow(answer: &OauthAnswer) -> bool {
    let OauthAnswer::Page { cookies, .. } = answer else {
        return false;
    };
    cookies
        .iter()
        .any(|c| c.as_str().starts_with("session_oauth=; Max-Age=0"))
}

/// The provider's refusal (`error=access_denied`) is a page that opens no
/// session, clears the flow, and calls the provider for nothing.
#[tokio::test]
async fn a_refusal_at_the_provider_is_a_page_and_no_session() {
    let auth = services();
    let stub = Arc::new(github(1, "ada@example.com", true));
    let routes = routes_on(GITHUB, stub.clone(), Arc::new(TestClock::at(1_700_000_000)));
    let (url, flow) = start(&auth, &routes).await;
    let query = format!(
        "error=access_denied&error_description=the+user+said+no&state={}",
        param(&url, "state")
    );
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &flow))
        .await
        .unwrap();
    let (status, message) = refusal(&answer);
    assert_eq!(status, 400);
    assert_eq!(message, "The provider did not let you in.");
    assert!(clears_the_flow(&answer), "{answer:?}");
    assert!(stub.requests().is_empty(), "no call to the provider");
}

/// A callback with a matching state and no code is a refusal, not a token
/// request with an empty code.
#[tokio::test]
async fn a_callback_without_a_code_is_refused() {
    let auth = services();
    let stub = Arc::new(github(1, "ada@example.com", true));
    let routes = routes_on(GITHUB, stub.clone(), Arc::new(TestClock::at(1_700_000_000)));
    let (url, flow) = start(&auth, &routes).await;
    let query = format!("state={}", param(&url, "state"));
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &flow))
        .await
        .unwrap();
    assert_eq!(refusal(&answer).0, 400);
    assert!(stub.requests().is_empty(), "no call to the provider");
}

/// A flow cookie with one byte changed, or one signed with another key, is
/// refused before anything leaves the process.
#[tokio::test]
async fn a_tampered_flow_cookie_is_refused() {
    let auth = services();
    let stub = Arc::new(github(1, "ada@example.com", true));
    let routes = routes_on(GITHUB, stub.clone(), Arc::new(TestClock::at(1_700_000_000)));
    let (url, flow) = start(&auth, &routes).await;
    let query = format!("code=the-code&state={}", param(&url, "state"));

    // One byte of the payload changed.
    let (name, value) = flow.split_once('=').unwrap();
    let mut bytes = value.as_bytes().to_vec();
    bytes[3] = if bytes[3] == b'A' { b'B' } else { b'A' };
    let tampered = format!("{name}={}", String::from_utf8(bytes).unwrap());
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &tampered))
        .await
        .unwrap();
    assert_eq!(
        refusal(&answer),
        (400, "This sign-in is not valid. Start again.".to_string())
    );

    // A cookie another application (another key) sealed.
    let mut config = services_config();
    config.signing_key = SigningKey::from_bytes([9; 32]);
    let other = Auth::build(config).unwrap();
    let (other_url, other_flow) = start(&other, &routes).await;
    let query = format!("code=the-code&state={}", param(&other_url, "state"));
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &other_flow))
        .await
        .unwrap();
    assert_eq!(refusal(&answer).0, 400);
    assert!(stub.requests().is_empty(), "no call to the provider");
}

/// The flow lasts ten minutes: at 600 seconds the callback signs in, at 601
/// it is refused with the reason, and the provider is not called.
#[tokio::test]
async fn a_flow_older_than_ten_minutes_is_refused() {
    for (waited, signs_in) in [(600, true), (601, false)] {
        let auth = services();
        let clock = Arc::new(TestClock::at(1_700_000_000));
        let stub = Arc::new(github(5, "new@example.com", true));
        let routes = routes_on(GITHUB, stub.clone(), clock.clone());
        let (url, flow) = start(&auth, &routes).await;
        clock.advance(waited);
        let query = format!("code=the-code&state={}", param(&url, "state"));
        let answer = routes
            .answer(&auth, get("/oauth/github/callback", &query, &flow))
            .await
            .unwrap();
        if signs_in {
            assert!(
                matches!(
                    answer,
                    OauthAnswer::Page {
                        page: OauthPage::SignedIn { .. },
                        ..
                    }
                ),
                "after {waited}s: {answer:?}"
            );
        } else {
            assert_eq!(
                refusal(&answer),
                (
                    400,
                    "This sign-in took longer than ten minutes. Start again.".to_string()
                ),
                "after {waited}s"
            );
            assert!(stub.requests().is_empty(), "no call to the provider");
        }
    }
}

/// The token request carries the verifier whose S256 digest is the
/// challenge the browser took to the provider, and the redirect URI of the
/// configuration, never one from the request.
#[tokio::test]
async fn the_token_request_proves_the_pkce_challenge() {
    let auth = services();
    let stub = Arc::new(github(5, "new@example.com", true));
    let routes = routes_on(GITHUB, stub.clone(), Arc::new(TestClock::at(1_700_000_000)));
    let (url, flow) = start(&auth, &routes).await;
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(
        param(&url, "redirect_uri"),
        "https://app.test/oauth/github/callback"
    );
    let query = format!(
        "code=the-code&state={}&redirect_uri=https%3A%2F%2Fevil.test%2F",
        param(&url, "state")
    );
    routes
        .answer(&auth, get("/oauth/github/callback", &query, &flow))
        .await
        .unwrap();
    let token = stub
        .requests()
        .into_iter()
        .find(|r| {
            r.url
                .starts_with("https://github.com/login/oauth/access_token")
        })
        .expect("a token request");
    let form = token.form.expect("a form");
    let field = |name: &str| {
        form.iter()
            .find(|(k, _)| k == name)
            .unwrap_or_else(|| panic!("no `{name}` in the token request"))
            .1
            .clone()
    };
    assert!(brocanth_auth::oauth::pkce::verify(
        &field("code_verifier"),
        &param(&url, "code_challenge")
    ));
    assert_eq!(field("code"), "the-code");
    assert_eq!(
        field("redirect_uri"),
        "https://app.test/oauth/github/callback"
    );
}

/// A callback replayed with the same code: the provider refuses a code it
/// has already exchanged (`bad_verification_code` at GitHub), and the route
/// turns that into a refusal, not a session and not a server error.
#[tokio::test]
async fn a_replayed_code_that_the_provider_refuses_signs_nobody_in() {
    let auth = services();
    let spent = StubHttp::new().on(
        http::Method::POST,
        "https://github.com/login/oauth/access_token",
        json_answer(json!({
            "error": "bad_verification_code",
            "error_description": "The code passed is incorrect or expired."
        })),
    );
    let routes = routes_on(
        GITHUB,
        Arc::new(spent),
        Arc::new(TestClock::at(1_700_000_000)),
    );
    let (url, flow) = start(&auth, &routes).await;
    let query = format!("code=used&state={}", param(&url, "state"));
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &flow))
        .await
        .unwrap();
    assert_eq!(
        refusal(&answer),
        (400, "The provider did not let you in.".to_string())
    );
}

/// The callback clears the flow cookie, so the same callback URL opened
/// again in that browser (back button, history, a leaked link) finds no
/// flow and is refused.
#[tokio::test]
async fn a_finished_flow_cannot_be_finished_again() {
    let auth = services();
    let routes = routes(5, "new@example.com");
    let (url, flow) = start(&auth, &routes).await;
    let query = format!("code=the-code&state={}", param(&url, "state"));
    let first = routes
        .answer(&auth, get("/oauth/github/callback", &query, &flow))
        .await
        .unwrap();
    assert!(clears_the_flow(&first), "{first:?}");
    // The browser applied `Max-Age=0`: what is left in its jar is the
    // session, and no flow.
    let after = jar(&first);
    assert!(!after.contains("session_oauth="), "{after}");
    let again = routes
        .answer(&auth, get("/oauth/github/callback", &query, &after))
        .await
        .unwrap();
    let OauthAnswer::Page { status, page, .. } = again else {
        panic!("a page");
    };
    assert_eq!(status, 400);
    assert!(matches!(page, OauthPage::Refused { .. }), "{page:?}");
}

/// Login CSRF: an attacker's own code and state, pushed into a victim's
/// browser that holds a flow of its own, is refused.
#[tokio::test]
async fn an_attackers_callback_in_a_victims_browser_is_refused() {
    let auth = services();
    let stub = Arc::new(github(66, "mallory@example.com", true));
    let routes = routes_on(GITHUB, stub.clone(), Arc::new(TestClock::at(1_700_000_000)));
    let (attacker_url, _) = start(&auth, &routes).await;
    let (_, victim_flow) = start(&auth, &routes).await;
    let query = format!("code=mallorys-code&state={}", param(&attacker_url, "state"));
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &victim_flow))
        .await
        .unwrap();
    assert_eq!(refusal(&answer).0, 400);
    assert!(stub.requests().is_empty(), "no call to the provider");
}

// ---------------------------------------------------------------------------
// Addresses: unverified, and never a link by themselves
// ---------------------------------------------------------------------------

/// GitHub lists Ada's address as unverified. The route does not hand Ada's
/// account over, does not link it, and does not even ask: the identity has
/// no address, so a new account without one is made.
#[tokio::test]
async fn an_unverified_github_address_does_not_reach_its_owner() {
    let auth = services();
    let ada = password_session(&auth, "ada@example.com").await;
    let ada = auth.sessions().from_cookie_header(&ada).await.unwrap();
    let routes = routes_on(
        GITHUB,
        Arc::new(github(77, "ada@example.com", false)),
        Arc::new(TestClock::at(1_700_000_000)),
    );
    let answer = round_trip(&auth, &routes, "").await;
    let OauthAnswer::Page {
        page: OauthPage::SignedIn { .. },
        ref cookies,
        ..
    } = answer
    else {
        panic!("a sign-in as a new account: {answer:?}");
    };
    let header = jar(&answer);
    assert!(cookies.iter().any(|c| c.as_str().starts_with("session=")));
    let signed_in = auth.sessions().from_cookie_header(&header).await.unwrap();
    assert_ne!(signed_in.account, ada.account, "not Ada's account");
    let account = auth
        .store()
        .account(&signed_in.account)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(account.email, None, "the unverified address is not taken");
    assert!(auth.linking().links(&ada.account).await.unwrap().is_empty());
}

/// Google's id_token for one OIDC flow, as `subject` with `email`.
fn google_stub(nonce: &str, subject: &str, email: &str, verified: bool) -> StubHttp {
    let id_token = common::sign_id_token(&json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id",
        "sub": subject,
        "email": email,
        "email_verified": verified,
        "nonce": nonce,
        "iat": common::real_now(),
        "exp": common::real_now() + 600,
    }));
    StubHttp::new()
        .on(
            http::Method::POST,
            "https://oauth2.googleapis.com/token",
            json_answer(json!({"access_token": "at", "id_token": id_token})),
        )
        .on(
            http::Method::GET,
            "https://www.googleapis.com/oauth2/v3/certs",
            HttpResponse::json(common::TEST_JWKS),
        )
}

/// Runs a Google flow whose id_token carries `nonce` (or the flow's own
/// when `None`), signed out.
async fn google_round_trip(
    auth: &Services,
    nonce: Option<&str>,
    email: &str,
    verified: bool,
) -> OauthAnswer {
    // The button needs no HTTP; the callback gets a stub that knows the
    // flow's nonce.
    let clock = Arc::new(TestClock::at(1_700_000_000));
    let button = routes_on(GOOGLE, Arc::new(StubHttp::new()), clock.clone());
    let started = button
        .answer(auth, post("/oauth/google", ""))
        .await
        .unwrap();
    let OauthAnswer::Redirect { location, .. } = &started else {
        panic!("a redirect: {started:?}");
    };
    let url = url::Url::parse(location).unwrap();
    let flow_nonce = param(&url, "nonce");
    let stub = google_stub(nonce.unwrap_or(&flow_nonce), "g-1", email, verified);
    let callback = routes_on(GOOGLE, Arc::new(stub), clock);
    let query = format!("code=c&state={}", param(&url, "state"));
    callback
        .answer(auth, get("/oauth/google/callback", &query, &jar(&started)))
        .await
        .unwrap()
}

/// An id_token minted for another flow (another nonce) is refused.
#[tokio::test]
async fn an_id_token_with_another_nonce_is_refused() {
    let auth = services();
    let answer = google_round_trip(&auth, Some("somebody-elses"), "g@example.com", true).await;
    assert_eq!(
        refusal(&answer),
        (400, "This sign-in is not valid. Start again.".to_string())
    );
    let fine = google_round_trip(&auth, None, "g@example.com", true).await;
    assert!(
        matches!(
            fine,
            OauthAnswer::Page {
                page: OauthPage::SignedIn { .. },
                ..
            }
        ),
        "the flow's own nonce signs in: {fine:?}"
    );
}

/// Google reports Ada's address, verified or not. Either way the callback
/// asks Ada to sign in first: a matching address never links by itself.
#[tokio::test]
async fn a_matching_oidc_address_never_links_verified_or_not() {
    for verified in [true, false] {
        let auth = services();
        let ada = password_session(&auth, "ada@example.com").await;
        let account = auth
            .sessions()
            .from_cookie_header(&ada)
            .await
            .unwrap()
            .account;
        let answer = google_round_trip(&auth, None, "ada@example.com", verified).await;
        let OauthAnswer::Page {
            status,
            page: OauthPage::LinkFirst { .. },
            ref cookies,
        } = answer
        else {
            panic!("verified={verified}: {answer:?}");
        };
        assert_eq!(status, 409);
        assert!(
            !cookies.iter().any(|c| c.as_str().starts_with("session=")),
            "verified={verified}: no session"
        );
        assert!(auth.linking().links(&account).await.unwrap().is_empty());
    }
}

// ---------------------------------------------------------------------------
// Where the browser goes: no open redirect
// ---------------------------------------------------------------------------

/// `next`, `return_to` and `redirect_uri` in a request go nowhere: the
/// routes read where to go from their configuration only.
#[tokio::test]
async fn a_return_path_in_the_request_is_ignored() {
    let auth = services();
    let routes = routes(5, "new@example.com");

    let page = routes
        .answer(
            &auth,
            get("/oauth/github", "next=https%3A%2F%2Fevil.test%2F", ""),
        )
        .await
        .unwrap();
    let OauthAnswer::Page { page, .. } = page else {
        panic!("a page");
    };
    assert!(!page.html().contains("evil"), "{}", page.html());

    let started = routes
        .answer(
            &auth,
            OauthRequest {
                post: true,
                path: "/oauth/github",
                query: "return_to=%2F%2Fevil.test",
                body: "next=https%3A%2F%2Fevil.test%2F",
                cookies: "",
            },
        )
        .await
        .unwrap();
    let OauthAnswer::Redirect { location, .. } = &started else {
        panic!("a redirect");
    };
    assert!(
        location.starts_with("https://github.com/login/oauth/authorize?"),
        "{location}"
    );
    assert!(!location.contains("evil"), "{location}");

    let url = url::Url::parse(location).unwrap();
    let query = format!(
        "code=c&state={}&next=https%3A%2F%2Fevil.test%2F&return_to=%2F%2Fevil.test",
        param(&url, "state")
    );
    let answer = routes
        .answer(&auth, get("/oauth/github/callback", &query, &jar(&started)))
        .await
        .unwrap();
    let OauthAnswer::Page { page, .. } = answer else {
        panic!("a page");
    };
    assert_eq!(
        page,
        OauthPage::SignedIn {
            location: "/".to_string()
        }
    );
    assert!(!page.html().contains("evil"), "{}", page.html());
}

/// `after_sign_in` is where a sign-in and a link go; the default is `/`.
#[tokio::test]
async fn after_sign_in_is_where_a_person_goes() {
    let auth = services();
    let dashboard = routes(5, "new@example.com").after_sign_in("/dashboard");
    let answer = round_trip(&auth, &dashboard, "").await;
    assert!(
        matches!(
            &answer,
            OauthAnswer::Page { page: OauthPage::SignedIn { location }, .. } if location == "/dashboard"
        ),
        "{answer:?}"
    );

    // And after a link.
    let auth = services();
    let routes = routes(7, "ada@example.com").after_sign_in("/settings");
    let ada = password_session(&auth, "ada@example.com").await;
    let pending = jar(&round_trip(&auth, &routes, "").await);
    let linked = routes
        .answer(
            &auth,
            post("/oauth/github/link", &format!("{ada}; {pending}")),
        )
        .await
        .unwrap();
    assert!(
        matches!(&linked, OauthAnswer::Redirect { location, .. } if location == "/settings"),
        "{linked:?}"
    );
}

/// Without a sign-in page, the "sign in first" page has no sign-in link;
/// with one, the link's `next` is the link path and nothing else.
#[tokio::test]
async fn the_sign_in_page_is_optional() {
    let auth = services();
    password_session(&auth, "ada@example.com").await;
    let routes = OauthRoutes::new(
        Arc::new(github(7, "ada@example.com", true)),
        Arc::new(TestClock::at(1_700_000_000)),
        vec![ConfiguredProvider {
            provider: GITHUB,
            client: ClientConfig::new("client-id", "client-secret", "https://app.test/cb"),
        }],
    );
    let answer = round_trip(&auth, &routes, "").await;
    let OauthAnswer::Page {
        page: OauthPage::LinkFirst { sign_in, .. },
        ..
    } = answer
    else {
        panic!("{answer:?}");
    };
    assert_eq!(sign_in, None);
}

/// The mount point moves every route, and the old one answers nothing.
#[tokio::test]
async fn the_mount_point_moves_every_route() {
    let auth = services();
    let routes = routes(5, "new@example.com").at("/auth/oauth/");
    let page = routes
        .answer(&auth, get("/auth/oauth/github", "", ""))
        .await
        .unwrap();
    assert!(
        matches!(
            &page,
            OauthAnswer::Page { page: OauthPage::Continue { action, .. }, .. } if action == "/auth/oauth/github"
        ),
        "{page:?}"
    );
    for path in ["/oauth/github", "/auth/oauthgithub", "/auth/oauth"] {
        let answer = routes.answer(&auth, get(path, "", "")).await.unwrap();
        assert!(
            matches!(answer, OauthAnswer::Page { status: 404, .. }),
            "{path}: {answer:?}"
        );
    }
}

/// A path under a provider that is no route is not found, whatever the
/// method.
#[tokio::test]
async fn an_unknown_step_is_not_found() {
    let auth = services();
    let routes = routes(5, "new@example.com");
    for (is_post, path) in [
        (false, "/oauth/github/token"),
        (true, "/oauth/github/token"),
        (true, "/oauth/github/callback/extra"),
        (false, "/oauth/"),
    ] {
        let request = OauthRequest {
            post: is_post,
            path,
            ..OauthRequest::default()
        };
        let answer = routes.answer(&auth, request).await.unwrap();
        assert!(
            matches!(
                answer,
                OauthAnswer::Page {
                    status: 404,
                    page: OauthPage::NotFound,
                    ..
                }
            ),
            "{path}: {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The pending link cookie
// ---------------------------------------------------------------------------

/// A pending link that was tampered with, is older than ten minutes, is for
/// another provider, or is the flow cookie under the link's name, links
/// nothing.
#[tokio::test]
async fn a_forged_or_stale_pending_link_links_nothing() {
    let clock = Arc::new(TestClock::at(1_700_000_000));
    let mut config = services_config();
    config.clock = clock.clone();
    let auth = Auth::build(config).unwrap();
    let routes = routes_on(
        GITHUB,
        Arc::new(github(7, "ada@example.com", true)),
        clock.clone(),
    );
    let ada = password_session(&auth, "ada@example.com").await;
    let pending = jar(&round_trip(&auth, &routes, "").await);
    let account = auth
        .sessions()
        .from_cookie_header(&ada)
        .await
        .unwrap()
        .account;

    let (name, value) = pending.split_once('=').unwrap();
    let (payload, signature) = value.rsplit_once('.').unwrap();

    // Another subject in the payload, the old signature.
    let forged_payload = {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut json: serde_json::Value =
            serde_json::from_slice(&engine.decode(payload).unwrap()).unwrap();
        json["subject"] = json!("666");
        engine.encode(serde_json::to_vec(&json).unwrap())
    };
    let forged = format!("{name}={forged_payload}.{signature}");

    // The flow cookie's value under the link's name.
    let (_, flow) = start(&auth, &routes).await;
    let flow_value = flow.split_once('=').unwrap().1;
    let swapped = format!("{name}={flow_value}");

    for (what, cookie) in [("forged", forged), ("flow as link", swapped)] {
        let answer = routes
            .answer(
                &auth,
                post("/oauth/github/link", &format!("{ada}; {cookie}")),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                answer,
                OauthAnswer::Page {
                    status: 400,
                    page: OauthPage::Refused { .. },
                    ..
                }
            ),
            "{what}: {answer:?}"
        );
    }

    // A pending link for GitHub, presented at GitLab.
    let both = OauthRoutes::new(
        Arc::new(github(7, "ada@example.com", true)),
        clock.clone(),
        vec![
            ConfiguredProvider {
                provider: GITHUB,
                client: ClientConfig::new("id", "secret", "https://app.test/cb"),
            },
            ConfiguredProvider {
                provider: GITLAB,
                client: ClientConfig::new("id", "secret", "https://app.test/cb"),
            },
        ],
    );
    let answer = both
        .answer(
            &auth,
            post("/oauth/gitlab/link", &format!("{ada}; {pending}")),
        )
        .await
        .unwrap();
    assert!(
        matches!(answer, OauthAnswer::Page { status: 400, .. }),
        "{answer:?}"
    );

    // Ten minutes and one second later.
    clock.advance(601);
    let answer = routes
        .answer(
            &auth,
            post("/oauth/github/link", &format!("{ada}; {pending}")),
        )
        .await
        .unwrap();
    assert!(
        matches!(answer, OauthAnswer::Page { status: 400, .. }),
        "stale: {answer:?}"
    );
    assert!(auth.linking().links(&account).await.unwrap().is_empty());
}

/// A provider account linked to Bob is not linked to Ada as well.
#[tokio::test]
async fn a_provider_account_linked_elsewhere_is_not_linked_again() {
    let auth = services();
    let routes = routes(7, "bob@example.com");
    // Bob registers with GitHub account 7.
    round_trip(&auth, &routes, "").await;
    // Ada, signed in, finishes a flow as GitHub account 7 and confirms.
    let ada = password_session(&auth, "ada@example.com").await;
    let answer = round_trip(&auth, &routes, &ada).await;
    let pending = jar(&answer);
    assert!(pending.contains("session_oauth_link="), "{answer:?}");
    let linked = routes
        .answer(
            &auth,
            post("/oauth/github/link", &format!("{ada}; {pending}")),
        )
        .await
        .unwrap();
    let OauthAnswer::Page { status, page, .. } = linked else {
        panic!("a refusal: {linked:?}");
    };
    assert_eq!(status, 409);
    assert_eq!(
        page,
        OauthPage::Refused {
            message: "That GitHub account is connected to another account.".to_string()
        }
    );
}

/// A signed-in person whose provider is linked already goes on, and the
/// callback does not ask again.
#[tokio::test]
async fn a_linked_provider_signs_in_again_without_asking() {
    let auth = services();
    let routes = routes(7, "ada@example.com");
    let first = round_trip(&auth, &routes, "").await;
    let session = jar(&first);
    let again = round_trip(&auth, &routes, &session).await;
    assert!(
        matches!(
            again,
            OauthAnswer::Page {
                status: 200,
                page: OauthPage::SignedIn { .. },
                ..
            }
        ),
        "{again:?}"
    );
    // Signed out, the same provider account signs in as the same account.
    let back = round_trip(&auth, &routes, "").await;
    let a = auth.sessions().from_cookie_header(&session).await.unwrap();
    let b = auth
        .sessions()
        .from_cookie_header(&jar(&back))
        .await
        .unwrap();
    assert_eq!(a.account, b.account);
}

// ---------------------------------------------------------------------------
// Second factors, cookies, escaping
// ---------------------------------------------------------------------------

/// An account with a confirmed TOTP secret is not signed in by its provider
/// alone.
#[tokio::test]
async fn an_account_with_a_second_factor_is_not_signed_in_by_a_provider() {
    use brocanth_auth::storage::{MfaStore, TotpSecretRecord};
    let auth = services();
    let routes = routes(7, "ada@example.com");
    let first = round_trip(&auth, &routes, "").await;
    let account = auth
        .sessions()
        .from_cookie_header(&jar(&first))
        .await
        .unwrap()
        .account;
    auth.store()
        .put_totp_secret(
            &account,
            TotpSecretRecord {
                secret: vec![1; 20],
                created_at: 1_700_000_000,
                confirmed_at: Some(1_700_000_000),
                last_step: 0,
            },
        )
        .await
        .unwrap();
    let answer = round_trip(&auth, &routes, "").await;
    let (status, message) = refusal(&answer);
    assert_eq!(status, 403);
    assert!(message.contains("second factor"), "{message}");
}

/// A pre-login session the browser arrived with is ended by the sign-in:
/// a planted session id does not become a signed-in one.
#[tokio::test]
async fn a_planted_pre_login_session_does_not_survive_the_sign_in() {
    let auth = services();
    auth.accounts()
        .register("ghost@example.com", "a long passphrase")
        .await
        .unwrap();
    let id = auth
        .store()
        .account_by_email(&brocanth_auth::EmailAddress::parse("ghost@example.com").unwrap())
        .await
        .unwrap()
        .unwrap()
        .id;
    let planted = auth.sessions().begin_anonymous(&id).await.unwrap();
    let planted = planted
        .cookie
        .as_str()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let answer = round_trip(&auth, &routes(5, "new@example.com"), &planted).await;
    let session = jar(&answer);
    assert!(session.starts_with("session="), "{answer:?}");
    assert_ne!(session, planted, "a new session id");
    assert!(
        auth.sessions().from_cookie_header(&planted).await.is_err(),
        "the planted session is gone"
    );
}

/// Under a `__Host-` session cookie, the flow cookie is `__Host-oauth`,
/// `Secure`, `HttpOnly`, `SameSite=Lax`, path `/`, ten minutes.
#[tokio::test]
async fn the_flow_cookie_has_the_attributes_the_doc_promises() {
    let mut config = services_config();
    config.session.cookie_name = "__Host-session".to_string();
    config.session.secure = true;
    let auth = Auth::build(config).unwrap();
    let started = routes(5, "new@example.com")
        .answer(&auth, post("/oauth/github", ""))
        .await
        .unwrap();
    let OauthAnswer::Redirect { cookies, .. } = started else {
        panic!("a redirect");
    };
    let flow = cookies[0].as_str();
    assert!(flow.starts_with("__Host-oauth="), "{flow}");
    for attribute in [
        "Max-Age=600",
        "Path=/",
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
    ] {
        assert!(flow.contains(attribute), "{attribute} in {flow}");
    }
    assert!(!flow.contains("Domain"), "{flow}");

    // A plain cookie name over HTTP: no `Secure`.
    let auth = services();
    let started = routes(5, "new@example.com")
        .answer(&auth, post("/oauth/github", ""))
        .await
        .unwrap();
    let OauthAnswer::Redirect { cookies, .. } = started else {
        panic!("a redirect");
    };
    assert!(!cookies[0].as_str().contains("Secure"), "{cookies:?}");
}

/// Every string a page prints is escaped: the address a provider reports,
/// the provider name, the locations.
#[test]
fn every_page_escapes_what_it_prints() {
    let evil = "\"><script>alert(1)</script>";
    let pages = [
        OauthPage::Continue {
            provider: evil.to_string(),
            action: evil.to_string(),
        },
        OauthPage::SignedIn {
            location: evil.to_string(),
        },
        OauthPage::LinkFirst {
            provider: evil.to_string(),
            sign_in: Some(evil.to_string()),
            link: evil.to_string(),
        },
        OauthPage::Link {
            provider: evil.to_string(),
            email: Some(evil.to_string()),
            action: evil.to_string(),
        },
        OauthPage::Refused {
            message: evil.to_string(),
        },
    ];
    for page in pages {
        let html = page.html();
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("\"><script"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }
}
