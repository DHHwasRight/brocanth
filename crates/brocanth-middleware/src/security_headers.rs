//! The response headers a browser reads as policy.
//!
//! Six headers, each a line somebody forgets in week two:
//!
//! | Header | Default | Says |
//! |---|---|---|
//! | `Strict-Transport-Security` | `max-age=31536000`, on HTTPS only | never come back over plain HTTP |
//! | `X-Content-Type-Options` | `nosniff` | do not guess a script out of a text file |
//! | `Referrer-Policy` | `strict-origin-when-cross-origin` | send another site the origin, not the path |
//! | `X-Frame-Options` | `DENY` | no page may frame this one |
//! | `Content-Security-Policy` | see [`Csp`] | what this page may load and run |
//! | `Permissions-Policy` | camera, microphone, geolocation, payment and USB off | which browser features a page may ask for |
//!
//! A header the handler already set is kept. A page that must be framed,
//! or that loads a script from a CDN, says so on its own response and
//! this hook does not overwrite it.
//!
//! # HSTS, and how this hook knows the request came over TLS
//!
//! HSTS sent over plain HTTP is ignored by the browser (RFC 6797 §8.1),
//! and sent from a development server on `localhost` it pins the name
//! for a year. So the header goes out only when the request is known to
//! be HTTPS:
//!
//! 1. the request URI has the `https` scheme, or
//! 2. [`ProxyHeaders`](crate::ProxyHeaders) ran and a trusted proxy said
//!    `proto=https`, or
//! 3. [`SecurityHeadersConfig::behind_tls`] is `true`: every request
//!    reaches this process through a TLS terminator.
//!
//! brocanth-server does not terminate TLS. A deployment that is not one
//! of the last two sends no HSTS, and `brocanth check --deploy` says so.
//!
//! `preload` is refused at startup unless `includeSubDomains` is on and
//! `max-age` is at least a year. Those are the preload list's own
//! entry rules, and a site submitted without them is rejected there.
//!
//! # The script policy, and why it is hashes and not a nonce
//!
//! dp-swap's two client scripts, [`dp::SCRIPT`](crate::dp::SCRIPT) and
//! [`dp::PREFETCH`](crate::dp::PREFETCH), go into a page inline:
//! `<script>{! brocanth_middleware::dp::SCRIPT }</script>`. A default
//! `script-src 'self'` blocks both.
//!
//! The usual escape is `'unsafe-inline'`, and that turns the policy off
//! for the attack it exists for. The two real choices are a nonce and a
//! hash:
//!
//! - **A nonce** is a fresh value per response, written on the header and
//!   on every `<script>` tag. Every template that inlines a script must
//!   read it from the request state. The admin's layout and the jobs tab
//!   inline the script from a constant and have no request in reach, so
//!   each would need a new parameter. A page that carries a nonce is also
//!   different for every visitor, which a shared cache must not store.
//! - **A hash** is `'sha256-…'` of the script text. The text is a
//!   compile-time constant, so the hash is too. No template changes, and
//!   a cached page stays valid.
//!
//! The render path is constants inlined by templates with no request, so
//! this hook uses hashes. [`Csp::default`] carries the hashes of both dp
//! scripts. An application that inlines its own script adds it with
//! [`Csp::inline_script`], which hashes the text it is given.
//!
//! The cost of a hash: an inline script whose text depends on the request
//! cannot have one. Write that value into a `data-` attribute or a
//! `<meta>` tag and read it from a constant script, as
//! [`CsrfToken::meta_tag`](crate::CsrfToken::meta_tag) does.
//!
//! # Styles
//!
//! `style-src` is `'self' 'unsafe-inline'` by default. The backoffice
//! screens carry `<style>` blocks and `style=` attributes, and a fragment
//! that dp-swap puts into the page may too. CSS injection reads less than
//! script injection does, and a default that breaks the backoffice gets
//! turned off rather than tuned. [`Csp::strict_styles`] drops it.

use std::collections::BTreeMap;

