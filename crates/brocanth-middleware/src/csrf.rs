//! A form post this application handed out.
//!
//! A browser sends the cookies for a site with every request to it,
//! whoever asked for the request. So a page on another site can post a
//! form here, and the visitor's session rides along. The defence is a
//! secret the other page cannot read: the application puts a token in the
//! form, and refuses any unsafe verb that does not carry it back.
//!
//! # Why a double-submit cookie
//!
//! The usual pattern is a synchroniser token: the server keeps the token
//! in the session and compares. That needs a per-session secret, and the
//! scaffolded `Session` holds one field, the cookie *name*. There is
//! nothing there to bind a token to, so a plugin that claimed to be a
//! synchroniser token would be claiming a secret it does not have.
//!
//! So this hook owns both halves. It mints a token, sets it as its own
//! cookie, and the form sends the same value back in a field. The cookie
//! and the field must match. A page on another site can make the browser
//! send the cookie, and cannot read it, so it cannot fill the field.
//!
//! # What this does not stop
//!
//! **Somebody who can write a cookie on this site's domain.** A sibling
//! subdomain over plain HTTP can set `csrf_token` for the parent, then
//! post the value it chose in both halves. Only a signed token bound to a
//! session closes that, and this crate has no session to bind to. Say it
//! here rather than in a post-mortem.
//!
//! **A body this hook cannot read.** The field is read out of an
//! `application/x-www-form-urlencoded` body and nothing else, because
//! reading one out of a `multipart/form-data` body means parsing an upload
//! in a hook, on every request. A form that uploads a file sends the token
//! in the header instead; [`CsrfConfig::header`] is what turns that on,
//! and it is on by default.
//!
//! # The two halves
//!
//! A [`RequestHook`] that refuses, and a [`ResponseHook`] that sets the
//! cookie. The second is not optional: a visitor with no cookie would
//! otherwise be refused for ever, with nothing to send back.
//!
//! The token for this request is on the request state as a [`CsrfToken`],
//! which is where a page reads it to write the hidden field.

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, Response, ResponseHead, ResponseHook,
};

use crate::form_field;

/// The width of a token: 256 bits, the same as every other secret in this
/// workspace mints.
const TOKEN_BYTES: usize = 32;

/// How long that is written in hex, which is the only form a token takes.
const TOKEN_CHARS: usize = TOKEN_BYTES * 2;

/// The token for this request.
///
/// On the request state, so a page can write the hidden field and a
/// handler can read what it checked against.
#[derive(Clone, Debug)]
pub struct CsrfToken {
    /// The value, and the same value is in the cookie.
    pub value: String,
    /// The form field this token goes in.
    pub field: String,
    /// The header a script sends it back in, when the configuration takes
    /// one.
    pub header: Option<String>,
    /// Whether this request minted it, so the response must set the
    /// cookie. Private: a hook writes it and a hook reads it.
    fresh: bool,
}

impl CsrfToken {
    /// The hidden input a form carries.
    ///
    /// Nothing here is escaped and nothing here needs to be: the value is
    /// hex, and [`CsrfConfig::check`] refuses a field name that is not
    /// letters, digits, `_` and `-` at startup.
    pub fn hidden_field(&self) -> String {
        format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            self.field, self.value
        )
    }

    /// The `<meta>` tag dp-swap's mutating verbs read the token from.
    ///
    /// It goes in the layout's `<head>`. The script sends the value in
    /// the header this tag names, or in the field when the configuration
    /// takes no header. Not escaped, for the reason
    /// [`hidden_field`](Self::hidden_field) gives.
    pub fn meta_tag(&self) -> String {
        match &self.header {
            Some(header) => format!(
                r#"<meta name="csrf-token" content="{}" data-header="{}">"#,
                self.value, header
            ),
            None => format!(
                r#"<meta name="csrf-token" content="{}" data-field="{}">"#,
                self.value, self.field
            ),
        }
    }
}

