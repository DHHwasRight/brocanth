//! An HTTP test client for brocanth, shaped on Django's.
//!
//! # The gap this closes
//!
//! `docs/framework-gaps.md` §G6: *"Every test in this repository that
//! exercises a screen calls `dispatch` directly, which is why three rounds of
//! tests in one afternoon asserted on CSS class names: there was no level at
//! which to assert on behaviour instead."*
//!
//! The two halves of that sentence are the two halves of this crate.
//!
//! **A level.** [`TestClient`] takes a built [`App`](brocanth::plugin_app::App)
//! and drives it the way `brocanth-server`'s bridge does: the request hooks,
//! the router, the handler, the response hooks. A test asks for a path, not
//! for a route by name, so the path matching, the 404 and the 405 are under
//! test for the first time — today every test reaches past the router and
//! hands a handler a `Params` it built itself.
//!
//! **Something to assert on.** [`TestResponse`] answers with a status, the
//! headers and an [`Html`] that knows what an element is. *"Is there a link
//! whose `href` is `/admin/post`"* and *"does this form post to
//! `/admin/user/3/delete`"* are questions; `body.contains("/admin/post")` is
//! a guess that passes when the path is in a comment.
//!
//! # The shape, in one test
//!
//! ```ignore
//! use brocanth_test::TestClient;
//!
//! let client = TestClient::new(app).seeding(|state| {
//!     state.insert(CurrentUser { staff: true });
//! });
//!
//! let page = client.get("/admin/jobs?page=2");
//! page.assert_ok();
//! page.html().assert_text("page 2");
//! page.html()
//!     .within("nav", "aria-label", "Sections")
//!     .assert_link_to("/admin/post");
//!
//! client
//!     .post("/admin/jobs", &[])
//!     .assert_redirects("/admin/jobs/retries");
//! ```
//!
//! | Django | here |
//! | --- | --- |
//! | `client.get(url)` | [`TestClient::get`] |
//! | `client.post(url, data)` | [`TestClient::post`] |
//! | `follow=True` | [`TestRequest::following`] |
//! | `force_login(user)` | [`TestClient::seeding`] |
//! | `assertStatus` | [`TestResponse::assert_status`] |
//! | `assertRedirects` | [`TestResponse::assert_redirects`] |
//! | `assertContains` | [`Html::assert_text`], and [`TestResponse::assert_contains`] for raw bytes |
//! | *(no equivalent)* | [`Html::assert_link_to`], [`Html::assert_form_posts_to`], [`Html::assert_field`], [`Html::within`] |
//!
//! # What it costs
//!
//! Two dependencies — the core, and the `http` whose types the core's
//! `Request` and `Response` already are. No HTML parser: `html` argues that
//! at length. No runtime: `rt` is thirty lines of `std`, and the one thing it
//! cannot do — drive a future that waits on a reactor — has
//! [`TestClient::send_async`] for an answer.
//!
//! # If the response body becomes a stream
//!
//! [`TestResponse::from_core`] is the only function here that reads a
//! [`Response`](brocanth::plugin_hooks::Response). Everything a test holds is
//! already a status, a header list and a `Vec<u8>` this crate owns. A body
//! that became a stream changes that one function — it collects the stream,
//! and `send` grows the `await` that `send_async` already has — and changes
//! no assertion above it.

#![forbid(unsafe_code)]

pub mod client;
pub mod html;
pub mod request;
pub mod response;
pub mod rt;

pub use client::{Planned, TestClient, TestFuture, TestRoute};
pub use html::{Element, Html};
pub use request::TestRequest;
pub use response::TestResponse;
