//! Messages that survive one redirect.
//!
//! A form posts, the handler saves, and the answer is a redirect: the page
//! the browser lands on is another request. "Saved" has to cross that gap.
//! Rails calls the carrier the flash, Django calls it messages.
//!
//! A page writes a message for the next page:
//!
//! ```ignore
//! let flash = ctx.hooked_mut::<Flash>().expect("the flash hook ran");
//! flash.success("Your password is changed.");
//! Ok(Response::empty(303).with_header("location", "/"))
//! ```
//!
//! The response hook puts the messages in a signed cookie. The request hook
//! of the next request reads them back onto the state, and the response
//! hook of that request renders them into the page, at the element
//! `<div id="flash">`, which the layout carries. Then the cookie is cleared.
//!
//! # Decisions
//!
//! * **The messages live in a cookie of their own, not in a session
//!   store.** A flash is a few words for one browser for one request. A
//!   cookie carries that with no table, no lookup and no sweep, and it works
//!   for a visitor who has no session. Rails keeps the flash in its cookie
//!   session for the same reason.
//! * **The cookie is signed.** Each message is shown on a page of this
//!   site. Unsigned, a cookie set from a sibling subdomain could show a
//!   visitor "your account is locked, call this number" in the site's own
//!   words. The key is `$<key_env>`; without it each process draws a random
//!   key, and the only cost is a message lost when a fleet sends the next
//!   request to another instance.
//! * **A message stays until a page shows it.** A redirect has no body, a
//!   JSON answer has no layout, and a partial swap leaves the layout out.
//!   Each of those keeps the messages for the next page that has the
//!   `<div id="flash">` element. Django's messages work the same way.
//! * **The markup is the application's.** [`FlashConfig::render`] takes a
//!   function. Without one, each message is a `<p>` with the level as its
//!   class, which a stylesheet can reach.
//! * **The hook draws, not the page.** A page that redirects, a page that
//!   fails, and every page that never heard of the flash still show the
//!   messages, because the response hook puts them into the layout. A
//!   page that wants them elsewhere calls [`Flash::take`].

use base64::Engine as _;
use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, ResponseHead, ResponseHook,
};
use sha2::Digest as _;

/// The start of the element the messages go into. The layout carries it.
pub const FLASH_MARKER: &str = r#"<div id="flash""#;

/// The most a flash cookie carries, in bytes of its value. A browser keeps
/// about 4096 per cookie, name and attributes included.
const MAX_VALUE: usize = 3000;

/// How serious a message is. The class a page gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Something happened.
    Info,
    /// Something the visitor asked for worked.
    Success,
    /// It worked, and something needs attention.
    Warning,
    /// It did not work.
    Error,
}

impl Level {
    /// `info`, `success`, `warning` or `error`.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Success => "success",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }

    fn code(self) -> char {
        match self {
            Level::Info => 'i',
            Level::Success => 's',
            Level::Warning => 'w',
            Level::Error => 'e',
        }
    }

    fn from_code(code: char) -> Option<Self> {
        Some(match code {
            'i' => Level::Info,
            's' => Level::Success,
            'w' => Level::Warning,
            'e' => Level::Error,
            _ => return None,
        })
    }
}

/// One message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashMessage {
    /// How serious it is.
    pub level: Level,
    /// What it says. Plain text: a renderer escapes it.
    pub text: String,
}

/// The flash of this request, on the request state.
///
/// `ctx.hooked_mut::<Flash>()` in a page. The messages a page writes with
/// [`Flash::push`] and its four short forms are for the next request; the
/// ones it writes with [`Flash::now`] are for this response.
#[derive(Debug, Default)]
pub struct Flash {
    incoming: Vec<FlashMessage>,
    now: Vec<FlashMessage>,
    next: Vec<FlashMessage>,
    shown: bool,
    had_cookie: bool,
}

