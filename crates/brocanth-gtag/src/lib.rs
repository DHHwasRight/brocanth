#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! # Design notes
//!
//! The tag reaches the page through a response hook, not through a
//! template. A hook sees every HTML page whatever renders it -- `rsx`,
//! `jt`, a handler that writes a string -- and the static export runs
//! every page through the same hooks, so one mechanism covers a server and
//! a directory of files. The generator has no slot for "a line in every
//! layout's `<head>`", and this module does not need one.
//!
//! The script is a constant. Everything that changes per site -- the
//! measurement id, the allow-list, the consent lifetime -- goes in a JSON
//! data block beside it, which a browser does not run. So the script's
//! `'sha256-…'` hash is the same for every application, and the security
//! headers allow it without `'unsafe-inline'`.
//!
//! See `docs/gtag.md` for the compliance reasoning and its sources.

// The plugin and its hooks: the part an application links. The `scaffold`
// feature alone, which the CLI asks for, compiles none of it, so the CLI
// does not link the core runtime to write a project.
#[cfg(feature = "runtime")]
mod plugin;
#[cfg(feature = "runtime")]
pub use plugin::Gtag;

#[cfg(feature = "scaffold")]
pub mod scaffold;

/// The consent script, byte for byte as it is inlined.
///
/// Allow it in a Content-Security-Policy by its hash:
/// `SecurityHeadersConfig::page_script(SCRIPT, CSP)` in
/// `brocanth-middleware`.
pub const SCRIPT: &str = include_str!("consent.js");

/// The origins Google's tag needs once the visitor accepts, per directive.
///
/// From Google's CSP guide for Gtag without the advertising
/// features, plus `*.analytics.google.com`, where GA4 sends its hits.
/// `img-src` and `connect-src` are where the hits go; `script-src` is
/// where `gtag.js` comes from.
pub const CSP: &[(&str, &[&str])] = &[
    ("script-src", &["https://www.googletagmanager.com"]),
    (
        "img-src",
        &[
            "https://www.googletagmanager.com",
            "https://*.google-analytics.com",
        ],
    ),
    (
        "connect-src",
        &[
            "https://www.googletagmanager.com",
            "https://*.google-analytics.com",
            "https://*.analytics.google.com",
        ],
    ),
];

/// The path the consent script reports a choice to when
/// [`GtagConfig::consent_log`] is set. A GET, so a CSRF check, which
/// leaves reads alone, does not refuse it.
pub const CONSENT_PATH: &str = "/__consent";

/// How the tag waits for consent.
pub const CONSENT_MODES: &[&str] = &["basic", "advanced"];

/// What the banner says. Every string is the site's to change.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct Banner {
    /// The sentence that asks.
    pub text: String,
    /// The button that accepts.
    pub accept: String,
    /// The button that refuses. It looks the same as the accept button.
    pub refuse: String,
    /// The small button that opens the banner again, on every page.
    pub manage: String,
    /// A link to the privacy policy, or `None` for no link.
    pub policy_url: Option<String>,
    /// The text of that link.
    pub policy: String,
}

impl Default for Banner {
    fn default() -> Self {
        Self {
            text: "We would like to measure visits with Google Analytics. It sets \
                   cookies only if you accept."
                .into(),
            accept: "Accept".into(),
            refuse: "Refuse".into(),
            manage: "Cookie settings".into(),
            policy_url: None,
            policy: "Privacy policy".into(),
        }
    }
}

/// The catalog keys the banner reads with the `i18n` feature, beside the
/// field each one replaces. A key the catalog lacks keeps the setting.
pub const BANNER_KEYS: [&str; 5] = [
    "gtag.banner.text",
    "gtag.banner.accept",
    "gtag.banner.refuse",
    "gtag.banner.manage",
    "gtag.banner.policy",
];

