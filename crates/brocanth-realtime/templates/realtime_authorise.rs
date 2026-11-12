//! Who may follow which realtime topic: the one place a topic is
//! authorised, for every transport.
//!
//! `GET /realtime/events` (server-sent events) and `GET /realtime/socket` (a
//! WebSocket) both read the topics with `topics`, read the viewer with
//! `viewer`, and pass each topic to `authorise`. The socket also passes each
//! `subscribe <topic>` the browser sends later. Edit `authorise` here; never
//! filter frames after delivery.
//!
//! A socket outlives the request that opened it, so the viewer is read once,
//! when the connection opens, and kept for its life. A logout does not reach
//! an open connection: the socket loop closes each socket after `MAX_AGE`
//! (`src/plugins/realtime_websocket/mod.rs`) and the browser reconnects with
//! a fresh viewer. Close a user's sockets at logout when that is too late.

use brocanth_realtime::{Gate, Subscription, Topic};

use crate::pages::Ctx;

/// Collections any visitor may follow: every row, every change.
///
/// Empty, so nothing is public until somebody says so here.
pub const PUBLIC: &[&str] = &[];

/// Who is asking: what `authorise` needs, read from the request once.
///
/// Put here what `authorise` decides on: the session's user id, a role.
pub struct Viewer;

/// The viewer of this request.
pub fn viewer(_ctx: &Ctx) -> Viewer {
    Viewer
}

/// Whether `viewer` may follow `topic`, and gated by what.
///
/// A public collection takes `Gate::everyone()`. Rows that a policy decides
/// take the policy: `from_policy` gates `posts/*` with the list rule and
/// `posts/42` with the view rule, and the broker inverts the rule into an
/// index key, so a write is a map lookup and not a check per subscriber.
///
/// ```ignore
/// return Ok(Subscription::from_policy(topic, &PostPolicy, &viewer.user_id));
/// ```
pub fn authorise(_viewer: &Viewer, topic: Topic) -> Result<Subscription, String> {
    if PUBLIC.contains(&topic.collection_name()) {
        return Ok(Subscription::new(topic, Gate::everyone()));
    }
    Err(format!(
        "no rule lets this request follow {topic}\n\
         help: add one to `authorise` in src/plugins/realtime/authorise.rs"
    ))
}

/// Every `topics=` in the query, split at commas, parsed, and each named
/// once: a topic asked for twice is one subscription, not two.
pub fn topics(query: &str) -> Result<Vec<Topic>, String> {
    let mut out: Vec<Topic> = Vec::new();
    for pair in query.split('&') {
        let Some(("topics", value)) = pair.split_once('=') else {
            continue;
        };
        for text in decoded(value)?.split(',').filter(|t| !t.is_empty()) {
            let topic = Topic::parse(text).map_err(|e| format!("topic {text:?}: {e}"))?;
            if !out.contains(&topic) {
                out.push(topic);
            }
        }
    }
    Ok(out)
}

/// The first value of `name` in the query, decoded.
pub fn value(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| decoded(value).ok())
}

/// A query value with its `%XX` and `+` undone.
fn decoded(value: &str) -> Result<String, String> {
    let bad = || {
        format!(
            "the query value {value:?} is not percent-encoded UTF-8\n\
             help: encode the topics with `encodeURIComponent`"
        )
    };
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.bytes();
    while let Some(byte) = rest.next() {
        bytes.push(match byte {
            b'+' => b' ',
            b'%' => {
                let hex = [rest.next().ok_or_else(bad)?, rest.next().ok_or_else(bad)?];
                let hex = std::str::from_utf8(&hex).map_err(|_| bad())?;
                u8::from_str_radix(hex, 16).map_err(|_| bad())?
            }
            other => other,
        });
    }
    String::from_utf8(bytes).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_read_from_the_query_encoded_or_not() {
        let read = topics("topics=posts/*,posts%2F42&x=1&topics=users%2F*").expect("three");
        let names: Vec<String> = read.iter().map(ToString::to_string).collect();
        assert_eq!(names, ["posts/*", "posts/42", "users/*"]);
        assert!(topics("").expect("none is read").is_empty());
        assert!(topics("topics=posts").is_err(), "a topic has a slash");
        assert_eq!(value("a=1&last=7", "last").as_deref(), Some("7"));
    }

    #[test]
    fn a_topic_asked_for_twice_is_read_once() {
        let read = topics("topics=posts/*,posts%2F*&topics=posts/*").expect("one");
        assert_eq!(read.len(), 1);
    }

    #[test]
    fn nothing_is_public_until_listed() {
        let refused =
            authorise(&Viewer, Topic::parse("posts/*").expect("a topic")).expect_err("no rule yet");
        assert!(refused.contains("\nhelp: "), "{refused}");
    }
}
