//! Who the client is, when a proxy is in the way.
//!
//! Gorilla's `ProxyHeaders` rewrites `RemoteAddr`, `URL.Scheme` and `Host`
//! in place from the forwarded headers. This one does not rewrite the
//! request: it puts a [`ClientInfo`] in the request state and leaves the
//! request as it arrived.
//!
//! The difference is deliberate. A rewritten request cannot be told apart
//! from one that really came from that address, so a later hook has no way
//! to ask "was this forwarded, and by whom". Keeping both means an audit
//! line can say so, and a handler that wants the simple answer asks the
//! state for it.
//!
//! # These headers are claims, not facts
//!
//! Anyone who can reach the socket can send `X-Forwarded-For`. Reading one
//! from an untrusted peer is how an address allow-list gets walked past, so
//! [`ProxyHeadersConfig::trust`] is **not** optional and there is no
//! "trust everything" value that is also the default: an application says
//! which peers may speak for somebody else, or it gets nothing.

use std::net::IpAddr;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState,
};

use crate::{first, text};

/// What the forwarded headers said, once a trusted peer said it.
///
/// Put in the request state by [`ProxyHeaders`]. Absent when the peer was
/// not trusted, which is the same shape as "no proxy", because the right
/// answer in both cases is to use the socket's own address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    /// The address the client connected from, as the proxy reports it.
    pub address: Option<IpAddr>,
    /// `http` or `https`, as the proxy reports it.
    pub scheme: Option<String>,
    /// The host the client asked for, as the proxy reports it.
    pub host: Option<String>,
    /// Every address in the chain, nearest client first. The first is
    /// [`address`](ClientInfo::address); the rest are the proxies.
    pub chain: Vec<IpAddr>,
}

/// Which peers may speak for somebody else.
#[derive(Clone, Debug)]
pub struct ProxyHeadersConfig {
    /// The peer addresses whose forwarded headers are read.
    ///
    /// An empty list reads nobody's, which is the safe reading of "this was
    /// configured and nothing was named".
    pub trust: Vec<IpAddr>,
}

impl ProxyHeadersConfig {
    /// Trust these peers and nobody else.
    pub fn trusting(peers: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            trust: peers.into_iter().collect(),
        }
    }
}

/// Reads the forwarded headers from a trusted peer.
pub struct ProxyHeaders {
    config: ProxyHeadersConfig,
}

impl Plugin for ProxyHeaders {
    type Config = ProxyHeadersConfig;
    type Provides = NoContext;
    const NAME: &'static str = "proxy-headers";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for ProxyHeaders {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "read",
            brocanth::plugin_order::Order::new(),
            Hook {
                trust: self.config.trust.clone(),
            },
        );
        Ok(())
    }
}

struct Hook {
    trust: Vec<IpAddr>,
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        // The peer is whoever opened the socket. Without one there is
        // nothing to trust, so there is nothing to read.
        let Some(peer) = request.extensions().get::<PeerAddress>() else {
            return Ok(Flow::Continue);
        };
        if !self.trust.contains(&peer.0) {
            return Ok(Flow::Continue);
        }
        if let Some(info) = read(request.headers()) {
            state.insert(info);
        }
        Ok(Flow::Continue)
    }
}

/// The address the socket came from.
///
/// Put in the request's extensions by whatever accepted the connection.
/// Declared here because this is the only thing that reads it, and a
/// transport that does not set it simply never trusts a proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerAddress(pub IpAddr);

/// Reads both spellings. RFC 7239 `Forwarded` wins where both are present,
/// because it is the one with a specification.
fn read(headers: &http::HeaderMap) -> Option<ClientInfo> {
    let forwarded = text(headers, http::header::FORWARDED).and_then(parse_forwarded);
    let legacy = parse_legacy(headers);
    match (forwarded, legacy) {
        (Some(info), _) => Some(info),
        (None, other) => other,
    }
}

