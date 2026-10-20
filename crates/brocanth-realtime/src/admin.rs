//! The broker, as a tab in `brocanth-admin`.
//!
//! One screen, because the broker has one question worth asking.
//!
//! # What a reader sees first, and why
//!
//! PocketBase's realtime screen shows how many clients are connected. That
//! number is easy to read and it says nothing: a thousand idle connections
//! cost nothing, and forty subscriptions on the slow path cost a policy
//! evaluation each, per write, for ever.
//!
//! So the first thing on this screen is the **indexed / scanned split**.
//! `indexed` is a map lookup; `scanned` is an in-memory evaluation per
//! subscription per change. The crate documentation calls the second one the
//! fallback and says it is still better than PocketBase's only path — but it
//! is the path you want to leave, and [`Broker::audit`] names the topic and
//! the reason, which is what the table under the split is.
//!
//! Clients, subscriptions and topics are under that, as the size of the
//! thing. They are the context for the split, not the headline.
//!
//! # Read only, on purpose
//!
//! There is no disconnect button and no unsubscribe button. A broker's state
//! belongs to the connections that made it: closing a client from a
//! backoffice tab is an operation the transport owns, and this crate does not
//! ship the transport. So the panel ignores
//! [`PanelRequest::can_act`](brocanth_admin::panel::PanelRequest::can_act) —
//! there is nothing to hide from a reader who may not act.
//!
//! # Why this lives here and not in the admin
//!
//! The same reason `brocanth-jobs` gives. `brocanth-admin` has four
//! dependencies and intends to keep them; a realtime tab written there would
//! put the broker in the binary of every application with a backoffice. The
//! admin owns the seam and this crate owns the screen, behind an `admin`
//! feature that is off.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::broker::{Broker, Fallback, Stats};

/// The panel's stylesheet, built from `style/panel.css` and inlined by the
/// screen. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// The broker as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(RealtimePanel::new(broker.clone()))
/// ```
pub struct RealtimePanel {
    broker: Arc<Broker>,
    key: String,
    label: String,
}

impl RealtimePanel {
    /// The panel, at `realtime`, labelled "Realtime".
    pub fn new(broker: Arc<Broker>) -> Self {
        Self {
            broker,
            key: "realtime".to_string(),
            label: "Realtime".to_string(),
        }
    }

    /// A different key, for an application that already has a `realtime`
    /// model or runs two brokers.
    #[must_use]
    pub fn at(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// A different tab label.
    #[must_use]
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    fn overview(&self) -> Result<PanelPage, AdminError> {
        let ctx = overview(&self.broker.stats(), &self.broker.audit());
        let html = render(
            &|ctx: &OverviewCtx, w: &mut rsx::Writer<'_>| {
                use rsx::Template as _;
                crate::admin_templates::overview::Overview { ctx }.render(w)
            },
            &ctx,
        )?;
        Ok(PanelPage::body("Realtime", html))
    }
}

#[async_trait]
impl Panel for RealtimePanel {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        // Nothing here changes anything, so a POST is not a screen this
        // panel serves and the admin turns it into a 404.
        if r.post || !r.path.is_empty() {
            return Ok(None);
        }
        self.overview().map(Some)
    }
}

// ---------------------------------------------------------------------------
// Reading the broker into what the screen prints
// ---------------------------------------------------------------------------

