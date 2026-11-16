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
//! **Somebody who can write a cookie on this site's domain, under another
//! name.** A sibling subdomain, or a page over plain HTTP, can set a cookie
//! for the parent, then post the value it chose in both halves. The
//! default name, `__Host-csrf_token`, closes that: a browser stores a
//! `__Host-` cookie only from this exact host, over HTTPS, with no
//! `Domain`. A name without the prefix, or `secure = false` (which drops
//! the prefix, since no browser stores a `__Host-` cookie that is not
//! `Secure`), leaves it open. Only a signed token bound to a session
//! closes it without the prefix, and this crate has no session to bind
//! to. Say it here rather than in a post-mortem.
//!
//! # Why the field is masked
//!
//! The token is the same on every page until the cookie changes. A page
//! compressed with gzip or brotli that also reflects what an attacker
//! typed leaks a fixed secret a byte at a time through the response size
//! (BREACH). So [`CsrfToken::hidden_field`] and [`CsrfToken::meta_tag`]
//! write a masked form: a fresh random pad, then the pad XOR the token,
//! both in hex. The bytes differ on every response, and the check
//! unmasks before it compares. The cookie and a header copied from it
//! hold the token as it is, and that is accepted too.
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

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
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
    /// The hidden input a form carries, with the token masked.
    ///
    /// Nothing here is escaped and nothing here needs to be: the value is
    /// hex, and [`CsrfConfig::check`] refuses a field name that is not
    /// letters, digits, `_` and `-` at startup.
    pub fn hidden_field(&self) -> String {
        format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            self.field,
            self.masked()
        )
    }

    /// The token masked with a fresh pad: hex of `pad`, then hex of `pad`
    /// XOR the token. Different on every call, and the check reads it
    /// back. The token as it is when the OS has no randomness to give,
    /// which the check accepts as well.
    pub fn masked(&self) -> String {
        mask(&self.value).unwrap_or_else(|| self.value.clone())
    }

    /// The same HTML with the hidden field inside every form that posts.
    ///
    /// For markup this application did not template itself, such as the
    /// admin's screens. A form whose opening tag says `method="post"`, in
    /// any case, quoted with either quote or not quoted, with or without
    /// spaces around the `=`, gets the field right after that tag.
    /// A form that already carries the field is left alone. A GET form
    /// gets nothing: it does not need the token and would leak it into a
    /// URL.
    pub fn stamp_forms(&self, html: &str) -> String {
        let field = self.hidden_field();
        let marker = format!(r#"name="{}""#, self.field);
        let lower = html.to_ascii_lowercase();
        let mut out = String::with_capacity(html.len() + field.len() * 2);
        let mut at = 0;
        while let Some(found) = lower[at..].find("<form") {
            let open = at + found;
            let Some(len) = lower[open..].find('>') else {
                break;
            };
            let close = open + len + 1;
            let posts = method_of(&lower[open..close]) == Some("post");
            let rest = &lower[close..];
            let body = &rest[..rest.find("</form").unwrap_or(rest.len())];
            out.push_str(&html[at..close]);
            if posts && !body.contains(&marker.to_ascii_lowercase()) {
                out.push_str(&field);
            }
            at = close;
        }
        out.push_str(&html[at..]);
        out
    }

    /// The `<meta>` tag dp-swap's mutating verbs read the token from.
    ///
    /// It goes in the layout's `<head>`. The script sends the value in
    /// the header this tag names, or in the field when the configuration
    /// takes no header. Not escaped, for the reason
    /// [`hidden_field`](Self::hidden_field) gives.
    pub fn meta_tag(&self) -> String {
        let value = self.masked();
        match &self.header {
            Some(header) => {
                format!(r#"<meta name="csrf-token" content="{value}" data-header="{header}">"#)
            }
            None => format!(
                r#"<meta name="csrf-token" content="{value}" data-field="{}">"#,
                self.field
            ),
        }
    }
}

/// The value of the `method` attribute of an opening `<form ...>` tag,
/// already lower case: `method="post"`, `method='post'`, `method=post`,
/// `method = "post"`.
fn method_of(tag: &str) -> Option<&str> {
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(found) = tag[from..].find("method") {
        let start = from + found;
        from = start + "method".len();
        // A whole attribute name: not `data-method`, not `methods`.
        let before = start.checked_sub(1).map(|at| bytes[at]);
        if !before.is_some_and(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        return Some(match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let inner = &rest[1..];
                &inner[..inner.find(quote).unwrap_or(inner.len())]
            }
            _ => {
                let end = rest
                    .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                    .unwrap_or(rest.len());
                &rest[..end]
            }
        });
    }
    None
}