impl Flash {
    /// A message for the next request.
    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        self.next.push(FlashMessage {
            level,
            text: text.into(),
        });
    }

    /// [`Level::Info`], for the next request.
    pub fn info(&mut self, text: impl Into<String>) {
        self.push(Level::Info, text);
    }

    /// [`Level::Success`], for the next request.
    pub fn success(&mut self, text: impl Into<String>) {
        self.push(Level::Success, text);
    }

    /// [`Level::Warning`], for the next request.
    pub fn warning(&mut self, text: impl Into<String>) {
        self.push(Level::Warning, text);
    }

    /// [`Level::Error`], for the next request.
    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Level::Error, text);
    }

    /// A message for this response: a form drawn again with what went
    /// wrong, with no redirect between.
    pub fn now(&mut self, level: Level, text: impl Into<String>) {
        self.now.push(FlashMessage {
            level,
            text: text.into(),
        });
    }

    /// The messages this response shows, without taking them.
    pub fn messages(&self) -> impl Iterator<Item = &FlashMessage> {
        self.incoming.iter().chain(&self.now)
    }

    /// The messages this response shows. They count as shown, so the next
    /// request does not see them again.
    ///
    /// For a page that draws them itself. A page with the layout's
    /// `<div id="flash">` needs none of this: the hook fills it.
    pub fn take(&mut self) -> Vec<FlashMessage> {
        self.shown = true;
        let mut out = std::mem::take(&mut self.incoming);
        out.append(&mut self.now);
        out
    }

    /// Carries the messages that arrived to the next request as well.
    pub fn keep(&mut self) {
        self.next.splice(0..0, std::mem::take(&mut self.incoming));
    }

    /// The messages the next request gets: the new ones, and those that
    /// arrived and were not shown.
    fn outgoing(&self) -> Vec<&FlashMessage> {
        let carried = if self.shown {
            &[][..]
        } else {
            &self.incoming[..]
        };
        carried.iter().chain(&self.next).collect()
    }
}

/// A function that turns messages into HTML. Each `text` is plain text, so
/// the function escapes it.
pub type Render = fn(&[FlashMessage]) -> String;

/// The cookie, the key and the markup.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct FlashConfig {
    /// The cookie this hook owns.
    pub cookie: String,
    /// The name of the environment variable that holds the signing key, not
    /// the key. Unset or empty: a random key per process.
    pub key_env: String,
    /// `Secure`, so the cookie never leaves over plain HTTP. False in
    /// development and nowhere else.
    pub secure: bool,
    /// The markup for the messages. `None` writes one `<p>` per message.
    #[cfg_attr(feature = "config", serde(skip))]
    pub render: Option<Render>,
}

impl Default for FlashConfig {
    fn default() -> Self {
        Self {
            cookie: "flash".to_string(),
            key_env: "FLASH_KEY".to_string(),
            secure: true,
            render: None,
        }
    }
}

impl FlashConfig {
    /// Draw the messages with this function.
    #[must_use]
    pub fn render(mut self, render: Render) -> Self {
        self.render = Some(render);
        self
    }

    /// Drop `Secure` from the cookie. Development only.
    #[must_use]
    pub fn insecure(mut self) -> Self {
        self.secure = false;
        self
    }

    /// Why this configuration cannot work, if it cannot.
    pub fn check(&self) -> Result<(), &'static str> {
        let name_ok = !self.cookie.is_empty()
            && self
                .cookie
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !name_ok {
            return Err(
                "the cookie name goes in a `Set-Cookie` header: keep it to letters, \
                        digits, `_` and `-`",
            );
        }
        Ok(())
    }
}

/// One `<p>` per message, with the level as its class.
pub fn default_render(messages: &[FlashMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        out.push_str(&format!(
            r#"<p class="flash flash-{}">{}</p>"#,
            message.level.as_str(),
            escape(&message.text)
        ));
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Carries messages across one redirect, in a signed cookie.
pub struct FlashPlugin {
    config: FlashConfig,
}

impl Plugin for FlashPlugin {
    type Config = FlashConfig;
    type Provides = NoContext;
    const NAME: &'static str = "flash";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for FlashPlugin {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config.check().map_err(|why| {
            PluginError::new(format!(
                "flash: {why}\nhelp: set [flash] cookie to a plain name"
            ))
        })?;
        let key = match std::env::var(&self.config.key_env) {
            Ok(key) if !key.is_empty() => key.into_bytes(),
            _ => {
                let mut bytes = vec![0u8; 32];
                getrandom::fill(&mut bytes).map_err(|e| {
                    PluginError::new(format!(
                        "flash: no OS randomness for a key: {e}\n\
                         help: set the variable that `key_env` names, or let the process read the OS random source"
                    ))
                })?;
                bytes
            }
        };
        let hook = Hook {
            config: self.config.clone(),
            key,
        };
        registrar
            .request("read", brocanth::plugin_order::Order::new(), hook.clone())
            .response("write", brocanth::plugin_order::Order::new(), hook);
        Ok(())
    }
}

