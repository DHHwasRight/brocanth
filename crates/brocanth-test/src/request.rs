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

    /// The first value of a header this request carries, case-insensitively.
    pub(crate) fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The same request with its `cookie` header replaced by `jar`.
    pub(crate) fn with_cookies(mut self, jar: &CookieJar) -> Self {
        self.headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
        if let Some(value) = jar.header() {
            self.headers.push(("cookie".to_string(), value));
        }
        self
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

/// Where a `location` points, as a target this client can send, resolved
/// against the target that answered.
///
/// `None` when the location names a scheme and a host that are not `host`:
/// another site, which a test client cannot visit. A fragment is dropped,
/// because a browser never sends it.
pub(crate) fn resolve(base: &str, location: &str, host: Option<&str>) -> Option<String> {
    let location = location.split('#').next().unwrap_or_default();
    let rest = if let Some(after) = location
        .strip_prefix("http://")
        .or_else(|| location.strip_prefix("https://"))
        .or_else(|| location.strip_prefix("//"))
    {
        let (authority, path) = match after.find(['/', '?']) {
            Some(at) => after.split_at(at),
            None => (after, "/"),
        };
        if host.is_none_or(|host| !host.eq_ignore_ascii_case(authority)) {
            return None;
        }
        return Some(if path.starts_with('?') {
            format!("/{path}")
        } else {
            path.to_string()
        });
    } else {
        location
    };
    let base_path = base.split(['?', '#']).next().unwrap_or("/");
    if rest.is_empty() {
        return Some(base.to_string());
    }
    if rest.starts_with('?') {
        return Some(format!("{base_path}{rest}"));
    }
    let (path, query) = match rest.find('?') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        // Relative: replace the last segment of the base path.
        let dir = &base_path[..=base_path.rfind('/').unwrap_or(0)];
        let dir = if dir.is_empty() { "/" } else { dir };
        format!("{dir}{path}")
    };
    Some(format!("{}{query}", normalise(&joined)))
}

/// `.` and `..` segments removed, as RFC 3986 section 5.2.4 does.
fn normalise(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                out.pop();
                if last {
                    out.push("");
                }
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

/// The cookies one followed request has collected, hop by hop.
///
/// A browser keeps the `set-cookie` of a redirect and sends it to the next
/// hop; a login that sets a session and redirects to the dashboard needs
/// that. This jar is per [`send`](crate::TestClient::send): nothing carries
/// over to the next request. It reads only a cookie's name and value, and
/// drops one whose `Max-Age` is zero or less.
#[derive(Debug, Default, Clone)]
pub(crate) struct CookieJar {
    cookies: Vec<(String, String)>,
}

impl CookieJar {
    /// The cookies the request already carries.
    pub(crate) fn from_request(request: &TestRequest) -> Self {
        let mut jar = Self::default();
        for (name, value) in &request.headers {
            if !name.eq_ignore_ascii_case("cookie") {
                continue;
            }
            for pair in value.split(';') {
                if let Some((key, value)) = pair.trim().split_once('=') {
                    jar.set(key.trim(), value.trim());
                }
            }
        }
        jar
    }

    /// Takes every `set-cookie` in a response.
    pub(crate) fn update(&mut self, headers: &[(String, String)]) {
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            let mut parts = value.split(';');
            let Some((key, val)) = parts.next().and_then(|pair| pair.split_once('=')) else {
                continue;
            };
            let expired = parts.any(|attribute| {
                attribute.trim().split_once('=').is_some_and(|(k, v)| {
                    k.trim().eq_ignore_ascii_case("max-age")
                        && v.trim().parse::<i64>().is_ok_and(|age| age <= 0)
                })
            });
            let key = key.trim();
            if expired {
                self.cookies.retain(|(name, _)| name != key);
            } else {
                self.set(key, val.trim());
            }
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        match self.cookies.iter_mut().find(|(name, _)| name == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => self.cookies.push((key.to_string(), value.to_string())),
        }
    }

    /// The `cookie` header value, or `None` when the jar is empty.
    pub(crate) fn header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
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
    fn a_location_is_resolved_against_the_target_that_answered() {
        assert_eq!(resolve("/posts/7", "/x", None).as_deref(), Some("/x"));
        assert_eq!(
            resolve("/posts/7", "edit", None).as_deref(),
            Some("/posts/edit")
        );
        assert_eq!(
            resolve("/posts/7/", "edit", None).as_deref(),
            Some("/posts/7/edit")
        );
        assert_eq!(
            resolve("/posts/7?a=1", "?page=2", None).as_deref(),
            Some("/posts/7?page=2")
        );
        assert_eq!(
            resolve("/a/b/c", "../d?q=1#top", None).as_deref(),
            Some("/a/d?q=1")
        );
        assert_eq!(
            resolve("/", "https://app.test/admin?x=1", Some("app.test")).as_deref(),
            Some("/admin?x=1")
        );
    }

    #[test]
    fn a_location_on_another_host_is_not_followed() {
        assert_eq!(resolve("/", "https://elsewhere.example/admin", None), None);
        assert_eq!(
            resolve("/", "//elsewhere.example/admin", Some("app.test")),
            None
        );
    }

    #[test]
    fn the_jar_keeps_set_cookie_and_drops_an_expired_one() {
        let mut jar = CookieJar::from_request(&TestRequest::get("/").header("cookie", "a=1"));
        jar.update(&[
            ("set-cookie".into(), "session=abc; Path=/; HttpOnly".into()),
            ("Set-Cookie".into(), "a=; Max-Age=0".into()),
        ]);
        assert_eq!(jar.header().as_deref(), Some("session=abc"));
        let next = TestRequest::get("/")
            .header("cookie", "old=1")
            .with_cookies(&jar);
        assert_eq!(next.header_value("cookie"), Some("session=abc"));
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
