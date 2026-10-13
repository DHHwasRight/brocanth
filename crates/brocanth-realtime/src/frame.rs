//! The payload, and where the bytes stop.
//!
//! § 10 is blunt about the shape:
//!
//! > **The payload should be a fragment, not JSON.** PocketBase pushes records
//! > because it has no renderer. We have one … and the fragment emitter caches
//! > one render per audience class, so the two ideas pay for each other.
//!
//! The renderer is `rsx`. This crate does not depend on it, and will not: an
//! arrow from realtime to a template engine would make every application that
//! broadcasts link one. So the fragment is a **seam** — [`FrameEmitter`] —
//! and what ships by default is [`ChangeLine`], which is twenty lines and
//! names the change instead of rendering it.
//!
//! # The caching §10 asks for
//!
//! The broker calls the emitter **once per audience class per topic**, not
//! once per subscriber, and every subscriber in that class gets the same
//! `Arc`. Six hundred subscribers who are all "the team" cost one render. That
//! is the second half of §10's sentence, and it is why [`Emit`] carries
//! [`AudienceClass`]: an emitter that renders the same bytes for everybody can
//! ignore it, and one that renders per class gets told which class it is
//! rendering without having to work it out.
//!
//! # Why the default emitter is not JSON and not a fragment either
//!
//! PocketBase sends `{"record": {...}, "action": "create"}`, which forces the
//! client to hold a copy of the model and re-render it. We are not doing that,
//! and the reason is §5: dp-swap plus `DP-Partial` means the server renders
//! and the client swaps.
//!
//! But this crate has no renderer and must not acquire one, so the honest
//! default is the **third** thing: the change's identity, in a form a client
//! can act on without parsing anything. [`ChangeLine`] emits
//!
//! ```text
//! action=update
//! collection=posts
//! id=42
//! fields=title,body
//! ```
//!
//! which a dp-swap client turns into a partial navigation. It is the right
//! default precisely because it is obviously a placeholder: the moment an
//! application installs a fragment emitter, every byte of this disappears.
//!
//! # SSE framing is encoding, not transport
//!
//! A [`Frame`] holds the complete `id:` / `event:` / `data:` record, newline
//! split and terminated, ready to be written. Producing those bytes needs no
//! socket and no runtime, which is exactly why this crate can produce them
//! and still stop short of the transport. See the crate documentation for
//! where the seam is and what the core must change to close it.

use std::fmt::Write as _;
use std::sync::Arc;

use brocanth_events::{ChangeKind, RowChange};
use brocanth_policy::Value;

use crate::row::Row;
use crate::topic::Topic;

/// One encoded server-sent-events record, ready to write.
///
/// Cheap to clone: the bytes live behind an `Arc` and one frame is shared by
/// every subscriber in its audience class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    inner: Arc<FrameInner>,
}

#[derive(Debug, PartialEq, Eq)]
struct FrameInner {
    id: u64,
    event: String,
    bytes: Box<[u8]>,
}

impl Frame {
    /// Encode an SSE record: an id, an event name, and a data body.
    ///
    /// The body is split at every newline into its own `data:` line, which is
    /// what the protocol requires and what a naive encoder gets wrong the
    /// first time somebody sends an HTML fragment. A `\r` is dropped rather
    /// than written, because a lone `\r` is also a record separator in SSE and
    /// would otherwise end the frame early.
    pub fn sse(id: u64, topic: &Topic, data: impl AsRef<str>) -> Frame {
        let event = topic.to_string();
        let data = data.as_ref();
        let mut text = String::with_capacity(data.len() + event.len() + 32);
        let _ = writeln!(text, "id:{id}");
        let _ = writeln!(text, "event:{event}");
        for line in data.split('\n') {
            text.push_str("data:");
            for ch in line.chars() {
                if ch != '\r' {
                    text.push(ch);
                }
            }
            text.push('\n');
        }
        text.push('\n');
        Frame {
            inner: Arc::new(FrameInner {
                id,
                event,
                bytes: text.into_bytes().into_boxed_slice(),
            }),
        }
    }

    /// Wrap bytes that are already a complete SSE record.
    ///
    /// For an emitter that builds its own framing — a comment line, a
    /// multi-event record, a keepalive. Nothing checks the bytes, so an
    /// emitter that uses this owns the protocol.
    pub fn raw(id: u64, event: impl Into<String>, bytes: impl Into<Box<[u8]>>) -> Frame {
        Frame {
            inner: Arc::new(FrameInner {
                id,
                event: event.into(),
                bytes: bytes.into(),
            }),
        }
    }

    /// The bytes, as they go on the wire.
    pub fn bytes(&self) -> &[u8] {
        &self.inner.bytes
    }

    /// The SSE `id:`. Monotonic per broker, so a client can resume.
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// The SSE `event:` name, which is the topic the frame was addressed to.
    /// A client demultiplexes on it, as PocketBase's does.
    pub fn event(&self) -> &str {
        &self.inner.event
    }

    /// How many bytes this frame occupies.
    pub fn len(&self) -> usize {
        self.inner.bytes.len()
    }

    /// `true` when the frame carries nothing.
    pub fn is_empty(&self) -> bool {
        self.inner.bytes.is_empty()
    }
}

