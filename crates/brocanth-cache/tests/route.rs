//! The route cache's hooks, driven through a real `App`.
//!
//! These tests are about the hooks: what is looked up, what is stored, what is
//! refused. They run against `Map`, a fake cache below, so they run on every
//! machine. That is the same choice `tests/panel.rs` makes, and the same
//! limit: nothing here proves anything about the SQL. The tag SQL, and the
//! route cache over a real Postgres, are in `tests/tags.rs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_body::{Body, BodyStream, Chunk};
use brocanth::plugin_hooks::{Begin, HookFamilies, Request, RequestState, Response};
use brocanth_cache::route::{RouteCache, RouteCacheConfig, RouteRule, STATUS_HEADER, TAGS_HEADER};
use brocanth_cache::{Cache, Key, Namespace, Result, Tag};

// ---------------------------------------------------------------------------
// The fake
// ---------------------------------------------------------------------------

type Entries = HashMap<(String, String), (Vec<u8>, Vec<String>)>;

/// A map with tags, and no expiry. Enough to watch the hooks.
#[derive(Default)]
struct Map(Mutex<Entries>);

impl Map {
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    fn tags(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .0
            .lock()
            .unwrap()
            .values()
            .flat_map(|(_, tags)| tags.clone())
            .collect();
        all.sort();
        all
    }
}

fn id(key: &Key) -> (String, String) {
    (key.namespace().as_str().into(), key.as_str().into())
}