/// `Forwarded: for=192.0.2.1;proto=https;host=example.com, for=198.51.100.7`
fn parse_forwarded(value: &str) -> Option<ClientInfo> {
    let mut chain = Vec::new();
    let mut scheme = None;
    let mut host = None;
    for element in value.split(',') {
        for pair in element.split(';') {
            let (key, raw) = pair.split_once('=')?;
            let raw = raw.trim().trim_matches('"');
            match key.trim().to_ascii_lowercase().as_str() {
                "for" => {
                    if let Some(address) = parse_node(raw) {
                        chain.push(address);
                    }
                }
                // The first element is the one nearest the client, and it
                // is the one whose scheme and host the client saw.
                "proto" if scheme.is_none() => scheme = Some(raw.to_ascii_lowercase()),
                "host" if host.is_none() => host = Some(raw.to_string()),
                _ => {}
            }
        }
    }
    if chain.is_empty() && scheme.is_none() && host.is_none() {
        return None;
    }
    Some(ClientInfo {
        address: chain.first().copied(),
        scheme,
        host,
        chain,
    })
}

/// A `for=` node: an address, optionally bracketed for IPv6, optionally
/// with a port. Obfuscated nodes (`for=_hidden`) and `unknown` parse to
/// nothing, which is what they mean.
fn parse_node(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('[') {
        // `[2001:db8::1]:4711`
        let (inside, _) = rest.split_once(']')?;
        return inside.parse().ok();
    }
    if let Ok(address) = raw.parse::<IpAddr>() {
        return Some(address);
    }
    // `192.0.2.1:4711`. Only for IPv4: a bare IPv6 address has colons of
    // its own and the line above already took it.
    raw.rsplit_once(':').and_then(|(host, _)| host.parse().ok())
}

