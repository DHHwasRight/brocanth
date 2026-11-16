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
//!
//! # The client is the nearest hop nobody trusted
//!
//! A proxy appends: nginx's `$proxy_add_x_forwarded_for` and most load
//! balancers keep what the client sent and add the address they saw. So
//! the leftmost entry is whatever the client wrote. The reader walks the
//! chain from the right, skips each address in
//! [`trust`](ProxyHeadersConfig::trust), and takes the first one left. The
//! scheme and host come from the same hop.
//!
//! The reader also reads one header, the one the edge sets
//! ([`ProxyHeadersConfig::header`]). Most edges pass a client's
//! `Forwarded` through untouched, so "whichever is present" lets the
//! client pick.

use std::net::IpAddr;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState,
};

use crate::text;

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
    /// Every address in the chain, as sent: leftmost first. Entries left
    /// of [`address`](ClientInfo::address) are claims the client made;
    /// entries right of it are trusted proxies.
    pub chain: Vec<IpAddr>,
}

/// Which forwarded header the edge sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForwardedHeader {
    /// `X-Forwarded-For`, with `X-Forwarded-Proto` and `X-Forwarded-Host`.
    /// What nginx, most load balancers and most PaaS edges set.
    #[default]
    XForwardedFor,
    /// RFC 7239 `Forwarded`.
    Forwarded,
}

/// Which peers may speak for somebody else.
#[derive(Clone, Debug)]
pub struct ProxyHeadersConfig {
    /// The peer addresses whose forwarded headers are read.
    ///
    /// An empty list reads nobody's, which is the safe reading of "this was
    /// configured and nothing was named".
    ///
    /// The same list marks the proxies inside the forwarded chain: the
    /// reader skips them from the right to find the client.
    pub trust: Vec<IpAddr>,
    /// The one header the edge sets. The other spelling is ignored, since
    /// a client can send it and the edge passes it through.
    pub header: ForwardedHeader,
}

impl ProxyHeadersConfig {
    /// Trust these peers and nobody else. Reads `X-Forwarded-For`.
    pub fn trusting(peers: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            trust: peers.into_iter().collect(),
            header: ForwardedHeader::default(),
        }
    }

    /// Read this header, and not the other spelling.
    pub fn reading(mut self, header: ForwardedHeader) -> Self {
        self.header = header;
        self
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
            brocanth::plugin_order::Phase::Identify,
            brocanth::plugin_order::Order::new(),
            Hook {
                trust: self.config.trust.clone(),
                header: self.config.header,
            },
        );
        Ok(())
    }
}

