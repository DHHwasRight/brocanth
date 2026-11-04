//! What a request is reduced to before anything is kept.
//!
//! Every function here takes a raw value off the request and returns a
//! coarser one. The raw value goes no further: the user agent becomes three
//! words, the referrer becomes an origin, the query loses every parameter
//! that is not on the allow-list.

/// The browser, the system and the kind of device, from a `User-Agent`.
///
/// Families, never versions: a version narrows a visitor down, and the
/// dashboard has no use for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UaClass {
    /// `Chrome`, `Firefox`, `Safari`, `Edge`, `Opera`, `Samsung Internet`,
    /// or `Other`.
    pub browser: &'static str,
    /// `Windows`, `macOS`, `iOS`, `Android`, `Linux`, `ChromeOS`, or
    /// `Other`.
    pub os: &'static str,
    /// `desktop`, `mobile` or `tablet`.
    pub device: &'static str,
}

/// Reduces a `User-Agent` to its [`UaClass`].
///
/// Order matters: Edge and Opera say `Chrome`, and Chrome says `Safari`, so
/// the narrower names are tested first.
pub fn ua_class(ua: &str) -> UaClass {
    let browser = if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        "Edge"
    } else if ua.contains("OPR/") || ua.contains("Opera") {
        "Opera"
    } else if ua.contains("SamsungBrowser/") {
        "Samsung Internet"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") || ua.contains("Chromium/") {
        "Chrome"
    } else if ua.contains("Safari/") && ua.contains("Version/") {
        "Safari"
    } else {
        "Other"
    };
    let os = if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") {
        "iOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Other"
    };
    let tablet = ua.contains("iPad")
        || ua.contains("Tablet")
        || (ua.contains("Android") && !ua.contains("Mobile"));
    let device = if tablet {
        "tablet"
    } else if ua.contains("Mobi") || ua.contains("iPhone") {
        "mobile"
    } else {
        "desktop"
    };
    UaClass {
        browser,
        os,
        device,
    }
}

/// Words that a crawler, a monitor or a script puts in its `User-Agent`.
///
/// A heuristic, and it says so: a bot that sends a browser's string is
/// counted. The list errs towards dropping, because a page view a person
/// never made is a worse error than one that went uncounted.
const BOT_WORDS: &[&str] = &[
    "bot",
    "crawl",
    "spider",
    "slurp",
    "facebookexternalhit",
    "embedly",
    "preview",
    "monitor",
    "pingdom",
    "uptime",
    "lighthouse",
    "headless",
    "phantomjs",
    "curl/",
    "wget/",
    "python-requests",
    "python-urllib",
    "go-http-client",
    "okhttp",
    "java/",
    "libwww",
    "httpclient",
    "axios/",
    "node-fetch",
];

/// Whether a `User-Agent` reads as a robot. An empty one does.
pub fn is_bot(ua: &str) -> bool {
    let ua = ua.trim();
    if ua.is_empty() {
        return true;
    }
    let lower = ua.to_ascii_lowercase();
    BOT_WORDS.iter().any(|word| lower.contains(word))
}

/// The longest path kept. A longer one is cut, so one crafted URL cannot
/// write a megabyte row.
pub const MAX_PATH: usize = 512;