/// Where the token lives, and who does not have to send one.
#[derive(Clone, Debug)]
pub struct CsrfConfig {
    /// The cookie this hook owns. It is this hook's own cookie, not the
    /// session's: nothing else reads or writes it.
    pub cookie: String,
    /// The form field the token comes back in.
    pub field: String,
    /// The header a fetch caller may send instead, or `None` to take the
    /// field and nothing else.
    ///
    /// A script reads the cookie and sets the header, so the cookie is
    /// `HttpOnly` only while this is `None`. A page that renders the
    /// hidden field server-side needs no script and wants `None`.
    pub header: Option<String>,
    /// Paths that are never checked, for the endpoint whose caller is not
    /// a browser: a payment provider's webhook, a device posting a
    /// reading.
    ///
    /// An entry matches the path itself and everything under it, so
    /// `/webhooks` covers `/webhooks/stripe` and does not cover
    /// `/webhooksecret`. An entry of `/` exempts the whole application,
    /// which is what it says.
    pub exempt: Vec<String>,
    /// `Secure`, so the cookie never leaves over plain HTTP. False in
    /// development and nowhere else.
    pub secure: bool,
}

impl Default for CsrfConfig {
    /// The shape an HTML application wants: a cookie, a field, the header
    /// for fetch callers, nothing exempt.
    fn default() -> Self {
        Self {
            cookie: "csrf_token".to_string(),
            field: "_csrf_token".to_string(),
            header: Some("x-csrf-token".to_string()),
            exempt: Vec::new(),
            secure: true,
        }
    }
}

impl CsrfConfig {
    /// Name the cookie something else.
    pub fn cookie(mut self, name: impl Into<String>) -> Self {
        self.cookie = name.into();
        self
    }

    /// Name the form field something else.
    pub fn field(mut self, name: impl Into<String>) -> Self {
        self.field = name.into();
        self
    }

    /// Take the token from this header as well as the field.
    pub fn header(mut self, name: impl Into<String>) -> Self {
        self.header = Some(name.into());
        self
    }

    /// Take the field and nothing else, and keep the cookie `HttpOnly`.
    pub fn without_header(mut self) -> Self {
        self.header = None;
        self
    }

    /// Paths whose callers are not browsers.
    pub fn exempt(mut self, paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.exempt = paths.into_iter().map(Into::into).collect();
        self
    }

    /// Drop `Secure` from the cookie. Development only.
    pub fn insecure(mut self) -> Self {
        self.secure = false;
        self
    }

    /// Why this configuration cannot work, if it cannot.
    ///
    /// Checked when the plugin installs its hooks, so a name that cannot
    /// go in a header or an attribute is a startup error rather than a
    /// request that fails later.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.cookie.is_empty() || self.field.is_empty() {
            return Err("the cookie and the field both need a name");
        }
        if !is_name(&self.cookie) {
            return Err(
                "the cookie name goes in a `Set-Cookie` header: keep it to letters, \
                        digits, `_` and `-`",
            );
        }
        if !is_name(&self.field) {
            return Err(
                "the field name goes in an HTML attribute unescaped: keep it to \
                        letters, digits, `_` and `-`",
            );
        }
        if self.header.as_deref().is_some_and(|h| !is_name(h)) {
            return Err(
                "the header name goes in a header and in an HTML attribute: keep it \
                        to letters, digits, `_` and `-`",
            );
        }
        Ok(())
    }
}

/// Letters, digits, `_` and `-`, which is what both names are allowed to
/// be. Narrower than either specification, and wide enough for every name
/// anybody writes.
fn is_name(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Refuses an unsafe request that did not carry the token.
pub struct Csrf {
    config: CsrfConfig,
}

impl Plugin for Csrf {
    type Config = CsrfConfig;
    type Provides = NoContext;
    const NAME: &'static str = "csrf";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for Csrf {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .check()
            .map_err(|why| PluginError::new(format!("csrf: {}", why)))?;
        registrar.request(
            "check",
            // After the method override, so a POST that asked to be a
            // DELETE is checked as the verb it became. Both verbs are
            // unsafe, so this is about what a log says rather than what
            // is refused -- and `_if_present`, because the override is
            // its own plugin and most applications do not register it.
            brocanth::plugin_order::Order::new().after_if_present("method-override.rewrite"),
            Hook {
                config: self.config.clone(),
            },
        );
        registrar.response(
            "mint",
            brocanth::plugin_order::Order::new(),
            Hook {
                config: self.config.clone(),
            },
        );
        Ok(())
    }
}

struct Hook {
    config: CsrfConfig,
}