fn parse_legacy(headers: &http::HeaderMap) -> Option<ClientInfo> {
    let chain: Vec<IpAddr> = text(
        headers,
        http::header::HeaderName::from_static("x-forwarded-for"),
    )
    .map(|value| {
        value
            .split(',')
            .filter_map(|hop| parse_node(hop.trim()))
            .collect()
    })
    .unwrap_or_default();
    let scheme = text(
        headers,
        http::header::HeaderName::from_static("x-forwarded-proto"),
    )
    .map(|value| first(value).to_ascii_lowercase());
    let host = text(
        headers,
        http::header::HeaderName::from_static("x-forwarded-host"),
    )
    .map(|value| first(value).to_string());
    if chain.is_empty() && scheme.is_none() && host.is_none() {
        return None;
    }
    Some(ClientInfo {
        address: chain.first().copied(),
        scheme,
        host,
        chain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> http::HeaderMap {
        let mut map = http::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                http::header::HeaderName::from_static(name),
                http::HeaderValue::from_str(value).expect("a test header"),
            );
        }
        map
    }

    #[test]
    fn the_legacy_spelling_gives_the_client_and_the_chain() {
        let info = read(&headers(&[
            ("x-forwarded-for", "203.0.113.9, 198.51.100.7, 10.0.0.1"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "example.com"),
        ]))
        .expect("something was forwarded");
        assert_eq!(info.address, Some("203.0.113.9".parse().unwrap()));
        assert_eq!(info.chain.len(), 3, "the proxies are kept, not dropped");
        assert_eq!(info.scheme.as_deref(), Some("https"));
        assert_eq!(info.host.as_deref(), Some("example.com"));
    }

    #[test]
    fn rfc_7239_wins_where_both_are_sent() {
        // A proxy that speaks both, and a client that sent a legacy header
        // of its own hoping the legacy one is read.
        let info = read(&headers(&[
            ("forwarded", "for=203.0.113.9;proto=https;host=real.example"),
            ("x-forwarded-for", "127.0.0.1"),
            ("x-forwarded-host", "attacker.example"),
        ]))
        .expect("something was forwarded");
        assert_eq!(info.address, Some("203.0.113.9".parse().unwrap()));
        assert_eq!(info.host.as_deref(), Some("real.example"));
    }

    #[test]
    fn a_port_and_a_bracketed_v6_address_both_parse() {
        assert_eq!(
            parse_node("192.0.2.1:4711"),
            Some("192.0.2.1".parse().unwrap())
        );
        assert_eq!(
            parse_node("[2001:db8::1]:4711"),
            Some("2001:db8::1".parse().unwrap())
        );
        assert_eq!(
            parse_node("2001:db8::1"),
            Some("2001:db8::1".parse().unwrap())
        );
    }

    #[test]
    fn an_obfuscated_node_is_not_an_address() {
        // RFC 7239 §6.3. `_hidden` and `unknown` are legal and name nobody.
        assert_eq!(parse_node("_hidden"), None);
        assert_eq!(parse_node("unknown"), None);
        let info = read(&headers(&[("forwarded", "for=_hidden;proto=https")]))
            .expect("the proto is still a claim");
        assert_eq!(info.address, None);
        assert_eq!(info.scheme.as_deref(), Some("https"));
    }

    #[test]
    fn nothing_forwarded_is_nothing_to_record() {
        assert_eq!(read(&headers(&[("accept", "text/html")])), None);
    }

    #[test]
    fn an_untrusted_peer_is_not_read() {
        // The whole point. A header is a claim, and only a peer on the
        // list may make it.
        let hook = Hook {
            trust: vec!["10.0.0.1".parse().unwrap()],
        };
        let mut state = RequestState::new();
        let mut request = http::Request::builder()
            .header("x-forwarded-for", "203.0.113.9")
            .extension(PeerAddress("198.51.100.200".parse().unwrap()))
            .body(Vec::new())
            .expect("a test request");
        hook.before_dispatch(&mut request, &mut state).expect("ok");
        assert!(
            state.get::<ClientInfo>().is_none(),
            "a stranger spoke for somebody else and was believed"
        );

        let mut request = http::Request::builder()
            .header("x-forwarded-for", "203.0.113.9")
            .extension(PeerAddress("10.0.0.1".parse().unwrap()))
            .body(Vec::new())
            .expect("a test request");
        hook.before_dispatch(&mut request, &mut state).expect("ok");
        assert_eq!(
            state.get::<ClientInfo>().and_then(|i| i.address),
            Some("203.0.113.9".parse().unwrap())
        );
    }

    #[test]
    fn an_empty_trust_list_trusts_nobody() {
        let hook = Hook { trust: Vec::new() };
        let mut state = RequestState::new();
        let mut request = http::Request::builder()
            .header("x-forwarded-for", "203.0.113.9")
            .extension(PeerAddress("10.0.0.1".parse().unwrap()))
            .body(Vec::new())
            .expect("a test request");
        hook.before_dispatch(&mut request, &mut state).expect("ok");
        assert!(state.get::<ClientInfo>().is_none());
    }

    #[test]
    fn the_request_is_left_as_it_arrived() {
        // Gorilla rewrites the request. Keeping both means a later hook
        // can still ask what the socket said.
        let hook = Hook {
            trust: vec!["10.0.0.1".parse().unwrap()],
        };
        let mut state = RequestState::new();
        let mut request = http::Request::builder()
            .uri("http://inside.local/x")
            .header("x-forwarded-host", "example.com")
            .header("x-forwarded-proto", "https")
            .extension(PeerAddress("10.0.0.1".parse().unwrap()))
            .body(Vec::new())
            .expect("a test request");
        hook.before_dispatch(&mut request, &mut state).expect("ok");
        assert_eq!(request.uri().host(), Some("inside.local"));
        assert_eq!(request.uri().scheme_str(), Some("http"));
        assert_eq!(
            state.get::<ClientInfo>().and_then(|i| i.host.clone()),
            Some("example.com".to_string())
        );
    }
}