/// The path as stored: no query, no fragment, a segment that looks like an
/// email replaced by `_`, and at most [`MAX_PATH`] bytes.
///
/// An email in a path is the one piece of personal data a URL most often
/// carries (`/unsubscribe/jane@example.com`), so it never reaches a row.
pub fn clean_path(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let mut out = String::with_capacity(path.len().min(MAX_PATH));
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let lower = segment.to_ascii_lowercase();
        if segment.contains('@') || lower.contains("%40") {
            out.push('_');
        } else {
            out.push_str(segment);
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    if out.len() > MAX_PATH {
        let mut cut = MAX_PATH;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// The query as stored: only the parameters on `allow`, in the order the
/// request sent them, and none whose value looks like an email.
///
/// Empty when nothing survives, which is the common case.
pub fn clean_query(query: &str, allow: &[String]) -> String {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = value.to_ascii_lowercase();
            allow.iter().any(|a| a == name) && !value.contains('@') && !value.contains("%40")
        })
        .take(10)
        .map(|pair| pair.chars().take(200).collect::<String>())
        .collect::<Vec<_>>()
        .join("&")
}

/// A `Referer` reduced to its origin, `https://news.example`, or `None`
/// when it is missing, malformed, or this site's own `host`.
///
/// The path of a referrer can be a search query or a private page, so only
/// where the visitor came from is kept, never what they were reading.
pub fn referrer_origin(referer: &str, host: Option<&str>) -> Option<String> {
    let (scheme, rest) = referer.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    // Credentials in a URL are never kept, even as an origin.
    let authority = authority.rsplit('@').next()?.to_ascii_lowercase();
    if authority.is_empty() || authority.len() > 255 {
        return None;
    }
    if host.is_some_and(|h| h.eq_ignore_ascii_case(&authority)) {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// A two-letter country code from an edge header, or `None` for anything
/// else, including the `XX` and `T1` codes some edges send for "unknown"
/// and "Tor".
pub fn country(value: &str) -> Option<String> {
    let value = value.trim();
    let code = value.as_bytes();
    if code.len() != 2 || !code.iter().all(u8::is_ascii_alphabetic) {
        return None;
    }
    let upper = value.to_ascii_uppercase();
    match upper.as_str() {
        "XX" | "ZZ" | "T1" => None,
        _ => Some(upper),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
    const SAFARI_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
    const EDGE_WIN: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0";
    const FIREFOX_ANDROID: &str =
        "Mozilla/5.0 (Android 14; Mobile; rv:131.0) Gecko/131.0 Firefox/131.0";

    #[test]
    fn a_user_agent_becomes_three_families() {
        assert_eq!(
            ua_class(CHROME_MAC),
            UaClass {
                browser: "Chrome",
                os: "macOS",
                device: "desktop"
            }
        );
        assert_eq!(
            ua_class(SAFARI_IPHONE),
            UaClass {
                browser: "Safari",
                os: "iOS",
                device: "mobile"
            }
        );
        assert_eq!(ua_class(EDGE_WIN).browser, "Edge");
        assert_eq!(ua_class(FIREFOX_ANDROID).device, "mobile");
        assert_eq!(ua_class(FIREFOX_ANDROID).os, "Android");
    }

    #[test]
    fn robots_and_scripts_are_recognised() {
        for ua in [
            "Googlebot/2.1 (+http://www.google.com/bot.html)",
            "curl/8.4.0",
            "python-requests/2.31",
            "Mozilla/5.0 (compatible; bingbot/2.0)",
            "",
        ] {
            assert!(is_bot(ua), "{ua:?}");
        }
        assert!(!is_bot(CHROME_MAC));
        assert!(!is_bot(SAFARI_IPHONE));
    }

    #[test]
    fn a_path_loses_its_query_and_any_email() {
        assert_eq!(clean_path("/a/b?x=1#top"), "/a/b");
        assert_eq!(
            clean_path("/unsubscribe/jane@example.com"),
            "/unsubscribe/_"
        );
        assert_eq!(clean_path("/u/jane%40example.com/edit"), "/u/_/edit");
        assert_eq!(clean_path(""), "/");
        assert_eq!(
            clean_path(&format!("/{}", "a".repeat(2000))).len(),
            MAX_PATH
        );
    }

    #[test]
    fn only_allowed_query_parameters_survive() {
        let allow = vec!["utm_source".to_string(), "utm_medium".to_string()];
        assert_eq!(
            clean_query(
                "token=abc&utm_source=news&email=a%40b.c&utm_medium=mail",
                &allow
            ),
            "utm_source=news&utm_medium=mail"
        );
        assert_eq!(clean_query("utm_source=jane@example.com", &allow), "");
        assert_eq!(clean_query("", &allow), "");
    }

    #[test]
    fn a_referrer_keeps_its_origin_and_nothing_else() {
        assert_eq!(
            referrer_origin("https://news.example/item?id=1", Some("site.example")).as_deref(),
            Some("https://news.example")
        );
        assert_eq!(
            referrer_origin("https://user:pw@News.Example/x", None).as_deref(),
            Some("https://news.example")
        );
        assert_eq!(
            referrer_origin("https://site.example/a", Some("site.example")),
            None
        );
        assert_eq!(referrer_origin("android-app://x", None), None);
        assert_eq!(referrer_origin("not a url", None), None);
    }

    #[test]
    fn a_country_is_two_letters_or_nothing() {
        assert_eq!(country("fr").as_deref(), Some("FR"));
        assert_eq!(country("XX"), None);
        assert_eq!(country("T1"), None);
        assert_eq!(country("FRA"), None);
    }
}