use base64::Engine as _;
use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, HookRegistrar, HookSource, OnResponse, Request, RequestState, ResponseHead,
    ResponseHook,
};
use http::header::{HeaderName, HeaderValue};
use sha2::Digest as _;

use crate::ClientInfo;

/// A year, in seconds: the preload list's minimum `max-age`.
pub const ONE_YEAR: u64 = 31_536_000;

/// The `Referrer-Policy` tokens a browser knows. Anything else is a typo,
/// and the browser ignores a typo silently.
const REFERRER_POLICIES: &[&str] = &[
    "no-referrer",
    "no-referrer-when-downgrade",
    "origin",
    "origin-when-cross-origin",
    "same-origin",
    "strict-origin",
    "strict-origin-when-cross-origin",
    "unsafe-url",
];

/// What the hook sends.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct SecurityHeadersConfig {
    /// `Strict-Transport-Security`.
    pub hsts: Hsts,
    /// Every request reaches this process through a TLS terminator, so
    /// HSTS goes out whatever the request URI says.
    ///
    /// False by default: a development server is plain HTTP, and HSTS
    /// from `localhost` pins the name in the browser for `max-age`.
    pub behind_tls: bool,
    /// `X-Content-Type-Options: nosniff`.
    pub nosniff: bool,
    /// `Referrer-Policy`, or `None` to send none.
    pub referrer_policy: Option<String>,
    /// `X-Frame-Options` and the CSP `frame-ancestors` directive.
    pub frames: Frames,
    /// `Content-Security-Policy`.
    pub csp: Csp,
    /// `Permissions-Policy`, or `None` to send none.
    pub permissions_policy: Option<String>,
}

impl Default for SecurityHeadersConfig {
    /// Every header on, at the value that breaks nothing brocanth renders.
    fn default() -> Self {
        Self {
            hsts: Hsts::default(),
            behind_tls: false,
            nosniff: true,
            referrer_policy: Some("strict-origin-when-cross-origin".to_string()),
            frames: Frames::Deny,
            csp: Csp::default(),
            permissions_policy: Some(
                "camera=(), microphone=(), geolocation=(), payment=(), usb=()".to_string(),
            ),
        }
    }
}

impl SecurityHeadersConfig {
    /// Send HSTS on every response: a TLS terminator is in front.
    #[must_use]
    pub fn behind_tls(mut self) -> Self {
        self.behind_tls = true;
        self
    }

    /// Replace the HSTS settings.
    #[must_use]
    pub fn hsts(mut self, hsts: Hsts) -> Self {
        self.hsts = hsts;
        self
    }

    /// Send no HSTS. For a deployment whose TLS terminator sets it.
    #[must_use]
    pub fn without_hsts(mut self) -> Self {
        self.hsts.enabled = false;
        self
    }

    /// Set the `Referrer-Policy`.
    #[must_use]
    pub fn referrer_policy(mut self, policy: impl Into<String>) -> Self {
        self.referrer_policy = Some(policy.into());
        self
    }

    /// Who may frame this application.
    #[must_use]
    pub fn frames(mut self, frames: Frames) -> Self {
        self.frames = frames;
        self
    }

    /// Replace the Content-Security-Policy.
    #[must_use]
    pub fn csp(mut self, csp: Csp) -> Self {
        self.csp = csp;
        self
    }

    /// Set the `Permissions-Policy`.
    #[must_use]
    pub fn permissions_policy(mut self, policy: impl Into<String>) -> Self {
        self.permissions_policy = Some(policy.into());
        self
    }

    /// Why this configuration cannot work, if it cannot.
    ///
    /// Checked when the plugin installs its hooks, so a value that is not
    /// a header, or a preload the list will refuse, stops the start.
    ///
    /// # Errors
    ///
    /// The reason, with what to change.
    pub fn check(&self) -> Result<(), String> {
        self.headers().map(|_| ())
    }