/// Where the token lives, and who does not have to send one.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct CsrfConfig {
    /// The cookie this hook owns. It is this hook's own cookie, not the
    /// session's: nothing else reads or writes it.
    ///
    /// `__Host-csrf_token` by default: a browser takes a `__Host-` cookie
    /// only from this host, over HTTPS, so a sibling subdomain cannot plant
    /// one. With `secure` off the prefix is dropped on the wire, because a
    /// browser stores no `__Host-` cookie that is not `Secure`.
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
            cookie: "__Host-csrf_token".to_string(),
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

    /// Drop `Secure` from the cookie. Development only. A `__Host-` prefix
    /// on the cookie name is dropped with it.
    pub fn insecure(mut self) -> Self {
        self.secure = false;
        self
    }

    /// The cookie's name on the wire: [`cookie`](Self::cookie), without a
    /// `__Host-` prefix when the cookie is not `Secure`.
    pub fn cookie_name(&self) -> &str {
        if self.secure {
            &self.cookie
        } else {
            self.cookie.strip_prefix("__Host-").unwrap_or(&self.cookie)
        }
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

    /// Publishes the configuration as a service, so a plugin that carries
    /// the token in a body this check does not read (the file routes copy
    /// it out of a multipart form) reads the same `field` and `header`
    /// instead of a copy of them that can drift.
    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut().insert(self.config.clone());
        Ok(())
    }
}

impl HookSource for Csrf {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .check()
            .map_err(|why| {
                PluginError::new(format!(
                    "csrf: {why}\nhelp: set `cookie`, `field` and `header` under [csrf] to letters, digits, `_` and `-`"
                ))
            })?;
        registrar.request(
            "check",
            // `Guard`, so it runs after the method override (`Identify`)
            // and checks a POST that asked to be a DELETE as the verb it
            // became. Last in its phase, so a hook that answers a caller
            // with no token (a webhook, a beacon) runs first and names
            // nothing.
            brocanth::plugin_order::Phase::Guard,
            brocanth::plugin_order::Order::new().last(),
            Hook {
                config: self.config.clone(),
            },
        );
        registrar.response(
            "mint",
            brocanth::plugin_order::Phase::Guard,
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
        let cookie = self.config.cookie_name();
        let how = match &self.config.header {
            Some(header) => format!(
                "send the `{}` field, or the `{header}` header, with the value of the `{cookie}` cookie",
                self.config.field
            ),
            None => format!(
                "send the `{}` field with the value of the `{cookie}` cookie",
                self.config.field
            ),
        };
        let mut response = Response::new(Body::from(format!("{why}\nhelp: {how}\n").into_bytes()));
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
        let present = cookie(request.headers(), self.config.cookie_name()).map(str::to_string);
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
        let sent = unmask(&sent);
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
        // Merged rather than inserted, because a handler may already
        // vary by something. A page holds this request's token, and a
        // shared cache that served it to the next visitor would hand out
        // somebody else's.
        crate::add_vary(&mut head.headers, "Cookie");
        if !token.fresh {
            // The client already has it. Setting it again on every
            // response would reset the cookie under a second tab that
            // has just been handed a different one.
            return Ok(());
        }
        let mut cookie = format!(
            "{}={}; Path=/; SameSite=Lax",
            self.config.cookie_name(),
            token.value
        );
        if self.config.header.is_none() {
            // Nothing reads it from script, so nothing should be able to.
            cookie.push_str("; HttpOnly");
        }
        if self.config.secure {
            cookie.push_str("; Secure");
        }
        let value = http::HeaderValue::from_str(&cookie).map_err(|_| {
            PluginError::new(
                "csrf: the cookie is not a header value\n\
                 help: set `cookie` under [csrf] to letters, digits, `_` and `-`",
            )
        })?;
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
    getrandom::fill(&mut bytes).map_err(|e| {
        PluginError::new(format!(
            "csrf: no OS randomness, so no token: {e}\n\
             help: let the process read the OS random source (getrandom(2) or /dev/urandom)"
        ))
    })?;
    Ok(hex(&bytes))
}

/// `pad || pad XOR token`, in hex, with a fresh pad. `None` when the OS has
/// no randomness or the token is not one this hook minted.
fn mask(token: &str) -> Option<String> {
    let raw = unhex(token).filter(|raw| raw.len() == TOKEN_BYTES)?;
    let mut pad = vec![0u8; raw.len()];
    getrandom::fill(&mut pad).ok()?;
    let masked: Vec<u8> = pad.iter().zip(&raw).map(|(p, t)| p ^ t).collect();
    Some(hex(&pad) + &hex(&masked))
}

/// The token inside a masked value, or the value as it came when it is
/// not masked: a fetch caller copies the cookie into the header as it is.
fn unmask(sent: &str) -> String {
    if sent.len() == TOKEN_CHARS * 2 {
        if let Some(bytes) = unhex(sent) {
            let (pad, masked) = bytes.split_at(TOKEN_BYTES);
            let token: Vec<u8> = pad.iter().zip(masked).map(|(p, m)| p ^ m).collect();
            return hex(&token);
        }
    }
    sent.to_string()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("a nibble is one hex digit"));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("a nibble is one hex digit"));
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