impl Hook {
    /// Whether this path is one nobody has to send a token to.
    fn exempt(&self, path: &str) -> bool {
        self.config.exempt.iter().any(|entry| {
            // A trailing slash is the same list entry. Writing
            // `/webhooks/` and getting nothing is a trap that costs an
            // afternoon.
            let entry = entry.trim_end_matches('/');
            path == entry || (path.starts_with(entry) && path[entry.len()..].starts_with('/'))
        })
    }

    /// The token the client sent back, from the header or the field.
    ///
    /// The header first, because a fetch caller sends both halves itself
    /// and a form sends only the field.
    fn submitted(&self, request: &Request) -> Option<String> {
        if let Some(name) = &self.config.header {
            if let Some(sent) = request
                .headers()
                .get(name.as_str())
                .and_then(|value| value.to_str().ok())
            {
                return Some(sent.trim().to_string());
            }
        }
        form_field(request, &self.config.field)
    }

    /// The 403, and what to send instead.
    fn refuse(&self, why: &str) -> Flow {
        let how = match &self.config.header {
            Some(header) => format!(
                "send the `{}` field, or the `{}` header, with the value of the `{}` cookie",
                self.config.field, header, self.config.cookie
            ),
            None => format!(
                "send the `{}` field with the value of the `{}` cookie",
                self.config.field, self.config.cookie
            ),
        };
        let mut response = Response::new(Body::from(format!("{why}: {how}\n").into_bytes()));
        *response.status_mut() = http::StatusCode::FORBIDDEN;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        Flow::respond(response)
    }
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let present = cookie(request.headers(), &self.config.cookie).map(str::to_string);
        let held = present.as_deref().filter(|value| ours(value));

        // Minted before the check, and put on the state even when the
        // answer is a refusal: a visitor whose first act is a POST has no
        // cookie yet, and refusing without handing one out refuses the
        // retry as well.
        let value = match held {
            Some(value) => value.to_string(),
            None => mint()?,
        };
        state.insert(CsrfToken {
            value,
            field: self.config.field.clone(),
            header: self.config.header.clone(),
            fresh: held.is_none(),
        });

        if safe(request.method()) || self.exempt(request.uri().path()) {
            return Ok(Flow::Continue);
        }
        let Some(sent) = self.submitted(request) else {
            return Ok(self.refuse("this request carried no CSRF token"));
        };
        let Some(held) = held else {
            return Ok(self.refuse("this request carried no CSRF cookie"));
        };
        if !ct_eq(sent.as_bytes(), held.as_bytes()) {
            return Ok(self.refuse("the CSRF token does not match the cookie"));
        }
        Ok(Flow::Continue)
    }
}

impl ResponseHook for Hook {
    /// Headers only, and a stream is no different: a `Set-Cookie` does not
    /// depend on the bytes of the body.
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let Some(token) = state.get::<CsrfToken>() else {
            // The request hook did not run, so this response belongs to
            // no token and must not carry one.
            return Ok(());
        };
        // Appended rather than inserted, because a handler may already
        // vary by something. A page holds this request's token, and a
        // shared cache that served it to the next visitor would hand out
        // somebody else's.
        head.headers
            .append(http::header::VARY, http::HeaderValue::from_static("Cookie"));
        if !token.fresh {
            // The client already has it. Setting it again on every
            // response would reset the cookie under a second tab that
            // has just been handed a different one.
            return Ok(());
        }
        let mut cookie = format!(
            "{}={}; Path=/; SameSite=Lax",
            self.config.cookie, token.value
        );
        if self.config.header.is_none() {
            // Nothing reads it from script, so nothing should be able to.
            cookie.push_str("; HttpOnly");
        }
        if self.config.secure {
            cookie.push_str("; Secure");
        }
        let value = http::HeaderValue::from_str(&cookie)
            .map_err(|_| PluginError::new("csrf: the cookie is not a header value"))?;
        head.headers.append(http::header::SET_COOKIE, value);
        Ok(())
    }
}

/// The verbs that change nothing, so nothing has to be proved to use one.
///
/// RFC 9110 §9.2.1's list. `OPTIONS` and `TRACE` are on it, and leaving
/// them off would refuse a preflight that never reaches a handler.
fn safe(method: &http::Method) -> bool {
    matches!(
        *method,
        http::Method::GET | http::Method::HEAD | http::Method::OPTIONS | http::Method::TRACE
    )
}