    /// The headers every response gets, built once. HSTS is apart,
    /// because it depends on the request.
    fn headers(&self) -> Result<Built, String> {
        let mut always = Vec::new();
        if self.nosniff {
            always.push((
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ));
        }
        if let Some(policy) = &self.referrer_policy {
            for token in policy.split(',').map(str::trim) {
                if !REFERRER_POLICIES.contains(&token) {
                    return Err(format!(
                        "`referrer_policy` has `{token}`, which no browser knows. \
                         help: use one of {}",
                        REFERRER_POLICIES.join(", ")
                    ));
                }
            }
            always.push((
                http::header::REFERRER_POLICY,
                value(policy, "referrer_policy")?,
            ));
        }
        if let Some(frame) = self.frames.x_frame_options() {
            always.push((
                http::header::X_FRAME_OPTIONS,
                HeaderValue::from_static(frame),
            ));
        }
        if self.csp.enabled {
            let name = if self.csp.report_only {
                http::header::CONTENT_SECURITY_POLICY_REPORT_ONLY
            } else {
                http::header::CONTENT_SECURITY_POLICY
            };
            always.push((name, value(&self.csp.render(&self.frames)?, "csp")?));
        }
        if let Some(policy) = &self.permissions_policy {
            always.push((
                HeaderName::from_static("permissions-policy"),
                value(policy, "permissions_policy")?,
            ));
        }
        let hsts = if self.hsts.enabled {
            Some(value(&self.hsts.render()?, "hsts")?)
        } else {
            None
        };
        Ok(Built {
            always,
            hsts,
            behind_tls: self.behind_tls,
        })
    }
}

fn value(raw: &str, key: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(raw).map_err(|_| {
        format!("`{key}` is not a header value. help: remove control characters and non-ASCII text")
    })
}

/// `Strict-Transport-Security`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct Hsts {
    /// Send it at all.
    pub enabled: bool,
    /// How long the browser remembers, in seconds.
    pub max_age: u64,
    /// The rule covers every subdomain too.
    ///
    /// Off by default. It is a statement about hosts this application may
    /// not serve, so it is the developer's to make.
    pub include_subdomains: bool,
    /// Ask to be on the browsers' built-in list.
    ///
    /// Off by default, and hard to undo: removal from the list takes
    /// months to reach users.
    pub preload: bool,
}

impl Default for Hsts {
    fn default() -> Self {
        Self {
            enabled: true,
            max_age: ONE_YEAR,
            include_subdomains: false,
            preload: false,
        }
    }
}

impl Hsts {
    /// HSTS for this many seconds.
    pub fn max_age(seconds: u64) -> Self {
        Self {
            max_age: seconds,
            ..Self::default()
        }
    }

    /// Cover every subdomain too.
    #[must_use]
    pub fn include_subdomains(mut self) -> Self {
        self.include_subdomains = true;
        self
    }

    /// Ask for the preload list. Needs `include_subdomains` and a year.
    #[must_use]
    pub fn preload(mut self) -> Self {
        self.preload = true;
        self
    }

    fn render(&self) -> Result<String, String> {
        if self.preload && !(self.include_subdomains && self.max_age >= ONE_YEAR) {
            return Err(format!(
                "`hsts.preload` needs `include_subdomains` and a `max_age` of at least \
                 {ONE_YEAR}; the preload list refuses anything less. help: set both, or \
                 turn `preload` off"
            ));
        }
        let mut out = format!("max-age={}", self.max_age);
        if self.include_subdomains {
            out.push_str("; includeSubDomains");
        }
        if self.preload {
            out.push_str("; preload");
        }
        Ok(out)
    }
}

/// Who may put this application in a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum Frames {
    /// Nobody. `X-Frame-Options: DENY`, `frame-ancestors 'none'`.
    Deny,
    /// Pages of this origin. `SAMEORIGIN`, `frame-ancestors 'self'`.
    SameOrigin,
    /// These origins. `X-Frame-Options` cannot say a list, so only
    /// `frame-ancestors` is sent, and a browser without CSP Level 2
    /// allows every framer.
    Allow(Vec<String>),
}