#[derive(Clone)]
struct Hook {
    config: FlashConfig,
    key: Vec<u8>,
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let held = cookie(request.headers(), &self.config.cookie);
        state.insert(Flash {
            incoming: held.and_then(|value| self.open(value)).unwrap_or_default(),
            had_cookie: held.is_some(),
            ..Flash::default()
        });
        Ok(Flow::Continue)
    }
}

impl ResponseHook for Hook {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        // A stream has no body to render into, and `on_body` will not run:
        // write the cookie now, and keep what arrived.
        if let BodyShape::Streaming(_) = body {
            if let Some(flash) = state.get::<Flash>() {
                self.write_cookie(head, flash)?;
            }
        }
        Ok(())
    }

    fn on_body(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let Some(flash) = state.get_mut::<Flash>() else {
            return Ok(());
        };
        let shows = !flash.shown
            && flash.messages().next().is_some()
            && head.status.is_success()
            && is_html(head);
        if shows {
            if let Some(at) = insertion_point(body) {
                let messages = flash.take();
                let html = (self.config.render.unwrap_or(default_render))(&messages);
                body.splice(at..at, html.into_bytes());
                crate::add_vary(&mut head.headers, "Cookie");
            }
        }
        self.write_cookie(head, flash)
    }
}

impl Hook {
    /// The `Set-Cookie` that carries what the next request gets, or clears
    /// the cookie when nothing is left.
    fn write_cookie(&self, head: &mut ResponseHead, flash: &Flash) -> Result<(), PluginError> {
        let outgoing = flash.outgoing();
        let line = if outgoing.is_empty() {
            if !flash.had_cookie {
                return Ok(());
            }
            format!("{}=; Path=/; Max-Age=0", self.config.cookie)
        } else if outgoing.len() == flash.incoming.len() && flash.next.is_empty() {
            // The same messages as arrived: the cookie the browser holds is
            // already right.
            return Ok(());
        } else {
            format!("{}={}; Path=/", self.config.cookie, self.seal(&outgoing))
        };
        let mut line = format!("{line}; HttpOnly; SameSite=Lax");
        if self.config.secure {
            line.push_str("; Secure");
        }
        let value = http::HeaderValue::from_str(&line).map_err(|_| {
            PluginError::new(
                "flash: the cookie is not a header value\n\
                 help: set `cookie` under [flash] to letters, digits, `_` and `-`",
            )
        })?;
        head.headers.append(http::header::SET_COOKIE, value);
        Ok(())
    }

    /// The cookie value: the messages, then a dot, then the signature.
    ///
    /// Messages that do not fit are dropped from the end. A flash is a few
    /// words; a page that writes kilobytes into one has a bug, and a cookie
    /// the browser refuses would lose every message rather than the last.
    fn seal(&self, messages: &[&FlashMessage]) -> String {
        let mut payload = String::new();
        for message in messages {
            let part = format!("{}{}", message.level.code(), b64(message.text.as_bytes()));
            if payload.len() + part.len() + 1 > MAX_VALUE - 44 {
                break;
            }
            if !payload.is_empty() {
                payload.push(',');
            }
            payload.push_str(&part);
        }
        let tag = b64(&hmac(&self.key, payload.as_bytes()));
        format!("{payload}.{tag}")
    }

    /// The messages in a cookie value, or `None` when it is not one this
    /// key signed.
    fn open(&self, value: &str) -> Option<Vec<FlashMessage>> {
        let (payload, tag) = value.rsplit_once('.')?;
        let presented = unb64(tag)?;
        let expected = hmac(&self.key, payload.as_bytes());
        if !crate::csrf::ct_eq(&presented, &expected) {
            return None;
        }
        if payload.is_empty() {
            return Some(Vec::new());
        }
        payload
            .split(',')
            .map(|part| {
                let mut chars = part.chars();
                let level = Level::from_code(chars.next()?)?;
                let text = String::from_utf8(unb64(chars.as_str())?).ok()?;
                Some(FlashMessage { level, text })
            })
            .collect()
    }
}

/// Whether the response is an HTML page.
fn is_html(head: &ResponseHead) -> bool {
    head.headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        // A page that names no type is HTML in a generated project.
        .is_none_or(|t| t.trim_start().to_ascii_lowercase().starts_with("text/html"))
}