#[async_trait]
impl Cache for Map {
    fn max_value_bytes(&self) -> usize {
        4096
    }
    async fn migrate(&self) -> Result<()> {
        Ok(())
    }
    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(&id(key)).map(|(v, _)| v.clone()))
    }
    async fn set(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<()> {
        self.set_tagged(key, value, ttl, &[]).await
    }
    async fn set_tagged(&self, key: &Key, value: &[u8], _: Duration, tags: &[Tag]) -> Result<()> {
        let tags = tags.iter().map(|t| t.as_str().to_owned()).collect();
        self.0
            .lock()
            .unwrap()
            .insert(id(key), (value.to_vec(), tags));
        Ok(())
    }
    async fn invalidate_tags(&self, tags: &[Tag]) -> Result<u64> {
        let mut map = self.0.lock().unwrap();
        let before = map.len();
        map.retain(|_, (_, have)| !tags.iter().any(|t| have.iter().any(|h| h == t.as_str())));
        Ok((before - map.len()) as u64)
    }
    async fn add(&self, _: &Key, _: &[u8], _: Duration) -> Result<bool> {
        unimplemented!("the route cache does not add")
    }
    async fn delete(&self, key: &Key) -> Result<bool> {
        Ok(self.0.lock().unwrap().remove(&id(key)).is_some())
    }
    async fn clear(&self, _: &Namespace) -> Result<u64> {
        unimplemented!("the route cache does not clear")
    }
    async fn lease(&self, _: &Key, _: Duration) -> Result<bool> {
        unimplemented!("the route cache takes no lease")
    }
    async fn release_lease(&self, _: &Key) -> Result<()> {
        unimplemented!("the route cache takes no lease")
    }
    async fn purge_expired(&self, _: u64) -> Result<u64> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// One request through the application
// ---------------------------------------------------------------------------

/// What the handler answers, and how many times it ran.
#[derive(Clone, Default)]
struct Handler {
    runs: Arc<Mutex<u32>>,
}

impl Handler {
    fn runs(&self) -> u32 {
        *self.runs.lock().unwrap()
    }
}

async fn serve<E, H: HookFamilies>(
    app: &App<E, H>,
    request: Request,
    handler: &Handler,
    respond: impl FnOnce() -> Response,
) -> Response {
    let mut request = request;
    let mut state = RequestState::new();
    let outcome = match app.begin_async(&mut request, &mut state).await {
        Begin::Dispatch => {
            *handler.runs.lock().unwrap() += 1;
            Ok(respond())
        }
        Begin::Answered(outcome) => outcome,
    };
    app.finish(&mut request, &mut state, outcome)
}

fn get(uri: &str) -> Request {
    http::Request::builder().uri(uri).body(Vec::new()).unwrap()
}

fn page(body: &str) -> Response {
    let mut response = Response::new(Body::from(body.to_owned()));
    response
        .headers_mut()
        .insert("content-type", "text/html".parse().unwrap());
    response
}

/// Waits for the spawned store to land.
async fn stored(map: &Map, entries: usize) {
    for _ in 0..200 {
        if map.len() >= entries {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the store did not land: {} entries", map.len());
}

/// Gives a spawned store the time to land, for a test that expects none.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn status(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(STATUS_HEADER)
        .and_then(|v| v.to_str().ok())
}

fn app_with(
    map: &Arc<Map>,
    rules: impl FnOnce(RouteCacheConfig) -> RouteCacheConfig,
) -> App<impl Sized, impl HookFamilies> {
    let config = rules(RouteCacheConfig::new(map.clone()));
    AppBuilder::new()
        .plugin_hooking(RouteCache::new(config))
        .build()
        .expect("an application")
}

const MINUTE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_miss_runs_the_handler_and_a_hit_does_not() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| c.rule(RouteRule::path("/", MINUTE)));
    let handler = Handler::default();

    let first = serve(&app, get("/"), &handler, || page("hello")).await;
    assert_eq!(status(&first), Some("miss"));
    stored(&map, 1).await;

    let second = serve(&app, get("/"), &handler, || page("changed")).await;
    assert_eq!(handler.runs(), 1, "the hit ran the handler");
    assert_eq!(status(&second), Some("hit"));
    assert_eq!(second.headers()["content-type"], "text/html");
    assert_eq!(second.body().buffered(), Some(&b"hello"[..]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_outside_every_rule_and_a_post_are_left_alone() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| c.rule(RouteRule::path("/", MINUTE)));
    let handler = Handler::default();

    let other = serve(&app, get("/about"), &handler, || page("about")).await;
    assert_eq!(status(&other), None);

    let post = http::Request::builder()
        .method("POST")
        .uri("/")
        .body(Vec::new())
        .unwrap();
    serve(&app, post, &handler, || page("posted")).await;
    settle().await;
    assert_eq!(map.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_error_a_cookie_or_a_private_response_is_not_stored() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| c.rule(RouteRule::prefix("/", MINUTE)));
    let handler = Handler::default();

    serve(&app, get("/missing"), &handler, || {
        let mut r = page("no");
        *r.status_mut() = http::StatusCode::NOT_FOUND;
        r
    })
    .await;
    serve(&app, get("/login"), &handler, || {
        let mut r = page("hi");
        r.headers_mut()
            .insert("set-cookie", "sid=1".parse().unwrap());
        r
    })
    .await;
    serve(&app, get("/me"), &handler, || {
        let mut r = page("me");
        r.headers_mut()
            .insert("cache-control", "private".parse().unwrap());
        r
    })
    .await;
    settle().await;
    assert_eq!(map.len(), 0);
}

/// A body that ends at once.
struct Empty;

impl BodyStream for Empty {
    fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
        Poll::Ready(None)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_is_never_stored() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| c.rule(RouteRule::path("/events", MINUTE)));
    let handler = Handler::default();

    let response = serve(&app, get("/events"), &handler, || {
        Response::new(Body::stream(Empty))
    })
    .await;
    assert!(response.body().is_streaming());
    assert_eq!(status(&response), None);
    settle().await;
    assert_eq!(map.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_with_credentials_is_not_cached_unless_the_rule_allows_it() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| {
        c.session_cookies(["sid"])
            .rule(RouteRule::path("/private", MINUTE))
            .rule(RouteRule::path("/shared", MINUTE).allow_authenticated())
    });
    let handler = Handler::default();

    let with_session = |uri: &str| {
        http::Request::builder()
            .uri(uri)
            .header("cookie", "sid=abc")
            .body(Vec::new())
            .unwrap()
    };

    serve(&app, with_session("/private"), &handler, || page("mine")).await;
    settle().await;
    assert_eq!(map.len(), 0, "a session's page was stored");

    // An analytics cookie is not a session, once the session is named.
    let tracked = http::Request::builder()
        .uri("/private")
        .header("cookie", "_ga=1")
        .body(Vec::new())
        .unwrap();
    serve(&app, tracked, &handler, || page("public")).await;
    stored(&map, 1).await;

    serve(&app, with_session("/shared"), &handler, || page("same")).await;
    stored(&map, 2).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_rule_tags_the_handler_tags_and_the_path_tag_are_stored() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| {
        c.rule(RouteRule::prefix("/posts/", MINUTE).tags([Tag::table("posts").unwrap()]))
    });
    let handler = Handler::default();

    let response = serve(&app, get("/posts/7"), &handler, || {
        let mut r = page("post 7");
        r.headers_mut()
            .insert(TAGS_HEADER, "posts:7, users:2".parse().unwrap());
        r
    })
    .await;
    assert!(
        !response.headers().contains_key(TAGS_HEADER),
        "the tags header left the server"
    );
    stored(&map, 1).await;
    assert_eq!(
        map.tags(),
        ["posts", "posts:7", "route:/posts/7", "users:2"]
    );

    // Invalidating the row drops the page, and the next request runs the
    // handler again.
    map.invalidate_tag(&Tag::row("posts", 7).unwrap())
        .await
        .unwrap();
    let again = serve(&app, get("/posts/7"), &handler, || page("post 7, edited")).await;
    assert_eq!(status(&again), Some("miss"));
    assert_eq!(handler.runs(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vary_header_and_the_query_string_split_the_entries() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| {
        c.rule(RouteRule::path("/", MINUTE).vary([http::header::ACCEPT_LANGUAGE]))
    });
    let handler = Handler::default();

    let in_lang = |lang: &str, uri: &str| {
        http::Request::builder()
            .uri(uri)
            .header("accept-language", lang)
            .body(Vec::new())
            .unwrap()
    };

    serve(&app, in_lang("en", "/"), &handler, || page("hello")).await;
    stored(&map, 1).await;
    serve(&app, in_lang("fr", "/"), &handler, || page("bonjour")).await;
    stored(&map, 2).await;
    serve(&app, in_lang("fr", "/?page=2"), &handler, || page("page 2")).await;
    stored(&map, 3).await;

    let fr = serve(&app, in_lang("fr", "/"), &handler, || page("wrong")).await;
    assert_eq!(fr.body().buffered(), Some(&b"bonjour"[..]));
    assert_eq!(handler.runs(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_over_the_cap_is_served_and_not_stored() {
    let map = Arc::new(Map::default());
    let app = app_with(&map, |c| c.rule(RouteRule::path("/big", MINUTE)));
    let handler = Handler::default();

    let big = "x".repeat(map.max_value_bytes() + 1);
    let response = serve(&app, get("/big"), &handler, || page(&big)).await;
    assert_eq!(response.body().len(), Some(big.len()));
    settle().await;
    assert_eq!(map.len(), 0);
}