impl Frames {
    fn x_frame_options(&self) -> Option<&'static str> {
        match self {
            Frames::Deny => Some("DENY"),
            Frames::SameOrigin => Some("SAMEORIGIN"),
            Frames::Allow(_) => None,
        }
    }

    fn ancestors(&self) -> Vec<String> {
        match self {
            Frames::Deny => vec!["'none'".to_string()],
            Frames::SameOrigin => vec!["'self'".to_string()],
            Frames::Allow(origins) => origins.clone(),
        }
    }
}

/// A Content-Security-Policy.
///
/// The policy is a base that [`Csp::default`] holds, with
/// [`directives`](Csp::directives) laid over it one directive at a
/// time. So a configuration that changes `img-src` keeps every other
/// default:
///
/// ```toml
/// [security_headers.csp.directives]
/// img-src = ["'self'", "data:", "https://images.example.com"]
/// ```
///
/// The base:
///
/// | Directive | Sources |
/// |---|---|
/// | `default-src` | `'self'` |
/// | `script-src` | `'self'` and the hashes of dp-swap's two inline scripts |
/// | `style-src` | `'self' 'unsafe-inline'` |
/// | `img-src` | `'self' data:` |
/// | `font-src`, `connect-src` | `'self'` |
/// | `object-src` | `'none'` |
/// | `base-uri`, `form-action` | `'self'` |
/// | `frame-ancestors` | from [`Frames`] |
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct Csp {
    /// Send a policy at all.
    pub enabled: bool,
    /// Send it as `Content-Security-Policy-Report-Only`: the browser
    /// reports what it would block and blocks nothing. The way to roll
    /// a policy out on a live site.
    pub report_only: bool,
    /// Allow dp-swap's two inline scripts by their hashes.
    pub dp_scripts: bool,
    /// Allow `style=` attributes and `<style>` blocks.
    pub inline_styles: bool,
    /// More `'sha256-…'` sources for `script-src`, for scripts this
    /// application inlines itself. [`Csp::inline_script`] fills it.
    pub script_hashes: Vec<String>,
    /// Directives over the base, by name. An empty list removes the
    /// directive.
    pub directives: BTreeMap<String, Vec<String>>,
    /// `report-uri`: where a browser posts what it blocked.
    pub report_uri: Option<String>,
}

impl Default for Csp {
    fn default() -> Self {
        Self {
            enabled: true,
            report_only: false,
            dp_scripts: true,
            inline_styles: true,
            script_hashes: Vec::new(),
            directives: BTreeMap::new(),
            report_uri: None,
        }
    }
}

impl Csp {
    /// No `Content-Security-Policy` at all.
    pub fn off() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Report what the policy would block, block nothing.
    #[must_use]
    pub fn report_only(mut self) -> Self {
        self.report_only = true;
        self
    }

    /// Set one directive, replacing the base's sources for it.
    #[must_use]
    pub fn directive(
        mut self,
        name: impl Into<String>,
        sources: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.directives
            .insert(name.into(), sources.into_iter().map(Into::into).collect());
        self
    }

    /// Remove one directive from the policy.
    #[must_use]
    pub fn without(mut self, name: impl Into<String>) -> Self {
        self.directives.insert(name.into(), Vec::new());
        self
    }

    /// Allow one inline script, by the hash of its exact text.
    ///
    /// The text is what sits between `<script>` and `</script>`, byte for
    /// byte. A space the template adds is a different hash.
    #[must_use]
    pub fn inline_script(mut self, text: &str) -> Self {
        self.script_hashes.push(script_hash(text));
        self
    }

    /// No `'unsafe-inline'` in `style-src`.
    #[must_use]
    pub fn strict_styles(mut self) -> Self {
        self.inline_styles = false;
        self
    }

    /// Where a browser reports what it blocked.
    #[must_use]
    pub fn report_uri(mut self, uri: impl Into<String>) -> Self {
        self.report_uri = Some(uri.into());
        self
    }

