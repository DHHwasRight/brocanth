//! The server half of dp-swap's mutating verbs.
//!
//! dp-swap fetches with GET. `assets/dpmutate.js` ([`SCRIPT`]) adds
//! `dp-post`, `dp-put`, `dp-patch` and `dp-delete` on the client. This
//! module is what a handler answering one of those requests needs:
//!
//! - [`DpRequest`]: what the request says about itself — that it came
//!   from dp-swap, how many layout layers the client already holds
//!   (`DP-Partial`), and which element it will swap (`DP-Target`).
//! - [`Directive`]: the response headers that steer the client. Typed, so
//!   a handler cannot ask for a redirect and a retarget at once.
//! - [`Oob`]: an out-of-band fragment, "and also replace the cart badge".
//! - [`DpSwap`]: a plugin that puts [`DpRequest`] on the request state and
//!   sets `Vary`, because a page that answers `DP-Partial` with less than
//!   the whole document must not be cached as the whole document.
//!
//! The CSRF token rides in a header that [`Csrf`](crate::Csrf) already
//! reads. [`CsrfToken::meta_tag`](crate::CsrfToken::meta_tag) writes the
//! tag the script reads it from. The 422 re-render is
//! `brocanth_input::rerender`, which is where a form's faults are.

use std::fmt;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, ResponseHead, ResponseHook,
};

/// The client script, to inline in a `<script>` after dp-swap's or to
/// serve beside it.
///
/// It does nothing until dp-swap is on the page, and then it intercepts
/// only elements that carry a `dp-<verb>` attribute. It holds no
/// `</script>`, so it can go inline as it is.
pub const SCRIPT: &str = include_str!("../assets/dpmutate.js");

/// The prefetch script: `dp-prefetch` on a link fetches the page before
/// the click, on `hover`, `tap`, `viewport` or `load`, as Astro's prefetch
/// does.
///
/// Same-origin GET links only, and nothing when the browser asks to save
/// data. It needs no dp-swap and no server half: the request is a plain
/// `<link rel="prefetch">`. It holds no `</script>`, so it can go inline,
/// and it guards against a second copy on the page as [`SCRIPT`] does.
pub const PREFETCH: &str = include_str!("../assets/dpprefetch.js");

/// The request header dp-swap sends on every request it makes.
pub const REQUEST: &str = "dp-request";
/// How many layout layers the client already holds.
pub const PARTIAL: &str = "dp-partial";
/// The id of the element the client will swap, when it has one.
pub const TARGET: &str = "dp-target";

/// What a dp-swap request says about itself.
///
/// On the request state when [`DpSwap`] is installed. Absent for a plain
/// browser request, a crawler and a client with JavaScript off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DpRequest {
    /// How many layout layers to leave out, counted from the root. Zero
    /// sends the whole document.
    pub partial: u32,
    /// The id of the element the client will swap, if it has one.
    pub target: Option<String>,
}

impl DpRequest {
    /// The request's dp-swap headers, or `None` when it carries none.
    pub fn from_headers(headers: &http::HeaderMap) -> Option<DpRequest> {
        let read = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        if read(REQUEST).is_none() && read(PARTIAL).is_none() {
            return None;
        }
        Some(DpRequest {
            partial: partial_depth(read(PARTIAL)),
            target: read(TARGET)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        })
    }
}

/// How many layout layers a `DP-Partial` value asks to leave out.
///
/// `true` is dp-swap's word for "a fragment, I did not count", which is one
/// layer. Absent, or anything that does not parse, is zero: a client that
/// cannot say what it holds gets everything, which is always correct and
/// only slower.
pub fn partial_depth(value: Option<&str>) -> u32 {
    match value.map(str::trim) {
        None => 0,
        Some("true") => 1,
        Some(raw) => raw.parse().unwrap_or(0),
    }
}

/// How the client puts the response into the page. dp-swap's eight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Replace the target's children.
    InnerHtml,
    /// Replace the target.
    OuterHtml,
    /// Insert before the target.
    BeforeBegin,
    /// Insert as the target's first child.
    AfterBegin,
    /// Insert as the target's last child.
    BeforeEnd,
    /// Insert after the target.
    AfterEnd,
    /// Remove the target and ignore the body.
    Delete,
    /// Swap nothing. Out-of-band fragments still apply.
    None,
}

impl Style {
    /// The word dp-swap reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            Style::InnerHtml => "innerHTML",
            Style::OuterHtml => "outerHTML",
            Style::BeforeBegin => "beforebegin",
            Style::AfterBegin => "afterbegin",
            Style::BeforeEnd => "beforeend",
            Style::AfterEnd => "afterend",
            Style::Delete => "delete",
            Style::None => "none",
        }
    }
}

impl fmt::Display for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the client does with the response, as response headers.
///
/// Three shapes, and they exclude each other: leaving the page makes a
/// retarget meaningless, and a reload replaces everything a swap would
/// have touched. An enum is what makes "redirect and also swap" a thing a
/// handler cannot write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Directive {
    /// Leave this page for another: `DP-Redirect`. The client does a full
    /// navigation, because a new URL means new layouts.
    Redirect(String),
    /// Reload this page: `DP-Refresh`.
    Refresh,
    /// Swap the body, steered.
    Swap(Steer),
}

