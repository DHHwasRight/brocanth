//! [`Beacon`]: the static side of the collector.
//!
//! A response hook that puts [`SNIPPET`] before the last `</body>` of every
//! HTML page (at the end of one that has none), except a dp-swap partial.
//! The static export runs pages through the hooks, so the files
//! it writes carry the snippet, and a static host serves them with no
//! server behind it. The snippet tells a brocanth server with the
//! `analytics` module, at [`BeaconConfig::collector`], that a page was read.
//!
//! The snippet sets no cookie and stores nothing in the browser. It sends
//! the path, the query and `document.referrer`; the collector reduces them
//! as the hook does. With [`BeaconConfig::respect_gpc`], a browser that
//! sends Global Privacy Control or Do Not Track sends nothing.

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, HookRegistrar, HookSource, OnResponse, Request, RequestState, ResponseHead,
    ResponseHook,
};
use brocanth::plugin_order::Order;

/// The script, the same text on every page and every site. What varies is
/// on the `<script>` element's `data-` attributes.
///
/// One request per page: `sendBeacon` with a form body, which is a
/// CORS-safelisted type and needs no preflight, or a 1×1 image when the
/// browser has no beacon. A dp-swap navigation changes the URL through
/// `history.pushState`, so the snippet wraps it and sends again; the back
/// button is `popstate`.
pub const SNIPPET: &str = r#"(function(){var s=document.currentScript,u=s&&s.getAttribute("data-collector");if(!u)return;if(s.getAttribute("data-gpc")==="1"&&(navigator.globalPrivacyControl||navigator.doNotTrack==="1"))return;function send(){var d="p="+encodeURIComponent(location.pathname+location.search)+"&r="+encodeURIComponent(document.referrer);try{if(navigator.sendBeacon&&navigator.sendBeacon(u,new Blob([d],{type:"application/x-www-form-urlencoded"})))return}catch(e){}var i=new Image(1,1);i.referrerPolicy="origin";i.src=u+(u.indexOf("?")<0?"?":"&")+d}send();var p=history.pushState;history.pushState=function(){var r=p.apply(this,arguments);send();return r};addEventListener("popstate",send)})();"#;

/// The beacon's settings: `[beacon]` in a config file.
#[derive(Clone, Debug, Default)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct BeaconConfig {
    /// The collector's absolute URL: `https://app.example.com/_a/v`.
    /// Unset, no page carries the snippet.
    pub collector: Option<String>,
    /// Send nothing from a browser with Global Privacy Control or Do Not
    /// Track on.
    pub respect_gpc: bool,
}

/// Puts the snippet in every HTML page.
pub struct Beacon {
    config: BeaconConfig,
}

impl Beacon {
    /// The `<script>` element this beacon inserts, or `None` without a
    /// collector.
    pub fn tag(&self) -> Option<String> {
        let url = self.config.collector.as_deref()?.trim();
        if url.is_empty() {
            return None;
        }
        let gpc = if self.config.respect_gpc {
            " data-gpc=\"1\""
        } else {
            ""
        };
        Some(format!(
            "<script data-collector=\"{}\"{gpc}>{SNIPPET}</script>",
            attribute(url)
        ))
    }
}

impl Plugin for Beacon {
    type Config = BeaconConfig;
    type Provides = NoContext;
    /// Also the configuration section.
    const NAME: &'static str = "beacon";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: BeaconConfig) -> Self {
        Self { config }
    }
}

impl HookSource for Beacon {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let Some(tag) = self.tag() else {
            return Ok(());
        };
        let url = self.config.collector.as_deref().unwrap_or("").trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(PluginError::new(format!(
                "[beacon] collector `{url}` is not an absolute URL\n\
                 help: name the analytics server's collector, such as \
                 `https://app.example.com/_a/v`"
            )));
        }
        registrar.response("insert", Order::new(), Insert { tag });
        Ok(())
    }
}

struct Insert {
    tag: String,
}

impl ResponseHook for Insert {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn on_body(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let html = head
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
        // A dp-swap partial is a fragment swapped into a page that already
        // runs the snippet; the snippet sends that navigation itself.
        let partial = request.headers().contains_key("dp-partial");
        if html && !partial {
            insert(body, &self.tag);
        }
        Ok(())
    }
}

/// Puts `tag` before the last `</body>`, in any case, or at the end of a
/// page that has none: `</body>` is optional in HTML, and a template that
/// leaves it out still renders a whole page.
pub fn insert(body: &mut Vec<u8>, tag: &str) {
    let lower = body.to_ascii_lowercase();
    let needle = b"</body>";
    match lower.windows(needle.len()).rposition(|w| w == needle) {
        Some(at) => {
            body.splice(at..at, tag.bytes());
        }
        None => body.extend_from_slice(tag.as_bytes()),
    }
}

fn attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tag_goes_before_the_last_body_end() {
        let mut page = b"<html><body><p>a</p></BODY></html>".to_vec();
        insert(&mut page, "<script></script>");
        assert_eq!(page, b"<html><body><p>a</p><script></script></BODY></html>");
        let mut bare = b"<!doctype html><h1>a</h1>".to_vec();
        insert(&mut bare, "<script></script>");
        assert_eq!(bare, b"<!doctype html><h1>a</h1><script></script>");
    }

    #[test]
    fn no_collector_is_no_tag() {
        assert_eq!(Beacon::new(BeaconConfig::default()).tag(), None);
        let tag = Beacon::new(BeaconConfig {
            collector: Some("https://a.example/_a/v?x=\"1\"".to_string()),
            respect_gpc: true,
        })
        .tag()
        .unwrap();
        assert!(tag.starts_with(
            "<script data-collector=\"https://a.example/_a/v?x=&quot;1&quot;\" data-gpc=\"1\">"
        ));
    }

    #[test]
    fn the_snippet_sets_no_cookie_and_stores_nothing() {
        for word in ["cookie", "localStorage", "sessionStorage", "indexedDB"] {
            assert!(!SNIPPET.contains(word), "{word}");
        }
    }
}