/// Where the messages go: after the `>` of the layout's flash element.
fn insertion_point(body: &[u8]) -> Option<usize> {
    let marker = FLASH_MARKER.as_bytes();
    let start = body.windows(marker.len()).position(|w| w == marker)?;
    let close = body[start..].iter().position(|&b| b == b'>')?;
    Some(start + close + 1)
}

/// One cookie out of the request.
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
        .filter(|value| !value.is_empty())
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .ok()
}

/// HMAC-SHA256 (RFC 2104), over `sha2`, which this crate already links.
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&sha2::Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = sha2::Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = sha2::Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook() -> Hook {
        Hook {
            config: FlashConfig::default(),
            key: b"test key".to_vec(),
        }
    }

    fn request(cookie: Option<&str>) -> Request {
        let mut builder = http::Request::builder().uri("/");
        if let Some(value) = cookie {
            builder = builder.header("cookie", format!("flash={value}"));
        }
        builder.body(Vec::new()).expect("a request")
    }

    fn head(status: u16, content_type: &str) -> ResponseHead {
        http::Response::builder()
            .status(status)
            .header("content-type", content_type)
            .body(())
            .expect("a response")
            .into_parts()
            .0
    }

    /// Runs one request through both hooks, with `page` in between, and
    /// answers the `Set-Cookie` and the body.
    fn exchange(
        cookie: Option<&str>,
        status: u16,
        body: &str,
        page: impl FnOnce(&mut Flash),
    ) -> (Option<String>, String) {
        let (head, body) = exchange_with(
            &hook(),
            cookie,
            status,
            "text/html; charset=utf-8",
            body,
            page,
        );
        let set = head
            .headers
            .get("set-cookie")
            .map(|v| v.to_str().expect("text").to_string());
        (set, body)
    }

    /// [`exchange`] with the hook and the content type chosen, answering
    /// the whole response head.
    fn exchange_with(
        hook: &Hook,
        cookie: Option<&str>,
        status: u16,
        content_type: &str,
        body: &str,
        page: impl FnOnce(&mut Flash),
    ) -> (ResponseHead, String) {
        let mut request = match cookie {
            Some(value) => http::Request::builder()
                .uri("/")
                .header("cookie", format!("{}={value}", hook.config.cookie))
                .body(Vec::new())
                .expect("a request"),
            None => request(None),
        };
        let mut state = RequestState::new();
        hook.before_dispatch(&mut request, &mut state)
            .expect("read");
        page(state.get_mut::<Flash>().expect("the hook put a flash"));
        let mut head = head(status, content_type);
        let mut bytes = body.as_bytes().to_vec();
        hook.on_body(&request, &mut head, &mut bytes, &mut state)
            .expect("write");
        (head, String::from_utf8(bytes).expect("utf-8"))
    }

    fn value(set_cookie: &str) -> &str {
        set_cookie
            .strip_prefix("flash=")
            .and_then(|rest| rest.split(';').next())
            .expect("a flash cookie")
    }

    const PAGE: &str = r#"<body><div id="flash" role="status"></div><main>x</main></body>"#;

    #[test]
    fn a_message_crosses_one_redirect_and_is_shown_once() {
        // POST: the page writes a message and redirects.
        let (set, _) = exchange(None, 303, "", |flash| flash.success("Saved <b>"));
        let set = set.expect("the redirect carries the cookie");
        assert!(set.contains("HttpOnly") && set.contains("Secure"), "{set}");

        // GET: the layout shows it, escaped, and the cookie is cleared.
        let (cleared, body) = exchange(Some(value(&set)), 200, PAGE, |_| {});
        assert!(
            body.contains(r#"<div id="flash" role="status"><p class="flash flash-success">Saved &lt;b&gt;</p></div>"#),
            "{body}"
        );
        let cleared = cleared.expect("the shown message is cleared");
        assert!(cleared.contains("Max-Age=0"), "{cleared}");
    }

    #[test]
    fn a_page_without_the_element_keeps_the_messages() {
        let (set, _) = exchange(None, 303, "", |flash| flash.error("Nope"));
        let held = set.expect("a cookie");
        let (again, body) = exchange(Some(value(&held)), 200, "<p>partial</p>", |_| {});
        assert_eq!(body, "<p>partial</p>");
        assert!(
            again.is_none(),
            "the browser keeps the cookie it has: {again:?}"
        );
    }

    #[test]
    fn a_cookie_signed_with_another_key_is_ignored() {
        let (set, _) = exchange(None, 303, "", |flash| flash.info("Hello"));
        let held = value(&set.expect("a cookie")).to_string();
        let forged = format!("i{}.{}", b64(b"Call 555"), held.rsplit_once('.').unwrap().1);
        let (cleared, body) = exchange(Some(&forged), 200, PAGE, |_| {});
        assert!(!body.contains("555"), "{body}");
        assert!(cleared
            .expect("a bad cookie is cleared")
            .contains("Max-Age=0"));
    }

    #[test]
    fn now_shows_on_this_response_and_does_not_carry() {
        let (set, body) = exchange(None, 200, PAGE, |flash| {
            flash.now(Level::Warning, "Check it")
        });
        assert!(body.contains("flash-warning"), "{body}");
        assert!(set.is_none(), "nothing for the next request: {set:?}");
    }

    #[test]
    fn a_page_that_takes_the_messages_draws_them_itself() {
        let (set, _) = exchange(None, 303, "", |flash| flash.info("One"));
        let held = set.expect("a cookie");
        let mut seen = Vec::new();
        let (cleared, body) = exchange(Some(value(&held)), 200, PAGE, |flash| {
            seen = flash.take();
        });
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].text, "One");
        assert!(!body.contains("One"), "taken, so not drawn twice: {body}");
        assert!(cleared.is_some());
    }

    #[test]
    fn a_cookie_that_is_not_one_this_hook_wrote_shows_nothing_and_is_cleared() {
        let (set, _) = exchange(None, 303, "", |flash| flash.info("Hello"));
        let held = value(&set.expect("a cookie")).to_string();
        let (payload, tag) = held.rsplit_once('.').expect("payload.tag");
        let mut flipped = tag.as_bytes().to_vec();
        flipped[0] = if flipped[0] == b'A' { b'B' } else { b'A' };
        let flipped = format!("{payload}.{}", String::from_utf8(flipped).unwrap());
        for garbage in [
            "no-dot-at-all",
            ".",
            "x.not!base64",
            &format!("{payload}."),
            &format!(".{tag}"),
            &flipped,
            &format!("{payload}x.{tag}"),
        ] {
            let mut seen = Vec::new();
            let (cleared, body) = exchange(Some(garbage), 200, PAGE, |flash| {
                seen = flash.messages().cloned().collect();
            });
            assert!(seen.is_empty(), "{garbage:?} opened: {seen:?}");
            assert!(!body.contains("Hello"), "{garbage:?}: {body}");
            assert!(
                cleared.is_some_and(|c| c.contains("Max-Age=0")),
                "{garbage:?} is cleared"
            );
        }
    }

    #[test]
    fn a_shown_message_is_gone_on_the_request_after() {
        let (set, _) = exchange(None, 303, "", |flash| flash.success("Saved."));
        let (cleared, first) = exchange(Some(value(&set.unwrap())), 200, PAGE, |_| {});
        assert!(first.contains("Saved."));
        assert!(cleared.unwrap().contains("Max-Age=0"));
        // The browser applied the clear: the next request carries nothing.
        let (set, second) = exchange(None, 200, PAGE, |_| {});
        assert!(!second.contains("Saved."), "{second}");
        assert!(set.is_none());
    }

    #[test]
    fn keep_carries_what_arrived_to_the_next_request_too() {
        let (set, _) = exchange(None, 303, "", |flash| flash.info("First"));
        let (kept, _) = exchange(Some(value(&set.unwrap())), 303, "", |flash| {
            flash.keep();
            flash.warning("Second");
        });
        let kept = kept.expect("a new cookie");
        let mut seen = Vec::new();
        exchange(Some(value(&kept)), 200, PAGE, |flash| {
            seen = flash.take();
        });
        let texts: Vec<&str> = seen.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["First", "Second"]);
        assert_eq!(seen[1].level, Level::Warning);
    }

    #[test]
    fn an_error_page_or_a_json_answer_keeps_the_messages() {
        let (set, _) = exchange(None, 303, "", |flash| flash.error("Nope"));
        let held = set.unwrap();
        let (_, body) = exchange(Some(value(&held)), 404, PAGE, |_| {});
        assert!(!body.contains("Nope"), "not on an error page: {body}");
        let (head, body) = exchange_with(
            &hook(),
            Some(value(&held)),
            200,
            "application/json",
            r##"{"html":"<div id="flash"></div>"}"##,
            |_| {},
        );
        assert!(!body.contains("Nope"), "not in JSON: {body}");
        assert!(head.headers.get("set-cookie").is_none(), "the cookie stays");
        let (_, body) = exchange(Some(value(&held)), 200, PAGE, |_| {});
        assert!(body.contains("Nope"), "the next page shows it: {body}");
    }

    #[test]
    fn every_character_html_gives_meaning_to_is_escaped() {
        let text = r#"<img src=x onerror='alert(1)'> & "quoted", dotted. é"#;
        let (set, _) = exchange(None, 303, "", |flash| flash.info(text));
        let mut seen = Vec::new();
        let (_, body) = exchange(Some(value(&set.unwrap())), 200, PAGE, |flash| {
            seen = flash.messages().cloned().collect();
        });
        assert_eq!(seen[0].text, text, "the text crosses the cookie unchanged");
        assert!(!body.contains("<img"), "{body}");
        assert!(
            body.contains(
                "&lt;img src=x onerror=&#39;alert(1)&#39;&gt; &amp; &quot;quoted&quot;, dotted. é"
            ),
            "{body}"
        );
    }

    #[test]
    fn a_drawn_page_varies_by_cookie() {
        let (set, _) = exchange(None, 303, "", |flash| flash.info("Hi"));
        let (head, _) = exchange_with(
            &hook(),
            Some(value(&set.unwrap())),
            200,
            "text/html",
            PAGE,
            |_| {},
        );
        let vary = head.headers.get("vary").expect("a Vary header");
        assert!(vary.to_str().unwrap().contains("Cookie"), "{vary:?}");
    }

    #[test]
    fn messages_that_do_not_fit_a_cookie_are_dropped_from_the_end() {
        let (set, _) = exchange(None, 303, "", |flash| {
            flash.info("first");
            for _ in 0..10 {
                flash.info("x".repeat(1000));
            }
        });
        let set = set.expect("a cookie");
        assert!(
            set.len() < 4096,
            "a browser keeps the cookie: {}",
            set.len()
        );
        let mut seen = Vec::new();
        exchange(Some(value(&set)), 200, PAGE, |flash| seen = flash.take());
        assert_eq!(seen[0].text, "first");
        assert!(seen.len() < 11, "some were dropped");
    }

    #[test]
    fn the_cookie_name_and_secure_settings_reach_the_cookie() {
        let hook = Hook {
            config: FlashConfig {
                cookie: "notice".into(),
                ..FlashConfig::default()
            }
            .insecure(),
            key: b"test key".to_vec(),
        };
        let (head, _) = exchange_with(&hook, None, 303, "text/html", "", |flash| flash.info("Hi"));
        let set = head
            .headers
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(set.starts_with("notice="), "{set}");
        assert!(!set.contains("Secure"), "{set}");
        assert!(
            set.contains("HttpOnly") && set.contains("SameSite=Lax"),
            "{set}"
        );
        let value = set["notice=".len()..]
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let mut seen = Vec::new();
        exchange_with(&hook, Some(&value), 200, "text/html", PAGE, |flash| {
            seen = flash.take()
        });
        assert_eq!(seen.len(), 1, "read back under its own name");
        let mut under_another_name = http::HeaderMap::new();
        under_another_name.insert("cookie", format!("flash={value}").parse().unwrap());
        assert_eq!(
            cookie(&under_another_name, "notice"),
            None,
            "a cookie under another name is not this hook's"
        );
    }

    #[test]
    fn a_custom_renderer_draws_the_messages() {
        fn render(messages: &[FlashMessage]) -> String {
            format!("<ul>{}</ul>", messages.len())
        }
        let hook = Hook {
            config: FlashConfig::default().render(render),
            key: b"test key".to_vec(),
        };
        let (head, body) = exchange_with(&hook, None, 200, "text/html", PAGE, |flash| {
            flash.now(Level::Info, "one");
            flash.now(Level::Info, "two");
        });
        assert!(body.contains(r#"role="status"><ul>2</ul></div>"#), "{body}");
        assert!(head.headers.get("set-cookie").is_none());
    }

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let tag = hmac(b"Jefe", b"what do ya want for nothing?");
        let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_cookie_name_that_cannot_go_in_a_header_is_refused() {
        let config = FlashConfig {
            cookie: "fl ash".into(),
            ..FlashConfig::default()
        };
        assert!(config.check().is_err());
    }
}