impl Banner {
    /// A short fingerprint of what the banner says, recorded with each
    /// choice, so a record names the text the visitor answered.
    pub fn version(&self) -> String {
        // FNV-1a over the fields, each followed by a separator byte.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for field in [
            self.text.as_str(),
            self.accept.as_str(),
            self.refuse.as_str(),
            self.policy_url.as_deref().unwrap_or(""),
        ] {
            for byte in field.bytes().chain([0xff]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        format!("{:08x}", hash >> 32)
    }

    /// This banner with each string a catalog has for `text(key)`. The
    /// keys are [`BANNER_KEYS`]. The link's address is a setting only: a
    /// translator changes words, not where a link goes.
    pub fn translated(&self, text: impl Fn(&str) -> Option<String>) -> Banner {
        let [t, a, r, m, p] = BANNER_KEYS.map(text);
        Banner {
            text: t.unwrap_or_else(|| self.text.clone()),
            accept: a.unwrap_or_else(|| self.accept.clone()),
            refuse: r.unwrap_or_else(|| self.refuse.clone()),
            manage: m.unwrap_or_else(|| self.manage.clone()),
            policy_url: self.policy_url.clone(),
            policy: p.unwrap_or_else(|| self.policy.clone()),
        }
    }
}

/// Where the tag goes and what it may send.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct GtagConfig {
    /// Insert the tag and the banner at all. Off by default, so a
    /// developer's machine and a staging server send nothing: a
    /// production deployment turns it on.
    pub enabled: bool,
    /// The GA4 measurement id, `G-` and letters and digits.
    pub measurement_id: String,
    /// `basic`: no Google script before the visitor accepts. `advanced`:
    /// the script loads with every consent denied and sends cookieless
    /// pings. Basic is the default because it sends nothing to Google
    /// without consent.
    pub consent_mode: String,
    /// Treat Global Privacy Control and Do Not Track as a refusal: no
    /// banner, no tag.
    pub respect_gpc: bool,
    /// How long a choice is kept before the banner asks again, in days.
    /// The CNIL recommends six months.
    pub consent_days: u32,
    /// Send `document.title`. Off: the page's scrubbed path is the title,
    /// because a title can hold a name.
    pub send_title: bool,
    /// The query parameters that reach Google. Every other one is
    /// removed before a page view leaves, and so is a parameter or a path
    /// segment that holds an `@`.
    pub allow_params: Vec<String>,
    /// Paths that never get the tag, each with everything under it.
    pub exclude: Vec<String>,
    /// The domain Google's cookies are set on and removed from, such as
    /// `example.co.uk`. `None`: the tag picks it, and withdrawal removes
    /// the `_ga` cookies from the host and every parent domain, which a
    /// browser ignores for a public suffix.
    pub cookie_domain: Option<String>,
    /// A file each consent choice is appended to, one JSON line per choice:
    /// the time, the choice, a random id the browser keeps, and the
    /// version of the banner text. No address and no user agent. `None`
    /// (the default): nothing is recorded and the script reports nothing.
    ///
    /// It needs a server. A static export has no one to report to, so a
    /// static site keeps the choice in the browser only.
    pub consent_log: Option<String>,
    /// What the banner says.
    pub banner: Banner,
    /// With the `i18n` feature, the banner's words come from the request's
    /// locale. False keeps the `banner` text in every locale.
    ///
    /// Not a setting: it is the same in every deployment, so a project
    /// sets it in `src/modules/gtag.rs` (`integrate.i18n`), and the file
    /// refuses it.
    #[cfg_attr(feature = "config", serde(skip))]
    pub translate: bool,
}

impl Default for GtagConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            measurement_id: String::new(),
            consent_mode: "basic".into(),
            respect_gpc: true,
            consent_days: 182,
            send_title: false,
            allow_params: [
                "utm_source",
                "utm_medium",
                "utm_campaign",
                "utm_term",
                "utm_content",
            ]
            .map(String::from)
            .to_vec(),
            exclude: vec!["/admin".into()],
            cookie_domain: None,
            consent_log: None,
            banner: Banner::default(),
            translate: true,
        }
    }
}