    /// The header value.
    ///
    /// # Errors
    ///
    /// A directive name or a source that cannot be written in the header.
    pub fn render(&self, frames: &Frames) -> Result<String, String> {
        let mut script = vec!["'self'".to_string()];
        if self.dp_scripts {
            script.push(script_hash(crate::dp::SCRIPT));
            script.push(script_hash(crate::dp::PREFETCH));
        }
        script.extend(self.script_hashes.iter().cloned());
        let mut style = vec!["'self'".to_string()];
        if self.inline_styles {
            style.push("'unsafe-inline'".to_string());
        }
        let own = |s: &str| vec![s.to_string()];
        let mut policy: Vec<(String, Vec<String>)> = vec![
            ("default-src".into(), own("'self'")),
            ("script-src".into(), script),
            ("style-src".into(), style),
            ("img-src".into(), vec!["'self'".into(), "data:".into()]),
            ("font-src".into(), own("'self'")),
            ("connect-src".into(), own("'self'")),
            ("object-src".into(), own("'none'")),
            ("base-uri".into(), own("'self'")),
            ("form-action".into(), own("'self'")),
            ("frame-ancestors".into(), frames.ancestors()),
        ];
        for (name, sources) in &self.directives {
            if name == "frame-ancestors" {
                return Err(
                    "`csp.directives` sets `frame-ancestors`, and `frames` owns it. \
                            help: set `frames` instead, so `X-Frame-Options` agrees"
                        .to_string(),
                );
            }
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
                return Err(format!(
                    "`csp.directives` has `{name}`, which is not a directive name. \
                     help: use lowercase letters and `-`, as in `img-src`"
                ));
            }
            match policy.iter_mut().find(|(n, _)| n == name) {
                Some(slot) => slot.1 = sources.clone(),
                None => policy.push((name.clone(), sources.clone())),
            }
        }
        if let Some(uri) = &self.report_uri {
            policy.push(("report-uri".into(), vec![uri.clone()]));
        }

        let mut out = Vec::new();
        for (name, sources) in policy {
            if sources.is_empty() {
                // An empty list removes it. `upgrade-insecure-requests`
                // takes no sources, so it is written with one empty one.
                continue;
            }
            for source in &sources {
                if source.contains([';', ',']) || source.chars().any(char::is_whitespace) {
                    return Err(format!(
                        "the CSP source `{source}` in `{name}` holds a space, `;` or `,`, \
                         which would start another source or directive. help: write one \
                         source per list entry"
                    ));
                }
            }
            let sources: Vec<&str> = sources
                .iter()
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .collect();
            if sources.is_empty() {
                out.push(name);
            } else {
                out.push(format!("{name} {}", sources.join(" ")));
            }
        }
        Ok(out.join("; "))
    }
}

/// The CSP source that allows one inline script: `'sha256-<base64>'` of
/// its exact text.
pub fn script_hash(text: &str) -> String {
    let digest = sha2::Sha256::digest(text.as_bytes());
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Sets the security headers on every response.
pub struct SecurityHeaders {
    config: SecurityHeadersConfig,
}

impl Plugin for SecurityHeaders {
    type Config = SecurityHeadersConfig;
    type Provides = NoContext;
    /// Also the configuration section, so it is a word an environment
    /// variable can spell: `MY_APP_SECURITY_HEADERS__BEHIND_TLS=true`.
    const NAME: &'static str = "security_headers";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for SecurityHeaders {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let built = self
            .config
            .headers()
            .map_err(|why| PluginError::new(format!("security_headers: {why}")))?;
        registrar.response("set", brocanth::plugin_order::Order::new(), built);
        Ok(())
    }
}

/// The header values, built at startup.
struct Built {
    always: Vec<(HeaderName, HeaderValue)>,
    hsts: Option<HeaderValue>,
    behind_tls: bool,
}

impl Built {
    fn over_tls(&self, request: &Request, state: &RequestState) -> bool {
        self.behind_tls
            || request.uri().scheme() == Some(&http::uri::Scheme::HTTPS)
            || state
                .get::<ClientInfo>()
                .and_then(|info| info.scheme.as_deref())
                == Some("https")
    }
}