/// How a swap is steered. Every part is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Steer {
    retarget: Option<String>,
    reswap: Option<Style>,
    trigger: Vec<String>,
}

impl Directive {
    /// Leave for `location`.
    pub fn redirect(location: impl Into<String>) -> Directive {
        Directive::Redirect(location.into())
    }

    /// A swap with nothing changed yet.
    pub fn swap() -> Directive {
        Directive::Swap(Steer::default())
    }

    /// Swap into this selector, not the one the element named:
    /// `DP-Retarget`. A 422 uses it to put the form back where the form is.
    ///
    /// On a redirect or a refresh this is ignored: there is nothing to
    /// retarget.
    #[must_use]
    pub fn retarget(mut self, selector: impl Into<String>) -> Directive {
        if let Directive::Swap(steer) = &mut self {
            steer.retarget = Some(selector.into());
        }
        self
    }

    /// Swap in this style, not the one the element named: `DP-Reswap`.
    #[must_use]
    pub fn reswap(mut self, style: Style) -> Directive {
        if let Directive::Swap(steer) = &mut self {
            steer.reswap = Some(style);
        }
        self
    }

    /// Fire this event on the element that made the request, after the
    /// swap: `DP-Trigger`. Repeatable.
    #[must_use]
    pub fn trigger(mut self, event: impl Into<String>) -> Directive {
        if let Directive::Swap(steer) = &mut self {
            steer.trigger.push(event.into());
        }
        self
    }

    /// The headers, as names and values.
    ///
    /// For a response type that takes headers one by one, such as
    /// `brocanth_server::Response::with_header`.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        match self {
            Directive::Redirect(location) => vec![("dp-redirect", location.clone())],
            Directive::Refresh => vec![("dp-refresh", "true".to_string())],
            Directive::Swap(steer) => {
                let mut out = Vec::new();
                if let Some(selector) = &steer.retarget {
                    out.push(("dp-retarget", selector.clone()));
                }
                if let Some(style) = steer.reswap {
                    out.push(("dp-reswap", style.as_str().to_string()));
                }
                if !steer.trigger.is_empty() {
                    out.push(("dp-trigger", steer.trigger.join(",")));
                }
                out
            }
        }
    }

    /// Write the headers onto a response.
    ///
    /// # Errors
    ///
    /// When a value cannot be a header value: a selector or a URL with a
    /// line break in it.
    pub fn apply(
        &self,
        headers: &mut http::HeaderMap,
    ) -> Result<(), http::header::InvalidHeaderValue> {
        for (name, value) in self.headers() {
            headers.insert(name, http::HeaderValue::from_str(&value)?);
        }
        Ok(())
    }
}

/// A fragment the client puts somewhere other than the target.
///
/// It goes in the same body as the main fragment, before or after it.
/// dp-swap finds it by its `dp-swap-oob` attribute, removes it from the
/// body, and swaps it in by selector.
///
/// ```
/// use brocanth_middleware::dp::Oob;
///
/// let badge = Oob::replace(r#"<span id="cart-count">3</span>"#);
/// assert_eq!(
///     badge.to_string(),
///     r#"<span dp-swap-oob="outerHTML" id="cart-count">3</span>"#,
/// );
/// ```
///
/// The wrapped forms put the markup in a `<div>`, which the browser's
/// parser rejects for table parts: an `Oob::append` of a `<tr>` arrives
/// empty. Replace the `<tbody>` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Oob(String);

impl Oob {
    /// Replace the element with the same id as the root of `html`.
    ///
    /// The root must carry an `id`: that is how the client finds what it
    /// replaces. `html` is trusted markup, as every rendered fragment is.
    pub fn replace(html: &str) -> Oob {
        let trimmed = html.trim_start();
        let name_end = trimmed
            .char_indices()
            .skip(1)
            .find(|(_, c)| c.is_whitespace() || *c == '>' || *c == '/')
            .map(|(at, _)| at);
        match (trimmed.starts_with('<'), name_end) {
            (true, Some(at)) => Oob(format!(
                r#"{} dp-swap-oob="outerHTML"{}"#,
                &trimmed[..at],
                &trimmed[at..]
            )),
            // Not an element. Wrapped, so it still reaches the page.
            _ => Oob::wrapped(Style::OuterHtml, "", html),
        }
    }

    /// Replace the children of `selector`.
    pub fn inner(selector: &str, html: &str) -> Oob {
        Oob::wrapped(Style::InnerHtml, selector, html)
    }

    /// Add to the end of `selector`.
    pub fn append(selector: &str, html: &str) -> Oob {
        Oob::wrapped(Style::BeforeEnd, selector, html)
    }

    /// Add to the start of `selector`.
    pub fn prepend(selector: &str, html: &str) -> Oob {
        Oob::wrapped(Style::AfterBegin, selector, html)
    }