impl GtagConfig {
    /// On, with this measurement id.
    #[must_use]
    pub fn measuring(mut self, id: impl Into<String>) -> Self {
        self.enabled = true;
        self.measurement_id = id.into();
        self
    }

    /// Whether the banner follows the request's locale, with the `i18n`
    /// feature. See [`GtagConfig::translate`](GtagConfig#structfield.translate).
    #[must_use]
    pub fn translate(mut self, on: bool) -> Self {
        self.translate = on;
        self
    }

    /// Whether this configuration can run.
    ///
    /// # Errors
    ///
    /// A missing or malformed measurement id while enabled, an unknown
    /// consent mode, a zero lifetime, a policy link that is not a path or
    /// an `http(s)` URL.
    pub fn check(&self) -> Result<(), String> {
        if self.enabled {
            let id = self.measurement_id.as_str();
            let well_formed = id.len() > 2
                && id.starts_with("G-")
                && id[2..]
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
            if !well_formed {
                return Err(format!(
                    "`measurement_id` is {id:?}, and a GA4 id is `G-` then capitals and \
                     digits. help: copy it from Admin > Data streams in Google Analytics"
                ));
            }
        }
        if !CONSENT_MODES.contains(&self.consent_mode.as_str()) {
            return Err(format!(
                "`consent_mode` is {:?}. help: use `basic` or `advanced`",
                self.consent_mode
            ));
        }
        if self.consent_days == 0 {
            return Err("`consent_days` is 0, so the banner asks on every page. \
                        help: use 182, the six months the CNIL recommends"
                .into());
        }
        if let Some(domain) = &self.cookie_domain {
            let valid = !domain.is_empty()
                && domain
                    .trim_start_matches('.')
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
            if !valid {
                return Err(format!(
                    "`cookie_domain` is {domain:?}. help: use a bare domain such as \
                     `example.co.uk`, with no scheme and no path"
                ));
            }
        }
        if self.consent_log.as_deref() == Some("") {
            return Err("`consent_log` is empty. help: name a file such as \
                        `log/consent.jsonl`, or leave it out to record nothing"
                .into());
        }
        if let Some(url) = &self.banner.policy_url {
            let safe =
                url.starts_with('/') || url.starts_with("https://") || url.starts_with("http://");
            if !safe {
                return Err(format!(
                    "`banner.policy_url` is {url:?}. help: use a path such as \
                     `/privacy` or an `https://` URL"
                ));
            }
        }
        Ok(())
    }