/// Equality that does not say where it stopped.
///
/// `==` on two slices returns at the first byte that differs, and a
/// caller who can time the answer recovers a token one byte at a time.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
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

    fn stamp_token() -> CsrfToken {
        CsrfToken {
            value: "abc123".into(),
            field: "_csrf_token".into(),
            header: None,
            fresh: false,
        }
    }

    const FIELD: &str = r#"<input type="hidden" name="_csrf_token" value=""#;

    #[test]
    fn every_posting_form_gets_the_field_and_a_get_form_does_not() {
        let html = r#"<form method="get"><input name="q"></form><FORM class="x" METHOD="POST" action="/a"><button></button></FORM><form method='post'></form>"#;
        let out = stamp_token().stamp_forms(html);
        assert_eq!(out.matches(FIELD).count(), 2, "{out}");
        assert!(out.contains(&format!(r#"action="/a">{FIELD}"#)), "{out}");
        assert!(
            out.starts_with(r#"<form method="get"><input name="q">"#),
            "{out}"
        );
    }

    #[test]
    fn an_unquoted_or_spaced_post_method_is_a_posting_form() {
        for html in [
            "<form method=post action=/a></form>",
            r#"<form method = "post"></form>"#,
            "<form action=/a method=POST></form>",
            "<form\n  method='Post'\n></form>",
        ] {
            let out = stamp_token().stamp_forms(html);
            assert_eq!(out.matches(FIELD).count(), 1, "{html} -> {out}");
        }
        for html in [
            "<form method=get></form>",
            r#"<form data-method="post"></form>"#,
            "<form></form>",
        ] {
            let out = stamp_token().stamp_forms(html);
            assert_eq!(out, html, "{html}");
        }
    }

    #[test]
    fn the_field_is_masked_differently_each_time_and_still_checks() {
        let value = token();
        let csrf = CsrfToken {
            value: value.clone(),
            field: "_csrf_token".into(),
            header: None,
            fresh: false,
        };
        let (one, two) = (csrf.masked(), csrf.masked());
        assert_ne!(one, two, "a fixed secret in a compressed page leaks");
        assert!(!one.contains(&value), "{one}");
        assert_eq!(unmask(&one), value);
        assert_eq!(unmask(&two), value);
        let (flow, _) = run(&hook(), form(Some(&value), Some(&one)));
        assert_eq!(status(&flow), None, "a masked field was refused");
        // The token as it is, as a fetch caller copies it from the cookie.
        let (flow, _) = run(&hook(), form(Some(&value), Some(&value)));
        assert_eq!(status(&flow), None);
    }

    #[test]
    fn the_default_cookie_is_host_only_and_drops_the_prefix_without_secure() {
        let hook = hook();
        let (_, mut state) = run(&hook, request(http::Method::GET, "/", &[], ""));
        let set = set_cookie(&hook, &mut state).remove(0);
        assert!(set.starts_with("__Host-csrf_token="), "{set}");
        assert!(set.contains("Path=/") && set.contains("Secure"), "{set}");
        assert!(!set.contains("Domain"), "{set}");

        let plain = Hook {
            config: CsrfConfig::default().insecure(),
        };
        let (_, mut state) = run(&plain, request(http::Method::GET, "/", &[], ""));
        let set = set_cookie(&plain, &mut state).remove(0);
        assert!(set.starts_with("csrf_token="), "{set}");
    }

    #[test]
    fn a_form_that_carries_the_field_is_left_alone() {
        let field = stamp_token().hidden_field();
        let html = format!(r#"<form method="post">{field}</form>"#);
        assert_eq!(stamp_token().stamp_forms(&html), html);
    }

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
            jar = format!("__Host-csrf_token={value}");
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
        let jar = format!("__Host-csrf_token={value}");
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
        let jar = format!("__Host-csrf_token={value}");
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
        let jar = format!("other=1; __Host-csrf_token={value}; _app_session=xyz");
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
        let field = token.hidden_field();
        let sent = field
            .strip_prefix(FIELD)
            .and_then(|rest| rest.strip_suffix(r#"">"#))
            .expect("the hidden input");
        assert_eq!(unmask(sent), token.value);
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
