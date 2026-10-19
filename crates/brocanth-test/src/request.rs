//! What a test asks for.
//!
//! [`TestClient::get`](crate::TestClient::get) and
//! [`TestClient::post`](crate::TestClient::post) cover almost everything, and
//! build one of these. This type is for the request that needs a header, a
//! body that is not a form, or a verb that is neither.

use brocanth::plugin_hooks::Request;
use brocanth::routing::Verb;

/// One request, before it is sent.
#[derive(Debug, Clone)]
pub struct TestRequest {
    verb: Verb,
    /// The path and query together, as a browser's address bar holds them.
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    follow: bool,
}

impl TestRequest {
    /// A request with a verb and a target.
    ///
    /// `target` is the path and the query together — `"/admin/jobs?page=2"` —
    /// because that is what a test has in its hand. The core splits them.
    #[must_use]
    pub fn new(verb: Verb, target: impl Into<String>) -> Self {
        TestRequest {
            verb,
            target: target.into(),
            headers: Vec::new(),
            body: Vec::new(),
            follow: false,
        }
    }

    /// `GET`.
    #[must_use]
    pub fn get(target: impl Into<String>) -> Self {
        Self::new(Verb::Get, target)
    }

    /// `POST`. Add a body with [`TestRequest::form`] or [`TestRequest::body`].
    #[must_use]
    pub fn post(target: impl Into<String>) -> Self {
        Self::new(Verb::Post, target)
    }

    /// `PUT`.
    #[must_use]
    pub fn put(target: impl Into<String>) -> Self {
        Self::new(Verb::Put, target)
    }

    /// `PATCH`.
    #[must_use]
    pub fn patch(target: impl Into<String>) -> Self {
        Self::new(Verb::Patch, target)
    }

    /// `DELETE`.
    #[must_use]
    pub fn delete(target: impl Into<String>) -> Self {
        Self::new(Verb::Delete, target)
    }

    /// One header. Repeat for a header that appears more than once.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// A form body, as a browser sends one:
    /// `application/x-www-form-urlencoded`.
    ///
    /// This is the body the admin reads, and a request that is not
    /// urlencoded reaches it as an empty submission — so a test that invents
    /// its own encoding would be testing nothing.
    #[must_use]
    pub fn form(mut self, pairs: &[(&str, &str)]) -> Self {
        self.body = urlencode(pairs).into_bytes();
        self.header("content-type", "application/x-www-form-urlencoded")
    }

    /// A body with a content type of its own.
    #[must_use]
    pub fn body(mut self, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self.header("content-type", content_type)
    }

    /// Follow the redirect, and the one after it, up to the client's limit.
    ///
    /// Django's `follow=True`. The response that comes back is the last one,
    /// and
    /// [`TestResponse::redirect_chain`](crate::TestResponse::redirect_chain)
    /// is everything it went through.
    #[must_use]
    pub fn following(mut self) -> Self {
        self.follow = true;
        self
    }

    /// The verb this request carries.
    #[must_use]
    pub fn verb(&self) -> Verb {
        self.verb
    }

    /// The path and query this request is for.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Whether this request follows a redirect.
    #[must_use]
    pub fn follows(&self) -> bool {
        self.follow
    }

    /// The same request, aimed somewhere else, which is what following a
    /// redirect produces.
    pub(crate) fn redirected_to(&self, location: &str, keep_method: bool) -> Self {
        let mut next = TestRequest {
            // 307 and 308 say the verb and the body survive. Every other
            // redirect a browser follows with a GET, and that is what a
            // post-redirect-get is for.
            verb: if keep_method { self.verb } else { Verb::Get },
            target: location.to_string(),
            headers: Vec::new(),
            body: if keep_method {
                self.body.clone()
            } else {
                Vec::new()
            },
            follow: true,
        };
        // A content type describes a body. Carrying one onto a GET with no
        // body would make the next handler read a form that is not there.
        for (name, value) in &self.headers {
            if keep_method || !name.eq_ignore_ascii_case("content-type") {
                next.headers.push((name.clone(), value.clone()));
            }
        }
        next
    }

    /// This request as the core's own type, under the client's headers.
    ///
    /// # Panics
    ///
    /// When the target is not a URI, or a header name or value is not one.
    /// Both are a mistake in the test rather than a thing under test.
    pub(crate) fn build(&self, client_headers: &[(String, String)]) -> Request {
        let mut builder = http::Request::builder()
            .method(self.verb.to_http())
            .uri(&self.target);
        for (name, value) in client_headers.iter().chain(self.headers.iter()) {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder
            .body(self.body.clone())
            .unwrap_or_else(|e| panic!("`{} {}` is not a request: {e}", self.verb, self.target))
    }
}

/// Form pairs as `application/x-www-form-urlencoded`.
///
/// Twenty lines rather than a dependency, and the rule is the narrow one the
/// encoding states: unreserved characters pass, a space is `+`, everything
/// else is `%XX` over the UTF-8 bytes.
fn urlencode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (index, (name, value)) in pairs.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        encode_into(&mut out, name);
        out.push('=');
        encode_into(&mut out, value);
    }
    out
}

fn encode_into(out: &mut String, text: &str) {
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_body_is_what_a_browser_would_send() {
        let request = TestRequest::post("/admin/user/create")
            .form(&[("name", "Ada Lovelace"), ("email", "ada@example.com")]);
        let built = request.build(&[]);
        assert_eq!(
            std::str::from_utf8(built.body()).expect("utf-8"),
            "name=Ada+Lovelace&email=ada%40example.com"
        );
        assert_eq!(
            built
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/x-www-form-urlencoded")
        );
    }

    #[test]
    fn the_query_reaches_the_request_as_a_query() {
        let built = TestRequest::get("/admin/jobs?page=2").build(&[]);
        assert_eq!(built.uri().path(), "/admin/jobs");
        assert_eq!(built.uri().query(), Some("page=2"));
    }

    #[test]
    fn a_303_is_followed_with_a_get_and_no_body() {
        let post = TestRequest::post("/admin/jobs").form(&[("id", "7")]);
        let next = post.redirected_to("/admin/jobs/retries", false);
        assert_eq!(next.verb(), Verb::Get);
        assert!(next.body.is_empty());
        assert!(
            next.headers.is_empty(),
            "a content type describes a body, and there is none"
        );
    }
}