/// Builds the screen's context from one snapshot and one audit.
///
/// A free function and not a method, because it is the whole of the panel's
/// judgement — which sentence the split gets — and a test reads better
/// against a [`Stats`] it built than against a broker it had to populate.
fn overview(stats: &Stats, audit: &[Fallback]) -> OverviewCtx {
    let indexed = stats.indexed as u64;
    let scanned = stats.scanned as u64;
    let reachable = indexed + scanned;

    OverviewCtx {
        healthy: scanned == 0,
        verdict: match (reachable, scanned) {
            (0, _) => "Nothing is subscribed, so the index has nothing to do yet.".to_string(),
            (_, 0) => "Every subscription is on the index. A write is a map lookup.".to_string(),
            (_, 1) => {
                "One subscription is checked by hand on every change of its topic.".to_string()
            }
            (_, n) => {
                format!("{n} subscriptions are checked by hand on every change of their topic.")
            }
        },
        indexed,
        scanned,
        share: match reachable {
            0 => String::new(),
            total => format!(
                "{}% of what can receive is on the index.",
                indexed * 100 / total
            ),
        },
        counts: vec![
            CountCard {
                label: "Clients".to_string(),
                value: stats.clients as u64,
                note: "connections held".to_string(),
            },
            CountCard {
                label: "Subscriptions".to_string(),
                value: stats.subscriptions as u64,
                note: "across every client".to_string(),
            },
            CountCard {
                label: "Topics".to_string(),
                value: stats.topics as u64,
                note: "with at least one subscriber".to_string(),
            },
            CountCard {
                label: "Denied".to_string(),
                value: stats.never as u64,
                note: "subscribed, and the policy says never".to_string(),
            },
        ],
        // The one number that is a bug rather than a cost, said in words.
        denied: match stats.never {
            0 => String::new(),
            1 => "One subscription receives nothing at all: its policy denies the subject \
                  outright. That is usually a mistake in the policy rather than an intent."
                .to_string(),
            n => format!(
                "{n} subscriptions receive nothing at all: their policy denies the subject \
                 outright. That is usually a mistake in the policy rather than an intent."
            ),
        },
        fallbacks: audit
            .iter()
            .map(|f| FallbackRow {
                topic: f.topic.to_string(),
                reason: f.reason.to_string(),
                subscribers: f.subscribers as u64,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// What the template is given
// ---------------------------------------------------------------------------
//
// Every field is already what the screen prints. No arithmetic and no
// judgement happens in a template, which is what lets somebody replace it
// without reimplementing the sentences above.

/// One of the four numbers under the split.
pub struct CountCard {
    /// What it counts.
    pub label: String,
    /// How many.
    pub value: u64,
    /// The half-line under it.
    pub note: String,
}

/// One topic on the slow path.
pub struct FallbackRow {
    /// Where, as `collection` or `collection/42`.
    pub topic: String,
    /// Why there is no key, verbatim from the policy.
    pub reason: String,
    /// How many subscriptions pay for it, per change of this topic.
    pub subscribers: u64,
}

/// The screen.
pub struct OverviewCtx {
    /// Whether nothing is on the slow path.
    pub healthy: bool,
    /// The sentence the split gets.
    pub verdict: String,
    /// Subscriptions on a key, or on everyone.
    pub indexed: u64,
    /// Subscriptions evaluated on every change.
    pub scanned: u64,
    /// "95% of what can receive is on the index.", or empty when nothing can.
    pub share: String,
    /// Clients, subscriptions, topics, denied.
    pub counts: Vec<CountCard>,
    /// What the denied count means, or empty when it is zero.
    pub denied: String,
    /// One row per topic with a subscriber on the slow path.
    pub fallbacks: Vec<FallbackRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(indexed: usize, scanned: usize, never: usize) -> Stats {
        Stats {
            clients: 1,
            subscriptions: indexed + scanned + never,
            topics: 1,
            indexed,
            scanned,
            never,
        }
    }

    #[test]
    fn an_empty_broker_does_not_claim_a_perfect_index() {
        let ctx = overview(&stats(0, 0, 0), &[]);
        assert!(ctx.healthy);
        assert!(
            ctx.verdict.contains("Nothing is subscribed"),
            "{}",
            ctx.verdict
        );
        assert!(ctx.share.is_empty());
    }

    #[test]
    fn one_scanned_subscription_is_said_in_the_singular() {
        let ctx = overview(&stats(3, 1, 0), &[]);
        assert!(!ctx.healthy);
        assert_eq!(ctx.share, "75% of what can receive is on the index.");
        assert!(
            ctx.verdict.starts_with("One subscription"),
            "{}",
            ctx.verdict
        );
    }

    #[test]
    fn a_denied_subscription_is_called_out_as_a_likely_bug() {
        let ctx = overview(&stats(1, 0, 2), &[]);
        assert!(
            ctx.denied.contains("2 subscriptions receive nothing"),
            "{}",
            ctx.denied
        );
    }
}