/// Which audience a frame is being rendered for.
///
/// §10's "one render per audience class". Borrowed, because it is handed to
/// the emitter and thrown away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudienceClass<'a> {
    /// A policy that admits every row. One render for all of them.
    Everyone,
    /// A policy inverted to this key. Every subscriber in the class asked the
    /// same question of the same column and got the same answer.
    Keyed {
        /// The column the audience keys on.
        column: &'a str,
        /// The value it keys on, which is this row's value for that column.
        value: &'a Value,
    },
    /// A policy that did not invert, or one whose key was only a prefilter.
    /// There is no class to share, so the render is per subscriber.
    Unindexed,
}

/// Everything an emitter is told about one frame it is asked to build.
#[derive(Clone, Copy, Debug)]
pub struct Emit<'a> {
    /// The topic the frame is addressed to. Becomes the SSE `event:` name.
    pub topic: &'a Topic,
    /// What changed.
    pub change: &'a RowChange<'a>,
    /// The changed row's columns, when the broker read them. `None` when no
    /// subscriber needed a column — every audience on this topic was
    /// `Everyone` — or when the row could not be read.
    pub row: Option<&'a Row>,
    /// Which audience class this render is for.
    pub class: AudienceClass<'a>,
    /// The monotonic frame id to put in `id:`.
    pub id: u64,
}

/// Turns a change into the bytes a subscriber receives.
///
/// Implement this to send fragments. The implementation that renders `rsx`
/// belongs in a crate that depends on both, which is PLAN.md §8's rule about
/// where an arrow between two plugins lives.
///
/// Runs under the broker's read lock, once per audience class per topic. Keep
/// it a render: no queries, no locks, no blocking. Returning `None` suppresses
/// the frame for that whole class, which is how an emitter declines a change
/// it has nothing to say about.
pub trait FrameEmitter: Send + Sync {
    /// Build the frame, or decline.
    fn emit(&self, emit: &Emit<'_>) -> Option<Frame>;
}

impl<F> FrameEmitter for F
where
    F: Fn(&Emit<'_>) -> Option<Frame> + Send + Sync,
{
    fn emit(&self, emit: &Emit<'_>) -> Option<Frame> {
        self(emit)
    }
}

/// The default emitter: the change's identity, one `key=value` per line.
///
/// A placeholder with a job. See the module documentation for why the default
/// is neither JSON nor a fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeLine;

impl FrameEmitter for ChangeLine {
    fn emit(&self, emit: &Emit<'_>) -> Option<Frame> {
        let change = emit.change;
        let action = match &change.kind {
            ChangeKind::Created => "create",
            ChangeKind::Updated { .. } => "update",
            ChangeKind::Deleted => "delete",
        };
        let mut body = String::with_capacity(64);
        let _ = writeln!(body, "action={action}");
        let _ = writeln!(body, "collection={}", change.collection);
        let _ = write!(body, "id={}", change.row);
        if let ChangeKind::Updated { fields } = &change.kind {
            if !fields.is_empty() {
                let names: Vec<&str> = fields.iter().collect();
                let _ = write!(body, "\nfields={}", names.join(","));
            }
        }
        Some(Frame::sse(emit.id, emit.topic, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitted(change: &RowChange<'_>) -> String {
        let topic = Topic::collection("posts").unwrap();
        let emit = Emit {
            topic: &topic,
            change,
            row: None,
            class: AudienceClass::Everyone,
            id: 7,
        };
        let frame = ChangeLine.emit(&emit).unwrap();
        assert_eq!(frame.event(), "posts/*");
        assert_eq!(frame.id(), 7);
        String::from_utf8(frame.bytes().to_vec()).unwrap()
    }

    #[test]
    fn a_create_names_the_row() {
        assert_eq!(
            emitted(&RowChange::created("posts", 42i64)),
            "id:7\nevent:posts/*\ndata:action=create\ndata:collection=posts\ndata:id=42\n\n"
        );
    }

    #[test]
    fn an_update_names_the_changed_fields() {
        let fields: &[&str] = &["title", "body"];
        let text = emitted(&RowChange::updated("posts", 42i64, fields));
        assert!(
            text.ends_with("data:id=42\ndata:fields=title,body\n\n"),
            "{text}"
        );
    }

    #[test]
    fn a_body_with_newlines_becomes_several_data_lines() {
        let topic = Topic::collection("posts").unwrap();
        let frame = Frame::sse(1, &topic, "<p>one</p>\n<p>two</p>");
        assert_eq!(
            String::from_utf8(frame.bytes().to_vec()).unwrap(),
            "id:1\nevent:posts/*\ndata:<p>one</p>\ndata:<p>two</p>\n\n"
        );
    }

    #[test]
    fn a_carriage_return_cannot_end_the_frame_early() {
        let topic = Topic::collection("posts").unwrap();
        let frame = Frame::sse(1, &topic, "a\rb");
        assert_eq!(
            String::from_utf8(frame.bytes().to_vec()).unwrap(),
            "id:1\nevent:posts/*\ndata:ab\n\n"
        );
    }

    #[test]
    fn a_closure_is_an_emitter() {
        let topic = Topic::collection("posts").unwrap();
        let change = RowChange::deleted("posts", 1i64);
        let emitter = |emit: &Emit<'_>| Some(Frame::sse(emit.id, emit.topic, "<li></li>"));
        let frame = emitter
            .emit(&Emit {
                topic: &topic,
                change: &change,
                row: None,
                class: AudienceClass::Unindexed,
                id: 1,
            })
            .unwrap();
        assert!(frame.bytes().ends_with(b"data:<li></li>\n\n"));
    }
}