impl ResponseHook for Built {
    /// Headers only, so a stream gets them too.
    fn on_headers(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        for (name, value) in &self.always {
            head.headers.entry(name).or_insert_with(|| value.clone());
        }
        if let Some(hsts) = &self.hsts {
            if self.over_tls(request, state) {
                head.headers
                    .entry(http::header::STRICT_TRANSPORT_SECURITY)
                    .or_insert_with(|| hsts.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth::plugin_body::Body;

    fn request(uri: &str) -> Request {
        http::Request::builder()
            .uri(uri)
            .body(Vec::new())
            .expect("a request")
    }

    fn run(
        config: SecurityHeadersConfig,
        request: &Request,
        state: &mut RequestState,
    ) -> http::HeaderMap {
        let hook = config.headers().expect("a valid configuration");
        let (mut head, _) = http::Response::new(Body::from(Vec::new())).into_parts();
        hook.on_headers(request, &mut head, BodyShape::Buffered { len: 0 }, state)
            .expect("the hook answers");
        head.headers
    }

    fn get<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn the_defaults_send_every_header_but_hsts_over_plain_http() {
        let headers = run(
            SecurityHeadersConfig::default(),
            &request("/"),
            &mut RequestState::new(),
        );
        assert_eq!(get(&headers, "x-content-type-options"), Some("nosniff"));
        assert_eq!(
            get(&headers, "referrer-policy"),
            Some("strict-origin-when-cross-origin")
        );
        assert_eq!(get(&headers, "x-frame-options"), Some("DENY"));
        assert!(get(&headers, "permissions-policy").is_some());
        let csp = get(&headers, "content-security-policy").expect("a policy");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(csp.contains("object-src 'none'"), "{csp}");
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"), "{csp}");
        assert!(
            get(&headers, "strict-transport-security").is_none(),
            "HSTS over plain HTTP pins localhost for a year"
        );
    }

    #[test]
    fn hsts_goes_out_when_the_request_is_known_to_be_https() {
        let over_https = run(
            SecurityHeadersConfig::default(),
            &request("https://example.com/"),
            &mut RequestState::new(),
        );
        assert_eq!(
            get(&over_https, "strict-transport-security"),
            Some("max-age=31536000")
        );

        let mut forwarded = RequestState::new();
        forwarded.insert(ClientInfo {
            address: None,
            scheme: Some("https".into()),
            host: None,
            chain: Vec::new(),
        });
        let behind_proxy = run(
            SecurityHeadersConfig::default(),
            &request("/"),
            &mut forwarded,
        );
        assert!(get(&behind_proxy, "strict-transport-security").is_some());

        let told = run(
            SecurityHeadersConfig::default().behind_tls(),
            &request("/"),
            &mut RequestState::new(),
        );
        assert!(get(&told, "strict-transport-security").is_some());

        let off = run(
            SecurityHeadersConfig::default().behind_tls().without_hsts(),
            &request("/"),
            &mut RequestState::new(),
        );
        assert!(get(&off, "strict-transport-security").is_none());
    }

    #[test]
    fn preload_needs_subdomains_and_a_year() {
        let short = SecurityHeadersConfig::default()
            .hsts(Hsts::max_age(600).include_subdomains().preload());
        assert!(short.check().unwrap_err().contains("help:"));
        let narrow = SecurityHeadersConfig::default().hsts(Hsts::max_age(ONE_YEAR * 2).preload());
        assert!(narrow.check().is_err());

        let ok = SecurityHeadersConfig::default()
            .behind_tls()
            .hsts(Hsts::max_age(ONE_YEAR * 2).include_subdomains().preload());
        let headers = run(ok, &request("/"), &mut RequestState::new());
        assert_eq!(
            get(&headers, "strict-transport-security"),
            Some("max-age=63072000; includeSubDomains; preload")
        );
    }

    #[test]
    fn a_header_the_handler_set_is_kept() {
        let hook = SecurityHeadersConfig::default().headers().unwrap();
        let mut response = http::Response::new(Body::from(Vec::new()));
        response
            .headers_mut()
            .insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
        let (mut head, _) = response.into_parts();
        hook.on_headers(
            &request("/"),
            &mut head,
            BodyShape::Buffered { len: 0 },
            &mut RequestState::new(),
        )
        .unwrap();
        assert_eq!(get(&head.headers, "x-frame-options"), Some("SAMEORIGIN"));
    }

    #[test]
    fn the_policy_allows_the_dp_scripts_by_hash_and_nothing_inline() {
        let csp = Csp::default().render(&Frames::Deny).unwrap();
        let script = csp
            .split("; ")
            .find(|d| d.starts_with("script-src "))
            .expect("a script-src");
        assert!(!script.contains("'unsafe-inline'"), "{script}");
        assert!(script.contains(&script_hash(crate::dp::SCRIPT)), "{script}");
        assert!(
            script.contains(&script_hash(crate::dp::PREFETCH)),
            "{script}"
        );
    }

    #[test]
    fn the_hash_is_the_one_a_browser_computes() {
        // `printf 'alert(1)' | openssl dgst -sha256 -binary | base64`
        assert_eq!(
            script_hash("alert(1)"),
            "'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='"
        );
    }

    #[test]
    fn a_directive_overrides_one_entry_and_keeps_the_rest() {
        let csp = Csp::default()
            .directive("img-src", ["'self'", "https://images.example.com"])
            .without("font-src")
            .directive("upgrade-insecure-requests", [""])
            .inline_script("boot()")
            .strict_styles()
            .report_uri("/csp-reports")
            .render(&Frames::Allow(vec!["https://partner.example".into()]))
            .unwrap();
        assert!(
            csp.contains("img-src 'self' https://images.example.com"),
            "{csp}"
        );
        assert!(!csp.contains("font-src"), "{csp}");
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(csp.contains("; upgrade-insecure-requests"), "{csp}");
        assert!(csp.contains(&script_hash("boot()")), "{csp}");
        assert!(csp.contains("style-src 'self';"), "{csp}");
        assert!(
            csp.contains("frame-ancestors https://partner.example"),
            "{csp}"
        );
        assert!(csp.ends_with("report-uri /csp-reports"), "{csp}");
    }

    #[test]
    fn frames_drive_both_headers() {
        let same = run(
            SecurityHeadersConfig::default().frames(Frames::SameOrigin),
            &request("/"),
            &mut RequestState::new(),
        );
        assert_eq!(get(&same, "x-frame-options"), Some("SAMEORIGIN"));
        assert!(get(&same, "content-security-policy")
            .unwrap()
            .contains("frame-ancestors 'self'"));

        let listed = run(
            SecurityHeadersConfig::default()
                .frames(Frames::Allow(vec!["https://a.example".into()])),
            &request("/"),
            &mut RequestState::new(),
        );
        assert!(
            get(&listed, "x-frame-options").is_none(),
            "XFO cannot say a list"
        );
    }

    #[test]
    fn report_only_changes_the_header_name() {
        let headers = run(
            SecurityHeadersConfig::default().csp(Csp::default().report_only()),
            &request("/"),
            &mut RequestState::new(),
        );
        assert!(get(&headers, "content-security-policy").is_none());
        assert!(get(&headers, "content-security-policy-report-only").is_some());
    }

    #[test]
    fn a_policy_that_cannot_be_written_is_a_startup_error() {
        let bad = [
            SecurityHeadersConfig::default().referrer_policy("strict-orign"),
            SecurityHeadersConfig::default().csp(Csp::default().directive("img-src", ["a; b"])),
            SecurityHeadersConfig::default()
                .csp(Csp::default().directive("frame-ancestors", ["*"])),
            SecurityHeadersConfig::default().csp(Csp::default().directive("Img Src", ["'self'"])),
            SecurityHeadersConfig::default().permissions_policy("camera=()\n"),
        ];
        for config in bad {
            let why = config.check().expect_err("refused");
            assert!(why.contains("help:"), "{why}");
        }
        assert!(SecurityHeadersConfig::default().check().is_ok());
        assert!(SecurityHeadersConfig::default()
            .referrer_policy("no-referrer, strict-origin-when-cross-origin")
            .check()
            .is_ok());
    }
}
