//! The provider sign-in as routes: the button, the redirect, the callback,
//! and the link that needs a signed-in session.

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::http_client::{HttpResponse, StubHttp};
use brocanth_auth::mfa::{Admission, FactorPolicy};
use brocanth_auth::oauth::web::{OauthAnswer, OauthPage, OauthRequest, OauthRoutes};
use brocanth_auth::oauth::ClientConfig;
use brocanth_auth::plugin::{AuthConfig, ConfiguredProvider};
use brocanth_auth::providers::catalog::GITHUB;
use brocanth_auth::session::{SessionConfig, SigningKey};
use brocanth_auth::{Auth, MemoryStore, PasswordCost, Passwords};
use serde_json::json;

type Services = Auth<MemoryStore, StubHttp, TestClock>;

fn services() -> Services {
    Auth::build(AuthConfig {
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
    })
    .unwrap()
}

fn json_answer(body: serde_json::Value) -> HttpResponse {
    HttpResponse::json(body.to_string())
}

/// GitHub, answering as the account with id `subject` and address `email`.
fn routes(subject: u64, email: &str) -> OauthRoutes<StubHttp, TestClock> {
    let github = StubHttp::new()
        .on(
            http::Method::POST,
            "https://github.com/login/oauth/access_token",
            json_answer(json!({"access_token": "gho_x", "token_type": "bearer"})),
        )
        .on(
            http::Method::GET,
            "https://api.github.com/user/emails",
            json_answer(json!([{"email": email, "primary": true, "verified": true}])),
        )
        .on(
            http::Method::GET,
            "https://api.github.com/user",
            json_answer(json!({"id": subject, "login": "octocat"})),
        );
    OauthRoutes::new(
        Arc::new(github),
        Arc::new(TestClock::at(1_700_000_000)),
        vec![ConfiguredProvider {
            provider: GITHUB,
            client: ClientConfig::new(
                "client-id",
                "client-secret",
                "https://app.test/oauth/github/callback",
            ),
        }],
    )
    .sign_in_page("/sign-in")
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
