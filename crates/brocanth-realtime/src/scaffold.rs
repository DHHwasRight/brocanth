//! `realtime` -- what `brocanth create` writes when somebody asks for a
//! broker.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: `Broker::builder`, the four fields of
//! [`Config`](crate::Config), the three spellings of
//! [`Overflow`](crate::Overflow), and the fact that a broker is built
//! synchronously and needs no runtime. Those move when this crate moves, and
//! a generator living in another repository would learn about it from a bug
//! report.
//!
//! # What it does not write, and will not until the core changes
//!
//! A transport. The crate documentation gives the argument in full; the short
//! form is that `brocanth::plugin_hooks::Response` is `http::Response<Vec<u8>>`
//! and `ResponseHook::on_response` is specified to see a complete body, so a
//! streaming response would silently skip compression, the partial-navigation
//! headers and `ETag`. SSE is therefore a change to the hook contract, not a
//! type change, and a websocket is that plus an upgrade out of the
//! request/response shape entirely. PLAN.md §8: widen the seam, do not
//! special-case the plugin.
//!
//! So this module scaffolds the half that pays for itself today -- the index,
//! the mailboxes and the frames -- and the generated file says in its own
//! header what reads them.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The broker, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Realtime;

impl Module for Realtime {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "realtime",
            "one write is a map lookup, not a query per subscriber; no transport yet",
        )
        .krate(Dep::brocanth("brocanth-realtime"))
        // Only for the order of the `[realtime]` section and the builder
        // line: the broker reads its limits from the file the config plugin
        // loaded, so it belongs under it rather than above it.
        .after("config")
        .absent(
            "brocanth-realtime",
            "no broker; a write tells nobody, and a client finds out by asking again",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/realtime.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod realtime;");
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "realtime", "Realtime"));
        // Also what makes the broker a thing the project reads rather than
        // a field nothing touches: a generated application that warns on
        // its first build is a bad first minute.
        s.at(Slot::AppContext).block(
            "    let realtime: &plugins::realtime::Broadcast = cx.extract();\n\
            \x20   eprintln!(\n\
            \x20       \"realtime: up to {} clients\",\n\
            \x20       realtime.broker.config().max_clients\n\
            \x20   );"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(SECTION.to_string());
        }
    }
}

/// Server-sent events, listed and not available.
///
/// On the menu rather than absent from it, because the question "where is
/// SSE" has an answer and the place to read it is where it is asked. What
/// the core must change is in this crate's own documentation, in three
/// numbered points.
pub struct Sse;

impl Module for Sse {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "realtime-sse",
            "one long response per subscriber, frames written as they are made",
        )
        .part_of_but_off("realtime")
        .because("it writes the frames the broker makes, and the broker is realtime")
        .blocked("the core's Response is finished before a hook sees it")
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // Never reached while the CLI honours `Manifest::blocked`. Here so
        // that a CLI which does not stops rather than writing a project
        // whose pages cannot compile.
        s.refuse(
            "realtime-sse cannot be scaffolded: the core's response body is a complete              Vec<u8> and ResponseHook is specified to see one, so a streaming body is a              change to the hook contract rather than a change here",
        );
    }
}

/// Websockets, listed and not available.
///
/// A longer way off than [`Sse`]: everything server-sent events need, and
/// then an upgrade out of the request/response shape, the frame protocol,
/// and a second path for writes that no hook sees. The row says the part
/// that is only true here, because a reason read beside a flag has no row
/// above it to lean on.
pub struct WebSocket;

impl Module for WebSocket {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "realtime-websocket",
            "a duplex socket per subscriber, frames both ways",
        )
        .part_of_but_off("realtime")
        .because("it carries the frames the broker makes, and the broker is realtime")
        .blocked("an upgrade out of the request/response shape, plus a frame codec")
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.refuse(
            "realtime-websocket cannot be scaffolded: it needs everything server-sent              events need, and an upgrade out of the request/response shape on top, which              is a second exit from brocanth-server rather than a new body type",
        );
    }
}