/// One cookie out of the request.
///
/// A request may carry more than one `Cookie` header and each holds a
/// `;`-separated list, so both are walked.
fn cookie<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|line| line.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key.trim() == name).then(|| value.trim())
        })
}

/// Whether this value has the shape of a token this hook minted.
///
/// Hygiene and not a defence: the right shape proves nothing. It is here
/// so a value somebody else's code left under this name is replaced
/// rather than carried, and so the comparison below is always over the
/// same width.
fn ours(value: &str) -> bool {
    value.len() == TOKEN_CHARS && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A fresh token, from the operating system's CSPRNG.
///
/// There is no fallback. A token a visitor can guess is worse than a
/// request that failed, so a machine with no randomness gets a 500 and a
/// line saying why.
fn mint() -> Result<String, PluginError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| PluginError::new(format!("csrf: no OS randomness, so no token: {e}")))?;
    let mut out = String::with_capacity(TOKEN_CHARS);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("a nibble is one hex digit"));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("a nibble is one hex digit"));
    }
    Ok(out)
}

/// Equality that does not say where it stopped.
///
/// `==` on two slices returns at the first byte that differs, and a
/// caller who can time the answer recovers a token one byte at a time.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    fold(a, b).0 == 0
}

/// The accumulated difference, and how many byte pairs it took.
///
/// Every byte of the longer input goes into the accumulator, and so do
/// the two lengths, so the loop runs `max(len)` times whatever the inputs
/// are. The lengths are not the secret; where two tokens stop matching
/// is, and a comparison that returned early on a short token would say so.
///
/// The count is the second half of the answer so a test can read it:
/// a loop that always runs to the end is the proof that nothing returns
/// early.
fn fold(a: &[u8], b: &[u8]) -> (usize, usize) {
    let mut diff = a.len() ^ b.len();
    let mut seen = 0;
    for at in 0..a.len().max(b.len()) {
        let x = a.get(at).copied().unwrap_or(0);
        let y = b.get(at).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
        seen += 1;
    }
    (diff, seen)
}

