//! The pages: what an error page is given, and the two pages this crate
//! writes itself.

use std::fmt::Write as _;

/// What an error page is given.
///
/// `not-found.rsx` and `error.rsx` declare `caught: &Caught` to read it.
/// A 4xx keeps its message, because the client caused it and the text says
/// what to change: "the title is empty". A 5xx has no message here at all:
/// the cause is for the log, and a page cannot show what it was not given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caught {
    /// The status the response carries: `404`, `500`.
    pub status: u16,
    /// Its reason phrase: `Not Found`.
    pub reason: &'static str,
    /// The message of a 4xx, for the visitor. `None` for a 5xx, and for a
    /// message that only repeats the reason phrase.
    pub message: Option<String>,
    /// The path that was asked for.
    pub path: String,
}

impl Caught {
    /// What a page is given for a failure with this status and message.
    ///
    /// ```
    /// use brocanth_errors::Caught;
    ///
    /// let missing = Caught::new(404, "no user 7", "/users/7");
    /// assert_eq!(missing.message.as_deref(), Some("no user 7"));
    ///
    /// let broken = Caught::new(500, "connection refused", "/users/7");
    /// assert_eq!(broken.message, None);
    /// assert_eq!(broken.reason, "Internal Server Error");
    /// ```
    pub fn new(status: u16, message: &str, path: &str) -> Self {
        let reason = reason(status);
        let message =
            (status < 500 && !message.is_empty() && message != reason).then(|| message.to_string());
        Self {
            status,
            reason,
            message,
            path: path.to_string(),
        }
    }

    /// Whether this is a 404: the page or the path does not exist.
    pub fn is_not_found(&self) -> bool {
        self.status == 404
    }
}

/// The reason phrase of a status, or `Error` for one this table lacks.
pub fn reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

/// Whether a client reads HTML, from its `Accept` header.
///
/// No header, `*/*`, `text/*`, `text/html` and `application/xhtml+xml` read
/// HTML. A client that asks for JSON only gets the plain answer it parses.
///
/// ```
/// use brocanth_errors::accepts_html;
///
/// assert!(accepts_html(None));
/// assert!(accepts_html(Some("text/html,application/xhtml+xml;q=0.9")));
/// assert!(accepts_html(Some("*/*")));
/// assert!(!accepts_html(Some("application/json")));
/// ```
pub fn accepts_html(accept: Option<&str>) -> bool {
    let Some(accept) = accept else {
        return true;
    };
    accept.split(',').any(|range| {
        let mut parts = range.split(';');
        let kind = parts.next().unwrap_or("").trim();
        // `q=0` says "not this one".
        let refused = parts.any(|p| {
            p.trim()
                .strip_prefix("q=")
                .is_some_and(|q| q.trim().parse::<f32>().is_ok_and(|q| q == 0.0))
        });
        !refused
            && ["*/*", "text/*", "text/html", "application/xhtml+xml"]
                .iter()
                .any(|html| kind.eq_ignore_ascii_case(html))
    })
}

/// A small HTML page for a project with no error page of its own, or for
/// an error page that failed to render.
///
/// ```
/// use brocanth_errors::{plain_page, Caught};
///
/// let html = plain_page(&Caught::new(404, "", "/nowhere"));
/// assert!(html.contains("<title>404 Not Found</title>"));
/// ```
pub fn plain_page(caught: &Caught) -> String {
    let mut body = String::new();
    if let Some(message) = &caught.message {
        let _ = writeln!(body, "<p>{}</p>", escape(message));
    }
    document(
        &format!("{} {}", caught.status, caught.reason),
        &format!("<h1>{}</h1>\n{body}", escape(caught.reason)),
    )
}

/// The page a debug build shows for a 5xx: the cause, the request line and
/// the request headers.
///
/// It stands alone, outside the layouts, because the layout can be what
/// failed. `cookie` and `authorization` show as `(hidden)`: a screenshot of
/// this page is a thing people share.
///
/// ```
/// use brocanth_errors::{development_page, Caught};
///
/// let caught = Caught::new(500, "invalid digit", "/users/x");
/// let html = development_page(&caught, "invalid digit", "GET", [("cookie", "s=1")]);
/// assert!(html.contains("invalid digit"));
/// assert!(html.contains("(hidden)"));
/// assert!(!html.contains("s=1"));
/// ```
pub fn development_page<'a>(
    caught: &Caught,
    cause: &str,
    method: &str,
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let mut rows = String::new();
    for (name, value) in headers {
        let secret = ["cookie", "authorization", "proxy-authorization"]
            .iter()
            .any(|s| name.eq_ignore_ascii_case(s));
        let value = if secret { "(hidden)" } else { value };
        let _ = writeln!(
            rows,
            "<tr><th>{}</th><td>{}</td></tr>",
            escape(name),
            escape(value)
        );
    }
    let body = format!(
        "<h1>{status} {reason}</h1>\n\
         <p><code>{method} {path}</code></p>\n\
         <h2>Cause</h2>\n\
         <pre>{cause}</pre>\n\
         <p>A debug build shows this page, and a release build never does. \
         To see <code>error.rsx</code> here instead, set \
         <code>details = false</code> under <code>[errors]</code>.</p>\n\
         <h2>Request headers</h2>\n\
         <table>\n{rows}</table>\n",
        status = caught.status,
        reason = escape(caught.reason),
        method = escape(method),
        path = escape(&caught.path),
        cause = escape(cause),
    );
    document(&format!("{} {}", caught.status, caught.reason), &body)
}

fn document(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n\
         <style>body{{font:16px/1.5 system-ui,sans-serif;max-width:48rem;margin:3rem auto;padding:0 1rem}}\
         pre{{white-space:pre-wrap;background:#f4f4f5;padding:1rem}}th{{text-align:left;padding-right:1rem}}</style>\n\
         </head>\n\
         <body>\n{body}</body>\n\
         </html>\n",
        escape(title)
    )
}

/// Text made safe for HTML content and attribute values.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_never_shows_markup_from_a_message() {
        let caught = Caught::new(422, "<script>x</script>", "/a\"b");
        let html = plain_page(&caught);
        assert!(!html.contains("<script>x"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        let html = development_page(&caught, "<b>", "GET", []);
        assert!(html.contains("/a&quot;b"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
    }

    #[test]
    fn a_message_that_only_repeats_the_reason_is_dropped() {
        assert_eq!(Caught::new(404, "Not Found", "/").message, None);
        assert!(Caught::new(404, "", "/").is_not_found());
    }

    #[test]
    fn a_range_with_q_zero_is_refused() {
        assert!(!accepts_html(Some("text/html;q=0, application/json")));
        assert!(accepts_html(Some("application/json, text/*;q=0.5")));
    }
}