/// The `[realtime]` section, with the numbers the crate defaults to and the
/// arithmetic that decides whether they are right here.
const SECTION: &str = "\n[realtime]\n\
# Refused at the limit, not traded for an existing connection: evicting a\n\
# subscriber to admit a new one lets anyone holding a socket knock everybody\n\
# else off. The memory is max_clients * queue * the size of a frame.\n\
max_clients = 1000\n\
# A page asking for more than this is almost certainly asking row by row for\n\
# something the collection wildcard covers.\n\
max_subscriptions_per_client = 64\n\
queue = 32\n\
# drop-oldest, drop-newest or disconnect. The reader is told how many frames\n\
# it missed either way, so a stale view is a thing the client can see.\n\
overflow = \"drop-oldest\"\n";

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "realtime");
    format!(
        r##"//! The realtime broker, as this application registers it.
//!
//! A subscription is an index entry, not a query. `brocanth-policy` is a
//! value rather than a string, so "the owner" inverts into `(author_id, 7)`,
//! the broker files the subscriber under that key, and a write becomes a map
//! lookup. PocketBase runs one `SELECT` per subscriber instead, which is
//! about 180 ms on Postgres at six hundred of them.
//!
//! `brocanth-realtime` is in `Cargo.toml` because this file is here, and
//! through no other route. Delete the registration in `src/app.rs` and delete
//! this file, and the application broadcasts nothing and links nothing to do
//! it with.
//!
//! # Publishing
//!
//! A handler that just wrote a row has the row, so it hands it over and the
//! broker reads nothing:
//!
//! ```ignore
//! let realtime: &Broadcast = cx.extract();
//! let row = Row::new().with("author_id", author);
//! realtime
//!     .broker
//!     .publish_with_row(&RowChange::created("posts", id), &row);
//! ```
//!
//! `src/app.rs` extracts it once at startup and prints the client limit,
//! which is there to be deleted: it exists so a brand-new project reads the
//! value it was given rather than warning that nothing does.
//!
//! # Reading
//!
//! The broker stops one layer short of the socket, on purpose: this
//! application's `Response` is a complete `Vec<u8>`, and every response hook
//! is specified to see one. A streaming body would skip compression, the
//! partial-navigation headers and `ETag` without saying so, so server-sent
//! events are a change to the core's hook contract rather than a type change
//! here.
//!
//! Three things read a mailbox today, none of them needing that change:
//!
//! 1. **In process.** `Broker::connect`, one subscription, and a thread on
//!    `Mailbox::recv_blocking`. This is how a cache invalidator or an audit
//!    log is written, and it is the reason to have a broker before there is a
//!    transport at all.
//! 2. **Long polling.** Keep the `Client` in the session; each request calls
//!    `Mailbox::recv_timeout` for a few seconds, then `Mailbox::drain`, and
//!    returns the frames as one finished body. Every hook runs exactly as
//!    specified. It costs a request per interval and a little latency.
//! 3. **Nothing.** The index costs what it holds, and an application that
//!    publishes without subscribers pays for a map lookup that finds no one.

use std::sync::Arc;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_realtime::{{Broker, Config, Overflow}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// How many connections the broker will hold. At the limit `connect`
    /// refuses rather than evicting, so a new socket cannot displace an
    /// established one.
    pub max_clients: usize,
    /// How many topics one client may hold, so a client cannot spend the
    /// server's memory by asking.
    pub max_subscriptions_per_client: usize,
    /// How many frames a mailbox holds before the overflow policy fires.
    pub queue: usize,
    /// `drop-oldest`, `drop-newest` or `disconnect`. A string here and an
    /// enum below: the config file is text, and the translation is checked
    /// at startup where a typo can still be answered.
    pub overflow: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            max_clients: 1000,
            max_subscriptions_per_client: 64,
            queue: 32,
            overflow: "drop-oldest".to_string(),
        }}
    }}
}}

/// What this plugin puts on the context.
///
/// A wrapper and not `Arc<Broker>` itself, because `ContextItem` is
/// brocanth's trait and `Broker` is `brocanth-realtime`'s type. That crate
/// deliberately does not depend on the core -- nothing in it touches a
/// request, a response or a route -- so the impl that joins the two belongs
/// in the one crate that can see both, which is this one.
pub struct Broadcast {{
    /// The broker. Shared, so a clone is a pointer and every request sees
    /// the same index.
    pub broker: Arc<Broker>,
}}

impl ContextItem for Broadcast {{}}

pub struct Realtime {{
    settings: Settings,
    /// Built at `configure` and not in `new`, because reading the overflow
    /// policy out of a string can fail and `Plugin::new` cannot.
    broker: Option<Arc<Broker>>,
}}

impl Plugin for Realtime {{
    type Config = Settings;
    type Provides = ProvidesContext<Broadcast>;
    const NAME: &'static str = "realtime";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            broker: None,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let overflow = match self.settings.overflow.as_str() {{
            "drop-oldest" => Overflow::DropOldest,
            "drop-newest" => Overflow::DropNewest,
            "disconnect" => Overflow::Disconnect,
            other => {{
                return Err(PluginError::new(format!(
                    "[realtime] overflow is {{other:?}}; write drop-oldest, \
                     drop-newest or disconnect"
                )))
            }}
        }};
        if self.settings.queue == 0 {{
            return Err(PluginError::new(
                "[realtime] queue must hold at least one frame",
            ));
        }}
        if self.settings.max_clients == 0 {{
            return Err(PluginError::new(
                "[realtime] max_clients is 0, so every connection is refused",
            ));
        }}
        // Synchronous, allocating a few maps, needing no runtime. Unlike a
        // pool, there is nothing here to open, so there is no `before_start`.
        self.broker = Some(
            Broker::builder()
                .config(Config {{
                    max_clients: self.settings.max_clients,
                    max_subscriptions_per_client: self.settings.max_subscriptions_per_client,
                    queue: self.settings.queue,
                    overflow,
                }})
                .build(),
        );
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // the broker is here by the time any request builds a context.
        if let Some(broker) = &self.broker {{
            cx.insert(Broadcast {{
                broker: broker.clone(),
            }});
        }}
    }}
}}
"##
    )
}
