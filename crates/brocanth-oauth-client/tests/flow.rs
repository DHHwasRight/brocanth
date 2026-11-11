//! The connect flow, the token source, client credentials and the
//! authenticated client, against a fake provider on a local port.

mod support;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use brocanth_oauth_client::web::{ConnectAnswer, ConnectPage, ConnectRequest, ConnectRoutes};
use brocanth_oauth_client::{Error, Key, MemoryTokenStore, OauthClient, Provider, TokenStore};
use support::{callback, client, connect, memory, param, start, Fake, Manual, BASE};

const NOW: i64 = 1_800_000_000;

async fn setup() -> (
    Arc<Fake>,
    Arc<Manual>,
    Arc<dyn TokenStore>,
    OauthClient,
    ConnectRoutes,
) {
    let fake = Fake::start().await;
    let clock = Manual::at(NOW);
    let store = memory();
    let client = client(&fake, Arc::clone(&clock), Arc::clone(&store));
    let routes = ConnectRoutes::new(client.clone(), "/connections", false);
    (fake, clock, store, client, routes)
}

fn refused(answer: &ConnectAnswer) -> &str {
    match answer {
        ConnectAnswer::Page {
            status: 400,
            page: ConnectPage::Refused { message },
            cookies,
        } => {
            assert!(
                cookies[0].contains("Max-Age=0"),
                "the flow cookie is cleared"
            );
            message
        }
        other => panic!("a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_connection_stores_a_sealed_token_for_the_account_that_started_it() {
    let (fake, _, store, client, routes) = setup().await;
    let begun = start(&routes, "alice", "return_to=%2Fsettings").await;

    // The consent URL carries PKCE (S256), the scopes and the redirect URI.
    assert!(begun.url.starts_with(&format!("{}/authorize?", fake.url)));
    assert_eq!(
        param(&begun.url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(param(&begun.url, "scope").as_deref(), Some("read write"));
    assert_eq!(
        param(&begun.url, "redirect_uri").as_deref(),
        Some(&*format!("{BASE}/fake/callback"))
    );
    // The cookie is sealed: the state is not readable in it.
    assert!(!begun.cookie.contains(&begun.state));

    fake.consent("c1", &param(&begun.url, "code_challenge").unwrap());
    // No session on the callback: a Strict session cookie is not sent on a
    // cross-site navigation. The account comes from the sealed cookie.
    let answer = callback(
        &routes,
        &begun.cookie,
        &format!("code=c1&state={}", begun.state),
        None,
    )
    .await;
    let ConnectAnswer::Page {
        status: 200,
        page: ConnectPage::Connected { location, .. },
        ..
    } = answer
    else {
        panic!("connected, got {answer:?}");
    };
    assert_eq!(location, "/settings");

    let token = client.token("alice", "fake").await.unwrap();
    assert_eq!(token.access_token(), "at-1");
    assert_eq!(token.scopes, ["read", "write"]);

    // At rest, neither token is in clear.
    let row = store.get("alice", "fake").await.unwrap().unwrap();
    for needle in [b"at-1".as_slice(), b"rt-2".as_slice()] {
        assert!(!row.access.windows(4).any(|w| w == needle));
        assert!(!row
            .refresh
            .as_ref()
            .unwrap()
            .windows(4)
            .any(|w| w == needle));
    }
    // Nobody else got it.
    assert!(matches!(
        client.token("bob", "fake").await,
        Err(Error::NotConnected { .. })
    ));
}

#[tokio::test]
async fn a_changed_cookie_or_state_is_refused() {
    let (fake, _, _, _, routes) = setup().await;
    let begun = start(&routes, "alice", "").await;
    fake.consent("c1", &param(&begun.url, "code_challenge").unwrap());

    // A flipped character in the sealed cookie.
    let mut tampered = begun.cookie.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    let answer = callback(
        &routes,
        &tampered,
        &format!("code=c1&state={}", begun.state),
        None,
    )
    .await;
    assert!(refused(&answer).contains("not valid"), "{answer:?}");

    // Another state than the one this browser started with.
    let answer = callback(&routes, &begun.cookie, "code=c1&state=forged", None).await;
    assert!(refused(&answer).contains("does not belong"), "{answer:?}");

    // No cookie at all: another browser.
    let answer = callback(&routes, "", &format!("code=c1&state={}", begun.state), None).await;
    assert!(refused(&answer).contains("did not start in this browser"));

    // Somebody else signed in meanwhile.
    let answer = callback(
        &routes,
        &begun.cookie,
        &format!("code=c1&state={}", begun.state),
        Some("mallory"),
    )
    .await;
    assert!(refused(&answer).contains("somebody else"), "{answer:?}");

    // The person said no at the provider.
    let answer = callback(
        &routes,
        &begun.cookie,
        &format!("error=access_denied&state={}", begun.state),
        None,
    )
    .await;
    assert!(refused(&answer).contains("access_denied"), "{answer:?}");
}

#[tokio::test]
async fn a_flow_older_than_ten_minutes_is_refused() {
    let (fake, clock, _, _, routes) = setup().await;
    let begun = start(&routes, "alice", "").await;
    fake.consent("c1", &param(&begun.url, "code_challenge").unwrap());
    clock.advance(601);
    let answer = callback(
        &routes,
        &begun.cookie,
        &format!("code=c1&state={}", begun.state),
        None,
    )
    .await;
    assert!(refused(&answer).contains("ten minutes"), "{answer:?}");
}

#[tokio::test]
async fn a_return_path_to_another_site_is_dropped() {
    let (fake, _, _, _, routes) = setup().await;
    for evil in [
        "https%3A%2F%2Fevil.example%2F",
        "%2F%2Fevil.example",
        "%2F%5Cevil.example",
    ] {
        let begun = start(&routes, "alice", &format!("return_to={evil}")).await;
        fake.consent("c1", &param(&begun.url, "code_challenge").unwrap());
        let answer = callback(
            &routes,
            &begun.cookie,
            &format!("code=c1&state={}", begun.state),
            None,
        )
        .await;
        let ConnectAnswer::Page {
            page: ConnectPage::Connected { location, .. },
            ..
        } = answer
        else {
            panic!("connected, got {answer:?}");
        };
        assert_eq!(location, "/connections", "{evil}");
    }
}

#[tokio::test]
async fn every_page_but_the_callback_needs_a_signed_in_account() {
    let (_, _, _, _, routes) = setup().await;
    for (post, path) in [
        (false, "/connections"),
        (false, "/connections/fake"),
        (true, "/connections/fake"),
        (true, "/connections/fake/disconnect"),
    ] {
        let answer = routes
            .answer(ConnectRequest {
                post,
                path,
                ..ConnectRequest::default()
            })
            .await
            .unwrap();
        assert!(
            matches!(
                answer,
                ConnectAnswer::Page {
                    status: 401,
                    page: ConnectPage::SignIn,
                    ..
                }
            ),
            "{path}: {answer:?}"
        );
    }
    let answer = routes
        .answer(ConnectRequest {
            path: "/connections/nope",
            account: Some("alice"),
            ..ConnectRequest::default()
        })
        .await
        .unwrap();
    assert!(matches!(answer, ConnectAnswer::Page { status: 404, .. }));
}

#[tokio::test]
async fn an_expired_token_is_refreshed_and_the_rotated_refresh_token_is_kept() {
    let (fake, clock, _, client, routes) = setup().await;
    connect(&fake, &routes, "alice").await;
    let first = client.token("alice", "fake").await.unwrap();
    assert_eq!(first.refresh_token(), Some("rt-2"));

    // Five seconds before expiry is inside the ten-second delta.
    clock.advance(3600 - 5);
    let second = client.token("alice", "fake").await.unwrap();
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(second.access_token(), "at-3");
    assert_eq!(
        second.refresh_token(),
        Some("rt-4"),
        "the rotated one is stored"
    );
    assert_eq!(second.expires_at, Some(NOW + 3600 - 5 + 3600));

    // The next refresh uses the rotated token: the old one is void.
    clock.advance(3600);
    let third = client.token("alice", "fake").await.unwrap();
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 2);
    assert_eq!(third.access_token(), "at-5");

    // A valid token is returned as it is, with no request.
    assert_eq!(client.token("alice", "fake").await.unwrap(), third);
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn concurrent_callers_make_one_refresh() {
    let (fake, clock, _, client, routes) = setup().await;
    connect(&fake, &routes, "alice").await;
    fake.delay_ms.store(150, Ordering::SeqCst);
    clock.advance(3600);
    let calls = (0..16).map(|_| {
        let client = client.clone();
        tokio::spawn(async move { client.token("alice", "fake").await })
    });
    let mut tokens = Vec::new();
    for call in calls {
        tokens.push(call.await.unwrap().expect("every caller gets a token"));
    }
    assert_eq!(
        fake.refreshes.load(Ordering::SeqCst),
        1,
        "one refresh for 16 callers"
    );
    assert!(tokens.iter().all(|t| t == &tokens[0]));
}

#[tokio::test]
async fn a_revoked_refresh_token_asks_the_app_to_reconnect() {
    let (fake, clock, _, client, routes) = setup().await;
    connect(&fake, &routes, "alice").await;
    fake.refresh.lock().unwrap().clear();
    clock.advance(3600);
    let err = client.token("alice", "fake").await.unwrap_err();
    assert!(matches!(err, Error::Reconnect { .. }), "{err:?}");
    let text = err.to_string();
    assert!(
        text.contains("invalid_grant") && text.contains("help:"),
        "{text}"
    );
    assert!(!text.contains("rt-"), "no token in the message: {text}");
}

#[tokio::test]
async fn an_expired_token_without_a_refresh_token_asks_to_reconnect() {
    let (fake, clock, _, client, routes) = setup().await;
    fake.no_refresh.store(true, Ordering::SeqCst);
    connect(&fake, &routes, "alice").await;
    clock.advance(3600);
    assert!(matches!(
        client.token("alice", "fake").await,
        Err(Error::Reconnect { .. })
    ));
}

#[tokio::test]
async fn the_app_token_is_fetched_once_and_cached_until_it_runs_out() {
    let (fake, clock, _, client, _) = setup().await;
    fake.delay_ms.store(100, Ordering::SeqCst);
    let calls = (0..8).map(|_| {
        let client = client.clone();
        tokio::spawn(async move { client.app_token("fake").await })
    });
    let mut tokens = Vec::new();
    for call in calls {
        tokens.push(call.await.unwrap().unwrap());
    }
    assert_eq!(fake.app_grants.load(Ordering::SeqCst), 1);
    assert!(tokens.iter().all(|t| t == &tokens[0]));
    assert_eq!(tokens[0].scopes, ["app"]);

    clock.advance(1000);
    assert_eq!(client.app_token("fake").await.unwrap(), tokens[0]);
    assert_eq!(fake.app_grants.load(Ordering::SeqCst), 1);

    clock.advance(3600);
    assert_ne!(client.app_token("fake").await.unwrap(), tokens[0]);
    assert_eq!(fake.app_grants.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_401_forces_one_refresh_and_one_retry() {
    let (fake, _, _, client, routes) = setup().await;
    connect(&fake, &routes, "alice").await;
    let api = format!("{}/api", fake.url);
    let http = client.http("alice", "fake");

    let response = http.send(http.get(&api)).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 0);

    // The provider voids the access token before its expiry.
    fake.access.lock().unwrap().clear();
    let response = http.send(http.get(&api)).await.unwrap();
    assert_eq!(response.status(), 200, "the retry carries the new token");
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);

    // An API that refuses every token: one retry, then the 401 comes back.
    fake.refuse_api.store(true, Ordering::SeqCst);
    let response = http.send(http.get(&api)).await.unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        fake.refreshes.load(Ordering::SeqCst),
        2,
        "one refresh, not a loop"
    );
}

#[tokio::test]
async fn the_app_client_fetches_a_new_token_after_a_401() {
    let (fake, _, _, client, _) = setup().await;
    let api = format!("{}/api", fake.url);
    let http = client.app_http("fake");
    assert_eq!(http.send(http.get(&api)).await.unwrap().status(), 200);
    fake.access.lock().unwrap().clear();
    assert_eq!(http.send(http.get(&api)).await.unwrap().status(), 200);
    assert_eq!(fake.app_grants.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_disconnect_revokes_at_the_provider_and_deletes_the_row() {
    let (fake, _, store, client, routes) = setup().await;
    connect(&fake, &routes, "alice").await;
    let answer = routes
        .answer(ConnectRequest {
            post: true,
            path: "/connections/fake/disconnect",
            account: Some("alice"),
            ..ConnectRequest::default()
        })
        .await
        .unwrap();
    assert!(matches!(answer, ConnectAnswer::Redirect { .. }));
    assert_eq!(*fake.revoked.lock().unwrap(), ["rt-2"]);
    assert!(store.get("alice", "fake").await.unwrap().is_none());
    assert!(client.connections("alice").await.unwrap().is_empty());
}

#[tokio::test]
async fn the_list_page_shows_each_provider_and_whether_it_is_connected() {
    let (fake, _, _, _, routes) = setup().await;
    let list = |account| {
        let routes = routes.clone();
        async move {
            let answer = routes
                .answer(ConnectRequest {
                    path: "/connections",
                    account: Some(account),
                    ..ConnectRequest::default()
                })
                .await
                .unwrap();
            match answer {
                ConnectAnswer::Page {
                    page: page @ ConnectPage::List { .. },
                    ..
                } => page,
                other => panic!("a list, got {other:?}"),
            }
        }
    };
    let ConnectPage::List { rows } = list("alice").await else {
        unreachable!()
    };
    assert!(!rows[0].connected);
    connect(&fake, &routes, "alice").await;
    let page = list("alice").await;
    let ConnectPage::List { rows } = &page else {
        unreachable!()
    };
    assert!(rows[0].connected);
    let html = page.html();
    assert!(
        html.contains("action=\"/connections/fake/disconnect\""),
        "{html}"
    );
    assert!(!html.contains("at-1"), "no token on the page");
}

#[test]
fn a_provider_url_must_be_https_unless_development_allows_http() {
    let insecure = Provider::new(
        "corp",
        "Corp",
        brocanth_oauth_client::Endpoint::new("https://corp.example/auth", "http://10.0.0.1/token"),
        "id",
        "secret",
    );
    let build = |allow| {
        OauthClient::builder(Key::from_bytes([1; 32]))
            .store(Arc::new(MemoryTokenStore::new()))
            .provider(insecure.clone())
            .allow_http(allow)
            .build()
    };
    let err = build(false).unwrap_err().to_string();
    assert!(err.contains("token_url") && err.contains("help:"), "{err}");
    assert!(build(true).is_ok());
    let text = format!("{insecure:?}");
    assert!(!text.contains("secret\""), "the secret is redacted: {text}");
}

#[test]
fn the_builtin_endpoints_are_https() {
    for id in ["github", "google", "microsoft", "gitlab"] {
        let p = Provider::builtin(id, "id", "secret").unwrap();
        p.check(false).unwrap();
    }
    assert!(Provider::builtin("myspace", "id", "secret").is_none());
}

#[tokio::test]
async fn the_memory_store_keeps_the_contract() {
    brocanth_oauth_client::conformance::run(&MemoryTokenStore::new()).await;
}

#[tokio::test]
async fn the_admin_tab_lists_connections_without_tokens_and_hides_the_button_from_a_reader() {
    use brocanth_admin::form::Submission;
    use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
    use brocanth_oauth_client::admin::ConnectionsPanel;

    let (fake, _, store, client, routes) = setup().await;
    connect(&fake, &routes, "<b>alice</b>").await;
    let panel = ConnectionsPanel::new(client);
    let empty = Submission::default();
    let revoke = Submission::from_pairs([("account", "<b>alice</b>"), ("provider", "fake")]);
    let render =
        |can_act: bool, post: bool, path: &'static str, submission: &'static Submission| {
            let panel = &panel;
            async move {
                panel
                    .render(PanelRequest {
                        path,
                        query: "",
                        post,
                        can_act,
                        submission,
                        mount: "/admin/connections",
                    })
                    .await
                    .unwrap()
            }
        };
    let empty: &'static Submission = Box::leak(Box::new(empty));
    let revoke: &'static Submission = Box::leak(Box::new(revoke));

    let Some(PanelPage::Body { html, .. }) = render(true, false, "", empty).await else {
        panic!("a body");
    };
    assert!(
        html.contains("&lt;b&gt;alice&lt;/b&gt;") && !html.contains("<b>alice"),
        "{html}"
    );
    assert!(html.contains("Revoke"));
    assert!(
        !html.contains("at-1") && !html.contains("rt-2"),
        "no token: {html}"
    );

    let Some(PanelPage::Body { html, .. }) = render(false, false, "", empty).await else {
        panic!("a body");
    };
    assert!(!html.contains("Revoke"), "a reader sees no button");

    // A POST from somebody who may not act changes nothing.
    render(false, true, "revoke", revoke).await;
    assert!(store.get("<b>alice</b>", "fake").await.unwrap().is_some());
    let Some(PanelPage::Redirect { .. }) = render(true, true, "revoke", revoke).await else {
        panic!("a redirect after the button");
    };
    assert!(store.get("<b>alice</b>", "fake").await.unwrap().is_none());
}