    /// Remove `selector`.
    pub fn delete(selector: &str) -> Oob {
        Oob::wrapped(Style::Delete, selector, "")
    }

    fn wrapped(style: Style, selector: &str, html: &str) -> Oob {
        let how = if selector.is_empty() {
            style.as_str().to_string()
        } else {
            format!("{}:{}", style.as_str(), selector)
        };
        Oob(format!(
            r#"<div dp-swap-oob="{}">{}</div>"#,
            escape_attribute(&how),
            html
        ))
    }

    /// The markup.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Oob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Enough escaping for a value between double quotes.
fn escape_attribute(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// Puts [`DpRequest`] on the request state and sets `Vary`.
///
/// Optional. A handler can call [`DpRequest::from_headers`] itself. The
/// plugin is what makes `Vary: DP-Request, DP-Partial` impossible to
/// forget: the same URL answers a whole document to a browser and a
/// fragment to dp-swap, and a shared cache that does not know that serves
/// one to the other.
pub struct DpSwap;

impl Plugin for DpSwap {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "dp-swap";
    const INSTALLS_HOOKS: bool = true;

    fn new(_config: Self::Config) -> Self {
        DpSwap
    }
}

impl HookSource for DpSwap {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request("read", brocanth::plugin_order::Order::new(), Hook);
        registrar.response("vary", brocanth::plugin_order::Order::new(), Hook);
        Ok(())
    }
}

struct Hook;

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if let Some(dp) = DpRequest::from_headers(request.headers()) {
            state.insert(dp);
        }
        Ok(Flow::Continue)
    }
}

impl ResponseHook for Hook {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        // Appended, so a handler's own `Vary` stays.
        head.headers.append(
            http::header::VARY,
            http::HeaderValue::from_static("DP-Request, DP-Partial"),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> http::HeaderMap {
        let mut map = http::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, http::HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn a_plain_request_is_not_a_dp_request() {
        assert_eq!(DpRequest::from_headers(&headers(&[])), None);
    }

    #[test]
    fn the_partial_depth_is_read_as_dp_swap_writes_it() {
        assert_eq!(partial_depth(None), 0);
        assert_eq!(partial_depth(Some("true")), 1);
        assert_eq!(partial_depth(Some("3")), 3);
        assert_eq!(partial_depth(Some("lots")), 0);
        let dp = DpRequest::from_headers(&headers(&[
            ("dp-request", "true"),
            ("dp-partial", "2"),
            ("dp-target", "cart"),
        ]))
        .expect("a dp request");
        assert_eq!(dp.partial, 2);
        assert_eq!(dp.target.as_deref(), Some("cart"));
    }

    #[test]
    fn a_redirect_cannot_carry_a_retarget() {
        let directive = Directive::redirect("/done")
            .retarget("#form")
            .trigger("saved");
        assert_eq!(directive.headers(), [("dp-redirect", "/done".to_string())]);
    }

    #[test]
    fn a_swap_carries_every_part_it_was_given() {
        let directive = Directive::swap()
            .retarget("#form")
            .reswap(Style::OuterHtml)
            .trigger("saved")
            .trigger("cart:changed");
        let mut map = http::HeaderMap::new();
        directive.apply(&mut map).expect("valid values");
        assert_eq!(map["dp-retarget"], "#form");
        assert_eq!(map["dp-reswap"], "outerHTML");
        assert_eq!(map["dp-trigger"], "saved,cart:changed");
    }

    #[test]
    fn a_value_with_a_line_break_is_an_error_not_a_split_header() {
        let mut map = http::HeaderMap::new();
        assert!(Directive::redirect("/a\r\nset-cookie: x=1")
            .apply(&mut map)
            .is_err());
    }

    #[test]
    fn an_oob_replace_marks_the_root_element() {
        assert_eq!(
            Oob::replace(r#"<li id="n">1</li>"#).as_str(),
            r#"<li dp-swap-oob="outerHTML" id="n">1</li>"#
        );
        assert_eq!(
            Oob::replace("<hr/>").as_str(),
            r#"<hr dp-swap-oob="outerHTML"/>"#
        );
    }

    #[test]
    fn a_wrapped_oob_names_its_selector_escaped() {
        assert_eq!(
            Oob::append(r#"[data-x="1"]"#, "<b>hi</b>").as_str(),
            r#"<div dp-swap-oob="beforeend:[data-x=&quot;1&quot;]"><b>hi</b></div>"#
        );
        assert_eq!(
            Oob::delete("#gone").as_str(),
            r#"<div dp-swap-oob="delete:#gone"></div>"#
        );
    }

    #[test]
    fn the_script_can_be_inlined() {
        assert!(!SCRIPT.contains("</script"));
        for verb in ["dp-post", "dp-put", "dp-patch", "dp-delete"] {
            assert!(SCRIPT.contains(&verb[3..]), "{verb}");
        }
        for header in [
            "DP-Redirect",
            "DP-Retarget",
            "DP-Reswap",
            "DP-Trigger",
            "DP-Refresh",
        ] {
            assert!(SCRIPT.contains(header), "{header}");
        }
    }
}