/// The token the request hook minted, for a test that needs a valid one.
#[cfg(test)]
fn minted(state: &RequestState) -> String {
    state.get::<CsrfToken>().expect("a token").value.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook() -> Hook {
        Hook {
            config: CsrfConfig::default(),
        }
    }

    fn request(method: http::Method, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
        let mut builder = http::Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(body.as_bytes().to_vec()).expect("a request")
    }

    /// A form post, with the cookie and the field spelled out.
    fn form(cookie: Option<&str>, field: Option<&str>) -> Request {
        let mut headers = vec![("content-type", "application/x-www-form-urlencoded")];
        let jar;
        if let Some(value) = cookie {
            jar = format!("csrf_token={value}");
            headers.push(("cookie", jar.as_str()));
        }
        let body = match field {
            Some(value) => format!("title=Hi&_csrf_token={value}"),
            None => "title=Hi".to_string(),
        };
        request(http::Method::POST, "/posts", &headers, &body)
    }

    fn run(hook: &Hook, mut request: Request) -> (Flow, RequestState) {
        let mut state = RequestState::new();
        let flow = hook
            .before_dispatch(&mut request, &mut state)
            .expect("the hook answers");
        (flow, state)
    }

    fn status(flow: &Flow) -> Option<http::StatusCode> {
        match flow {
            Flow::Continue => None,
            Flow::Respond(response) => Some(response.status()),
        }
    }

    /// A token this hook would accept, minted the way it mints them.
    fn token() -> String {
        mint().expect("the machine has randomness")
    }

    #[test]
    fn a_safe_verb_is_never_challenged() {
        for verb in [
            http::Method::GET,
            http::Method::HEAD,
            http::Method::OPTIONS,
            http::Method::TRACE,
        ] {
            let (flow, state) = run(&hook(), request(verb.clone(), "/posts", &[], ""));
            assert_eq!(status(&flow), None, "{verb} was challenged");
            // And it still gets a token, because the page it is about to
            // render is the page that holds the form.
            assert!(ours(&minted(&state)), "{verb} got no token to render");
        }
    }

    #[test]
    fn a_post_with_no_token_is_refused() {
        let (flow, _) = run(&hook(), form(Some(&token()), None));
        assert_eq!(status(&flow), Some(http::StatusCode::FORBIDDEN));
    }

    #[test]
    fn a_post_with_the_wrong_token_is_refused() {
        let (flow, _) = run(&hook(), form(Some(&token()), Some(&token())));
        assert_eq!(status(&flow), Some(http::StatusCode::FORBIDDEN));
    }

    #[test]
    fn a_post_with_the_right_token_passes() {
        let value = token();
        let (flow, _) = run(&hook(), form(Some(&value), Some(&value)));
        assert_eq!(status(&flow), None);
    }

    #[test]
    fn a_token_minted_for_one_cookie_does_not_match_another() {
        // The whole pattern in one test: the field is checked against the
        // cookie that arrived, not against anything the server kept, so a
        // token lifted from another visitor's page is worth nothing
        // without their cookie.
        let mine = token();
        let theirs = token();
        let (flow, _) = run(&hook(), form(Some(&theirs), Some(&mine)));
        assert_eq!(status(&flow), Some(http::StatusCode::FORBIDDEN));
    }

    #[test]
    fn the_comparison_does_not_stop_where_the_bytes_do() {
        let token = token();
        let mut wrong = token.clone();
        wrong.replace_range(0..1, "0");
        let mut late = token.clone();
        late.replace_range(TOKEN_CHARS - 1.., "0");

        // Differing at the first byte and differing at the last byte cost
        // the same number of steps, and so does a length that does not
        // match at all.
        let steps = |other: &str| fold(token.as_bytes(), other.as_bytes()).1;
        assert_eq!(steps(&wrong), TOKEN_CHARS);
        assert_eq!(steps(&late), TOKEN_CHARS);
        assert_eq!(steps(""), TOKEN_CHARS, "an empty token returned early");
        assert_eq!(steps(&token.repeat(2)), TOKEN_CHARS * 2);

        // And a prefix is not a match, which is what the lengths in the
        // accumulator are for.
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"a"));
        assert!(ct_eq(token.as_bytes(), token.as_bytes()));
    }

    #[test]
    fn an_exempt_path_is_not_checked() {
        let hook = Hook {
            config: CsrfConfig::default().exempt(["/webhooks"]),
        };
        let under = request(http::Method::POST, "/webhooks/stripe", &[], "");
        assert_eq!(status(&run(&hook, under).0), None);
        let itself = request(http::Method::POST, "/webhooks", &[], "");
        assert_eq!(status(&run(&hook, itself).0), None);
        // A prefix of a path segment is not that segment.
        let elsewhere = request(http::Method::POST, "/webhooksecret", &[], "");
        assert_eq!(
            status(&run(&hook, elsewhere).0),
            Some(http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn the_header_carries_the_token_for_a_fetch_caller() {
        let value = token();
        let jar = format!("csrf_token={value}");
        let sent = request(
            http::Method::POST,
            "/posts",
            &[("cookie", jar.as_str()), ("x-csrf-token", value.as_str())],
            "{}",
        );
        assert_eq!(status(&run(&hook(), sent).0), None);

        // And it is refused where the application said the field only.
        let only_fields = Hook {
            config: CsrfConfig::default().without_header(),
        };
        let again = request(
            http::Method::POST,
            "/posts",
            &[("cookie", jar.as_str()), ("x-csrf-token", value.as_str())],
            "{}",
        );
        assert_eq!(
            status(&run(&only_fields, again).0),
            Some(http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn a_first_visit_is_handed_a_cookie() {
        let hook = hook();
        let (_, mut state) = run(&hook, request(http::Method::GET, "/posts", &[], ""));
        assert_eq!(set_cookie(&hook, &mut state).len(), 1);
    }

    #[test]
    fn a_cookie_that_is_already_right_is_not_set_again() {
        // A second tab has its own in-flight response, and resetting the
        // cookie under it would invalidate the form it is rendering.
        let hook = hook();
        let value = token();
        let (_, mut state) = run(&hook, form(Some(&value), Some(&value)));
        assert!(set_cookie(&hook, &mut state).is_empty());
    }

    #[test]
    fn a_refusal_still_hands_out_a_cookie() {
        // Otherwise a visitor whose first act is a POST is refused for
        // ever: no cookie, so no token, so no cookie.
        let hook = hook();
        let (flow, mut state) = run(&hook, form(None, None));
        assert_eq!(status(&flow), Some(http::StatusCode::FORBIDDEN));
        let set = set_cookie(&hook, &mut state);
        assert_eq!(set.len(), 1, "the retry has nothing to send");
        assert!(set[0].contains("SameSite=Lax"), "{:?}", set[0]);
        assert!(set[0].contains("Secure"), "{:?}", set[0]);
    }

    #[test]
    fn a_foreign_value_under_this_name_is_replaced() {
        let hook = hook();
        let (flow, mut state) = run(&hook, form(Some("not-a-token"), Some("not-a-token")));
        assert_eq!(
            status(&flow),
            Some(http::StatusCode::FORBIDDEN),
            "a value of our own shape is what the cookie has to hold"
        );
        assert!(ours(&minted(&state)));
        assert_eq!(set_cookie(&hook, &mut state).len(), 1);
    }

    #[test]
    fn the_cookie_is_http_only_only_where_no_script_reads_it() {
        let value = |config: CsrfConfig| {
            let hook = Hook { config };
            let (_, mut state) = run(&hook, request(http::Method::GET, "/", &[], ""));
            set_cookie(&hook, &mut state).remove(0)
        };
        assert!(!value(CsrfConfig::default()).contains("HttpOnly"));
        assert!(value(CsrfConfig::default().without_header()).contains("HttpOnly"));
        assert!(!value(CsrfConfig::default().insecure()).contains("Secure"));
    }

    #[test]
    fn a_multipart_body_is_never_parsed() {
        // The same answer the method override gives, for the same reason:
        // reading a field out of an upload means parsing one in a hook. A
        // form that uploads sends the header.
        let value = token();
        let jar = format!("csrf_token={value}");
        let upload = request(
            http::Method::POST,
            "/posts",
            &[
                ("cookie", jar.as_str()),
                ("content-type", "multipart/form-data; boundary=xyz"),
            ],
            &format!("--xyz\r\nContent-Disposition: form-data; name=\"_csrf_token\"\r\n\r\n{value}\r\n--xyz--"),
        );
        assert_eq!(
            status(&run(&hook(), upload).0),
            Some(http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn one_cookie_is_found_among_others() {
        let value = token();
        let jar = format!("other=1; csrf_token={value}; _app_session=xyz");
        let sent = request(
            http::Method::POST,
            "/posts",
            &[
                ("cookie", jar.as_str()),
                ("content-type", "application/x-www-form-urlencoded"),
            ],
            &format!("_csrf_token={value}"),
        );
        assert_eq!(status(&run(&hook(), sent).0), None);
    }

    #[test]
    fn a_name_that_cannot_be_written_is_a_startup_error() {
        assert!(CsrfConfig::default().cookie("").check().is_err());
        assert!(CsrfConfig::default().field("a\"b").check().is_err());
        assert!(CsrfConfig::default().cookie("a b").check().is_err());
        assert!(CsrfConfig::default().check().is_ok());
    }

    #[test]
    fn the_hidden_field_carries_the_token() {
        let (_, state) = run(&hook(), request(http::Method::GET, "/posts", &[], ""));
        let token = state.get::<CsrfToken>().expect("a token");
        assert_eq!(
            token.hidden_field(),
            format!(
                r#"<input type="hidden" name="_csrf_token" value="{}">"#,
                token.value
            )
        );
    }

    /// Every `Set-Cookie` the response hook writes for this request.
    fn set_cookie(hook: &Hook, state: &mut RequestState) -> Vec<String> {
        let (mut head, _) = Response::new(Body::from(Vec::new())).into_parts();
        hook.on_headers(
            &request(http::Method::GET, "/", &[], ""),
            &mut head,
            BodyShape::Buffered { len: 0 },
            state,
        )
        .expect("the hook answers");
        assert!(
            head.headers
                .get_all(http::header::VARY)
                .iter()
                .any(|v| v == "Cookie"),
            "a cached page would hold somebody else's token"
        );
        head.headers
            .get_all(http::header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().expect("ascii").to_string())
            .collect()
    }
}