    /// Whether a page at this path gets the tag.
    pub fn tracks(&self, path: &str) -> bool {
        !self.exclude.iter().any(|entry| {
            let entry = entry.trim_end_matches('/');
            entry.is_empty()
                || path == entry
                || path
                    .strip_prefix(entry)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// The markup the hook inserts before `</body>`: the banner, the
    /// settings as JSON, and [`SCRIPT`].
    pub fn markup(&self) -> String {
        self.markup_with(&self.banner, None)
    }

    /// The markup with another banner, in the language `lang` names. The
    /// hook calls it with the banner [`Banner::translated`] gives for the
    /// request's locale.
    pub fn markup_with(&self, b: &Banner, lang: Option<&str>) -> String {
        let lang = lang
            .map(|tag| format!(r#" lang="{}""#, escape(tag)))
            .unwrap_or_default();
        let button = "font:inherit;padding:.5em 1em;border:1px solid currentColor;\
                      border-radius:.25em;background:Canvas;color:CanvasText;cursor:pointer";
        let policy = b
            .policy_url
            .as_deref()
            .map(|url| format!(r#" <a href="{}">{}</a>"#, escape(url), escape(&b.policy)))
            .unwrap_or_default();
        format!(
            concat!(
                r#"<div id="brocanth-consent"{lang} role="dialog" aria-modal="false" "#,
                r#"aria-labelledby="brocanth-consent-text" hidden "#,
                r#"style="position:fixed;inset:auto 1em 1em 1em;z-index:2147483647;"#,
                r#"padding:1em;border:1px solid currentColor;border-radius:.5em;"#,
                r#"background:Canvas;color:CanvasText;max-width:40em">"#,
                r#"<p id="brocanth-consent-text" style="margin:0 0 .75em">{text}{policy}</p>"#,
                r#"<button type="button" data-consent="refuse" style="{button}">{refuse}</button> "#,
                r#"<button type="button" data-consent="accept" style="{button}">{accept}</button>"#,
                r#"</div>"#,
                r#"<button type="button" data-consent-manage "#,
                r#"style="position:fixed;left:.5em;bottom:.5em;font-size:.75em;{button}">"#,
                r#"{manage}</button>"#,
                r#"<script type="application/json" id="brocanth-gtag">{json}</script>"#,
                r#"<script>{script}</script>"#,
            ),
            lang = lang,
            text = escape(&b.text),
            policy = policy,
            button = button,
            refuse = escape(&b.refuse),
            accept = escape(&b.accept),
            manage = escape(&b.manage),
            json = self.json(b),
            script = SCRIPT,
        )
    }

    /// A short fingerprint of what the banner says, recorded with each
    /// choice, so a record names the text the visitor answered.
    pub fn banner_version(&self) -> String {
        self.banner.version()
    }

    /// What the script reads, as JSON safe inside `<script>`.
    fn json(&self, banner: &Banner) -> String {
        let allow: Vec<String> = self.allow_params.iter().map(|p| json_string(p)).collect();
        let domain = self
            .cookie_domain
            .as_deref()
            .map_or_else(|| "null".to_string(), json_string);
        format!(
            concat!(
                r#"{{"id":{},"mode":{},"gpc":{},"days":{},"title":{},"allow":[{}],"#,
                r#""domain":{},"log":{},"version":{}}}"#
            ),
            json_string(&self.measurement_id),
            json_string(&self.consent_mode),
            self.respect_gpc,
            self.consent_days,
            self.send_title,
            allow.join(","),
            domain,
            self.consent_log.is_some(),
            json_string(&banner.version()),
        )
    }
}

/// A page URL with what may identify a person taken out: the fragment,
/// every query parameter not in `allow`, and any parameter or path segment
/// that holds an `@` (raw or `%40`), which is how an email address sits in
/// a URL. The consent script does the same in the browser; this is its
/// specification and its test.
pub fn scrub(href: &str, allow: &[String]) -> String {
    let pii = |s: &str| s.contains('@') || s.to_ascii_lowercase().contains("%40");
    let href = href.split('#').next().unwrap_or_default();
    let (base, query) = href.split_once('?').unwrap_or((href, ""));
    let cut = base.find("://").map_or(Some(0), |scheme| {
        base[scheme + 3..].find('/').map(|at| scheme + 3 + at)
    });
    let (origin, path) = match cut {
        Some(at) => base.split_at(at),
        None => (base, ""),
    };
    let path: Vec<&str> = path
        .split('/')
        .map(|segment| if pii(segment) { "redacted" } else { segment })
        .collect();
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            !pair.is_empty()
                && allow
                    .iter()
                    .any(|name| name == pair.split('=').next().unwrap_or_default())
                && !pii(pair)
        })
        .collect();
    let mut out = format!("{origin}{}", path.join("/"));
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    out
}

/// One choice as the browser reported it, checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Choice {
    /// `granted` or `denied`.
    pub choice: &'static str,
    /// The random id the browser keeps beside its choice.
    pub id: String,
    /// The banner version the browser saw.
    pub version: String,
}

/// Read `c=granted&id=…&v=…`. `None` for anything else: an unknown
/// choice, an id that is not 8 to 64 letters, digits and hyphens, a
/// version that is not 1 to 16 hexadecimal digits.
pub fn parse_choice(query: &str) -> Option<Choice> {
    let mut choice = None;
    let mut id = None;
    let mut version = None;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        match name {
            "c" => {
                choice = Some(match value {
                    "granted" => "granted",
                    "denied" => "denied",
                    _ => return None,
                });
            }
            "id" if (8..=64).contains(&value.len())
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
            {
                id = Some(value.to_string());
            }
            "v" if (1..=16).contains(&value.len())
                && value.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                version = Some(value.to_string());
            }
            _ => return None,
        }
    }
    Some(Choice {
        choice: choice?,
        id: id?,
        version: version?,
    })
}

/// Text for an HTML element or a quoted attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A JSON string that cannot close the `<script>` it sits in.
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> Vec<String> {
        GtagConfig::default().allow_params
    }

    #[test]
    fn scrubbing_keeps_the_allowed_parameters_and_drops_the_rest() {
        assert_eq!(
            scrub(
                "https://example.com/shop?utm_source=news&token=abc&utm_medium=mail#top",
                &allow()
            ),
            "https://example.com/shop?utm_source=news&utm_medium=mail"
        );
        assert_eq!(
            scrub("https://example.com/?reset=1", &allow()),
            "https://example.com/"
        );
    }

    #[test]
    fn an_email_in_a_path_or_an_allowed_parameter_never_survives() {
        assert_eq!(
            scrub(
                "https://example.com/users/ann@example.com/edit?utm_source=ann%40example.com",
                &allow()
            ),
            "https://example.com/users/redacted/edit"
        );
    }

    #[test]
    fn a_url_with_no_path_keeps_its_origin() {
        assert_eq!(
            scrub("https://example.com", &allow()),
            "https://example.com"
        );
    }

    #[test]
    fn exclusions_cover_the_path_and_everything_under_it_only() {
        let config = GtagConfig::default();
        assert!(!config.tracks("/admin"));
        assert!(!config.tracks("/admin/users/3"));
        assert!(config.tracks("/administrator"));
        assert!(config.tracks("/"));
    }

    #[test]
    fn the_checks_refuse_what_cannot_run() {
        assert!(GtagConfig::default().check().is_ok(), "off needs no id");
        let on = GtagConfig::default().measuring("G-ABC123");
        assert!(on.check().is_ok());
        let bad = GtagConfig::default().measuring("UA-1234-1");
        assert!(bad.check().unwrap_err().contains("help:"));
        let mode = GtagConfig {
            consent_mode: "always".into(),
            ..GtagConfig::default()
        };
        assert!(mode.check().is_err());
        let mut link = GtagConfig::default();
        link.banner.policy_url = Some("javascript:alert(1)".into());
        assert!(link.check().is_err());
    }

    #[test]
    fn the_settings_cannot_close_the_script_they_sit_in() {
        let mut config = GtagConfig::default().measuring("G-ABC123");
        config.allow_params = vec!["</script><script>x".into()];
        let markup = config.markup();
        assert_eq!(markup.matches("</script>").count(), 2, "{markup}");
    }

    #[test]
    fn a_translation_replaces_the_words_and_keeps_the_link() {
        let mut config = GtagConfig::default().measuring("G-ABC123");
        config.banner.policy_url = Some("/privacy".into());
        let fr = config.banner.translated(|key| match key {
            "gtag.banner.accept" => Some("Accepter".into()),
            "gtag.banner.policy" => Some("Confidentialité".into()),
            _ => None,
        });
        assert_eq!(fr.accept, "Accepter");
        assert_eq!(
            fr.refuse, "Refuse",
            "a key the catalog lacks keeps the setting"
        );
        assert_ne!(fr.version(), config.banner.version());
        let markup = config.markup_with(&fr, Some("fr"));
        assert!(
            markup.contains(r#"<div id="brocanth-consent" lang="fr""#),
            "{markup}"
        );
        assert!(markup.contains(r#"<a href="/privacy">Confidentialité</a>"#));
        assert!(markup.contains(&format!(r#""version":"{}""#, fr.version())));
    }

    #[test]
    fn the_banner_text_is_escaped() {
        let mut config = GtagConfig::default().measuring("G-ABC123");
        config.banner.text = "<b>hi</b>".into();
        assert!(config.markup().contains("&lt;b&gt;hi&lt;/b&gt;"));
    }
}
