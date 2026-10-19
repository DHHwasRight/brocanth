//! What came back, and the assertions a test makes about it.
//!
//! # One place that touches the core's `Response`
//!
//! [`TestResponse::from_core`] is the only function in this crate that reads
//! a [`brocanth::plugin_hooks::Response`]. Everything a test holds afterwards
//! is a status, a header list and a `Vec<u8>` that this crate owns. That is
//! deliberate: the core's response type is under active change, and a body
//! that became a stream would change this one function — collect the stream,
//! or hand back a future — and no assertion above it.

use crate::html::Html;
use brocanth::plugin_hooks::Response;

/// What a request produced.
#[derive(Debug, Clone)]
pub struct TestResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    chain: Vec<String>,
}

impl TestResponse {
    /// The core's response, read into something a test owns.
    ///
    /// The one boundary; see the module header.
    pub(crate) fn from_core(response: Response) -> Self {
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        TestResponse {
            status,
            headers,
            // The one place in this crate that reads a core `Response`,
            // which is why a streaming body changed this line and no
            // assertion above it. A stream is collected here, because a
            // test holds bytes and asserts on them.
            body: collect(response.into_body()),
            chain: Vec::new(),
        }
    }

    pub(crate) fn with_chain(mut self, chain: Vec<String>) -> Self {
        self.chain = chain;
        self
    }

    // -----------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------

    /// The status code.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// One header, case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every header, in the order the response carried them.
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// `location`, which is where a redirect points.
    #[must_use]
    pub fn location(&self) -> Option<&str> {
        self.header("location")
    }

    /// `content-type`.
    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    /// The body, as bytes.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as text.
    ///
    /// # Panics
    ///
    /// When the body is not UTF-8. A screen that is not UTF-8 is a bug worth
    /// a loud failure rather than a lossy one.
    #[must_use]
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or_else(|e| panic!("the body is not UTF-8: {e}"))
    }

    /// The body as a page to ask questions of.
    ///
    /// # Panics
    ///
    /// As [`TestResponse::text`].
    #[must_use]
    pub fn html(&self) -> Html {
        Html::new(self.text())
    }

    /// Whether this is a redirect a browser would follow.
    #[must_use]
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }

    /// Everywhere a followed request went, in order, ending at this response.
    ///
    /// Empty unless the request said
    /// [`following`](crate::TestRequest::following).
    #[must_use]
    pub fn redirect_chain(&self) -> &[String] {
        &self.chain
    }

    // -----------------------------------------------------------------
    // Assertions
    // -----------------------------------------------------------------

    /// The status is this.
    ///
    /// # Panics
    ///
    /// When it is not, quoting the body, because a 500 that should have been
    /// a 200 has its reason in there.
    pub fn assert_status(&self, status: u16) -> &Self {
        assert!(
            self.status == status,
            "expected {status}, got {}.\n{}",
            self.status,
            self.describe()
        );
        self
    }

    /// The status is 200.
    ///
    /// # Panics
    ///
    /// As [`TestResponse::assert_status`].
    pub fn assert_ok(&self) -> &Self {
        self.assert_status(200)
    }

    /// This is a redirect, and it points exactly here.
    ///
    /// Django's `assertRedirects`, minus the part that fetches the target:
    /// ask for that with [`following`](crate::TestRequest::following), which
    /// then also records the chain.
    ///
    /// # Panics
    ///
    /// When the status is not a redirect, or `location` is something else.
    pub fn assert_redirects(&self, location: &str) -> &Self {
        assert!(
            self.is_redirect(),
            "expected a redirect to `{location}`, got {}.\n{}",
            self.status,
            self.describe()
        );
        assert!(
            self.location() == Some(location),
            "expected a redirect to `{location}`, got {}",
            match self.location() {
                Some(other) => format!("one to `{other}`"),
                None => "a redirect with no `location` at all".to_string(),
            }
        );
        self
    }

    /// A header is exactly this.
    ///
    /// # Panics
    ///
    /// When it is not, naming what is there.
    pub fn assert_header(&self, name: &str, value: &str) -> &Self {
        assert!(
            self.header(name) == Some(value),
            "expected `{name}: {value}`, got {}",
            match self.header(name) {
                Some(other) => format!("`{name}: {other}`"),
                None => format!("no `{name}` header"),
            }
        );
        self
    }

    /// The raw bytes of the body contain this.
    ///
    /// **The escape hatch, and not the first thing to reach for.** A
    /// substring of markup is the assertion §G6 was written about: it passes
    /// on a path inside a comment and breaks when an attribute is reordered.
    /// Prefer [`Html::assert_text`] for what a reader sees and
    /// [`Html::assert_link_to`] for where a page goes.
    ///
    /// # Panics
    ///
    /// When it does not.
    pub fn assert_contains(&self, needle: &str) -> &Self {
        assert!(
            self.text().contains(needle),
            "the body does not contain `{needle}`.\n{}",
            self.describe()
        );
        self
    }

    /// The status, the headers, and the body, for a failure message.
    ///
    /// An HTML body is shown as the text a reader would see. A screen carries
    /// its stylesheet inline, so the markup is twenty-five kilobytes of
    /// Tailwind with the one readable line somewhere inside it — and a
    /// failure nobody reads is a failure that does not help.
    fn describe(&self) -> String {
        let body = match std::str::from_utf8(&self.body) {
            Err(_) => format!("{} bytes, not UTF-8", self.body.len()),
            Ok(text) if self.is_html() => truncate(&Html::new(text).text()),
            Ok(text) => truncate(text),
        };
        let headers = self
            .headers
            .iter()
            .map(|(name, value)| format!("  {name}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("--- {} ---\n{headers}\n\n{body}\n---", self.status)
    }

    fn is_html(&self) -> bool {
        self.content_type()
            .is_some_and(|kind| kind.starts_with("text/html"))
    }
}

/// A body long enough to bury the point, cut at a character boundary.
fn truncate(text: &str) -> String {
    const MOST: usize = 2000;
    if text.len() <= MOST {
        return text.to_string();
    }
    let mut cut = MOST;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… and {} more bytes", &text[..cut], text.len() - cut)
}

/// Every byte of a body, whichever shape it arrived in.
///
/// A buffered body is already the answer. A streaming one is driven to its
/// end on this thread, because a test holds a whole response and asserts on
/// it — a client that handed back a half-read stream would make every
/// assertion racy.
///
/// It parks between chunks rather than giving up on `Pending`. An SSE body
/// fed by a broker on another thread is pending most of the time, and a
/// collector that stopped there would silently return a prefix. A stream
/// that never ends hangs the test, which is the honest failure: a response
/// that never ends is what the handler wrote.
fn collect(body: brocanth::plugin_body::Body) -> Vec<u8> {
    use brocanth::plugin_body::Body;
    use core::task::Poll;

    match body {
        Body::Buffered(bytes) => bytes,
        Body::Streaming(mut stream) => {
            let mut out = Vec::new();
            crate::rt::block_on(core::future::poll_fn(move |cx| loop {
                match stream.poll_chunk(cx) {
                    Poll::Ready(Some(Ok(chunk))) => out.extend_from_slice(&chunk),
                    // An error mid-body is not a status a test can see:
                    // the headers are long gone. What arrived is what a
                    // client would have had.
                    Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                        return Poll::Ready(core::mem::take(&mut out))
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }))
        }
    }
}