struct Hook {
    trust: Vec<IpAddr>,
    header: ForwardedHeader,
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        // The peer is whoever opened the socket. Without one there is
        // nothing to trust, so there is nothing to read.
        let Some(peer) = PeerAddress::of(request) else {
            return Ok(Flow::Continue);
        };
        if !self.trust.contains(&peer) {
            return Ok(Flow::Continue);
        }
        if let Some(info) = read(request.headers(), self.header, &self.trust) {
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
///
/// A plain [`std::net::SocketAddr`] extension counts too: that is what
/// `brocanth-server`'s bridge sets, and that crate does not depend on this
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerAddress(pub IpAddr);

impl PeerAddress {
    /// The peer of this request: a `PeerAddress` extension, or else a
    /// `SocketAddr` one. `None` when the transport set neither.
    pub fn of(request: &Request) -> Option<IpAddr> {
        let extensions = request.extensions();
        extensions
            .get::<PeerAddress>()
            .map(|peer| peer.0)
            .or_else(|| {
                extensions
                    .get::<std::net::SocketAddr>()
                    .map(|peer| peer.ip())
            })
    }
}

/// Reads the one header the edge sets.
fn read(
    headers: &http::HeaderMap,
    header: ForwardedHeader,
    trust: &[IpAddr],
) -> Option<ClientInfo> {
    let hops = match header {
        ForwardedHeader::Forwarded => parse_forwarded(text(headers, http::header::FORWARDED)?),
        ForwardedHeader::XForwardedFor => parse_legacy(headers),
    };
    pick(hops, trust)
}

/// One element of the chain: what one proxy said about the hop before it.
#[derive(Default)]
struct Hop {
    address: Option<IpAddr>,
    scheme: Option<String>,
    host: Option<String>,
}

/// The nearest hop that is not a trusted proxy, walking from the right.
/// When every hop is trusted, the leftmost one.
fn pick(hops: Vec<Hop>, trust: &[IpAddr]) -> Option<ClientInfo> {
    if hops
        .iter()
        .all(|hop| hop.address.is_none() && hop.scheme.is_none() && hop.host.is_none())
    {
        return None;
    }
    let chain: Vec<IpAddr> = hops.iter().filter_map(|hop| hop.address).collect();
    let at = hops
        .iter()
        .rposition(|hop| !hop.address.is_some_and(|address| trust.contains(&address)))
        .unwrap_or(0);
    let hop = hops.into_iter().nth(at)?;
    Some(ClientInfo {
        address: hop.address,
        scheme: hop.scheme,
        host: hop.host,
        chain,
    })
}

/// `Forwarded: for=192.0.2.1;proto=https;host=example.com, for=198.51.100.7`
fn parse_forwarded(value: &str) -> Vec<Hop> {
    let mut hops = Vec::new();
    for element in value.split(',') {
        let mut hop = Hop::default();
        for pair in element.split(';') {
            let Some((key, raw)) = pair.split_once('=') else {
                continue;
            };
            let raw = raw.trim().trim_matches('"');
            match key.trim().to_ascii_lowercase().as_str() {
                "for" => hop.address = parse_node(raw),
                "proto" => hop.scheme = Some(raw.to_ascii_lowercase()),
                "host" => hop.host = Some(raw.to_string()),
                _ => {}
            }
        }
        hops.push(hop);
    }
    hops
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

/// `X-Forwarded-For`, with `X-Forwarded-Proto` and `X-Forwarded-Host`
/// lined up with it from the right: an appending proxy adds one entry to
/// each. A proto or host list shorter than the chain was overwritten, not
/// appended, and its last entry is the nearest proxy's.
fn parse_legacy(headers: &http::HeaderMap) -> Vec<Hop> {
    let list = |name: &'static str| -> Vec<&str> {
        text(headers, http::header::HeaderName::from_static(name))
            .map(|value| value.split(',').map(str::trim).collect())
            .unwrap_or_default()
    };
    let addresses = list("x-forwarded-for");
    let schemes = list("x-forwarded-proto");
    let hosts = list("x-forwarded-host");
    let length = addresses.len().max(1);
    let aligned = |values: &[&str], at: usize| -> Option<String> {
        let value = if values.len() == length {
            values.get(at)
        } else {
            values.last()
        };
        value
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
    };
    (0..length)
        .map(|at| Hop {
            address: addresses.get(at).and_then(|hop| parse_node(hop)),
            scheme: aligned(&schemes, at).map(|scheme| scheme.to_ascii_lowercase()),
            host: aligned(&hosts, at),
        })
        .collect()
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

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("an address")
    }

    fn legacy(pairs: &[(&'static str, &str)], trust: &[IpAddr]) -> Option<ClientInfo> {
        read(&headers(pairs), ForwardedHeader::XForwardedFor, trust)
    }

    fn rfc(pairs: &[(&'static str, &str)], trust: &[IpAddr]) -> Option<ClientInfo> {
        read(&headers(pairs), ForwardedHeader::Forwarded, trust)
    }

    #[test]
    fn the_legacy_spelling_gives_the_client_and_the_chain() {
        let info = legacy(
            &[
                ("x-forwarded-for", "203.0.113.9, 198.51.100.7, 10.0.0.1"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "example.com"),
            ],
            &[ip("198.51.100.7"), ip("10.0.0.1")],
        )
        .expect("something was forwarded");
        assert_eq!(info.address, Some(ip("203.0.113.9")));
        assert_eq!(info.chain.len(), 3, "the proxies are kept, not dropped");
        assert_eq!(info.scheme.as_deref(), Some("https"));
        assert_eq!(info.host.as_deref(), Some("example.com"));
    }

    #[test]
    fn a_client_written_hop_through_an_appending_proxy_is_not_the_client() {
        // The client sent `X-Forwarded-For: 1.2.3.4`; the edge appended the
        // address it saw. The leftmost entry is the client's own claim.
        let info = legacy(
            &[
                ("x-forwarded-for", "1.2.3.4, 203.0.113.9"),
                ("x-forwarded-proto", "http, https"),
            ],
            &[ip("10.0.0.1")],
        )
        .expect("something was forwarded");
        assert_eq!(info.address, Some(ip("203.0.113.9")));
        assert_eq!(
            info.scheme.as_deref(),
            Some("https"),
            "the scheme comes from the hop the edge wrote"
        );

        // Two trusted proxies: the inner one appended the edge's address.
        let info = legacy(
            &[("x-forwarded-for", "1.2.3.4, 203.0.113.9, 10.0.0.2")],
            &[ip("10.0.0.1"), ip("10.0.0.2")],
        )
        .expect("something was forwarded");
        assert_eq!(info.address, Some(ip("203.0.113.9")));
    }

    #[test]
    fn a_chain_of_trusted_proxies_only_gives_the_leftmost() {
        let info = legacy(&[("x-forwarded-for", "10.0.0.2")], &[ip("10.0.0.2")])
            .expect("something was forwarded");
        assert_eq!(info.address, Some(ip("10.0.0.2")));
    }

    #[test]
    fn only_the_configured_header_is_read() {
        // An edge that sets `X-Forwarded-For` passes a client's `Forwarded`
        // through. Reading it would let the client pick its address.
        let pairs = [
            ("forwarded", "for=127.0.0.1;host=attacker.example"),
            ("x-forwarded-for", "203.0.113.9"),
        ];
        let info = legacy(&pairs, &[]).expect("something was forwarded");
        assert_eq!(info.address, Some(ip("203.0.113.9")));
        assert_eq!(info.host, None);

        let info = rfc(
            &[
                (
                    "forwarded",
                    "for=1.2.3.4;host=attacker.example, for=203.0.113.9;proto=https;host=real.example",
                ),
                ("x-forwarded-for", "127.0.0.1"),
            ],
            &[],
        )
        .expect("something was forwarded");
        assert_eq!(info.address, Some(ip("203.0.113.9")));
        assert_eq!(info.host.as_deref(), Some("real.example"));
        assert_eq!(info.scheme.as_deref(), Some("https"));
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
        let info = rfc(&[("forwarded", "for=_hidden;proto=https")], &[])
            .expect("the proto is still a claim");
        assert_eq!(info.address, None);
        assert_eq!(info.scheme.as_deref(), Some("https"));
    }

    #[test]
    fn nothing_forwarded_is_nothing_to_record() {
        assert_eq!(legacy(&[("accept", "text/html")], &[]), None);
        assert_eq!(rfc(&[("accept", "text/html")], &[]), None);
    }

    #[test]
    fn an_untrusted_peer_is_not_read() {
        // The whole point. A header is a claim, and only a peer on the
        // list may make it.
        let hook = Hook {
            trust: vec!["10.0.0.1".parse().unwrap()],
            header: ForwardedHeader::XForwardedFor,
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
        let hook = Hook {
            trust: Vec::new(),
            header: ForwardedHeader::XForwardedFor,
        };
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
            header: ForwardedHeader::XForwardedFor,
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
