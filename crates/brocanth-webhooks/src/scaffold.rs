//! The modules `brocanth create` offers: `webhooks`, `webhooks-in` and
//! `webhooks-out`.
//!
//! `webhooks` is the parent: the crate, and the rule about secrets. The two
//! children are the two directions, and a project picks either or both.
//!
//! * `webhooks-in` writes `src/plugins/webhooks_in/mod.rs`: the settings,
//!   one example source with a typed handler, and a plugin that installs
//!   the crate's [`Receiver`](crate::Receiver) as a request hook. With
//!   `db`, the idempotency keys are rows; without it, they are in memory.
//! * `webhooks-out` writes `src/plugins/webhooks_out/mod.rs`, which names
//!   the crate's plugin and its settings. It needs `db` for the tables and
//!   `jobs` for the retries.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{
    configurable, configurable_hooking, integration, settings_derive, Hooking, PLUGIN_TRAIT_USE,
};
use brocanth_scaffold::{Category, Dep, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The parent: the crate both directions are written in.
pub struct Webhooks;

impl Module for Webhooks {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "webhooks",
            "signed webhooks: verified deliveries in, signed events out; pick a direction below",
        )
        .category(Category::Enhancer)
        .asked_in(
            "doors",
            "Webhooks: receive signed deliveries, send signed events",
        )
        .krate(krate())
        .absent(
            "brocanth-webhooks",
            "no webhooks; a payment provider or a partner has no verified way in or out",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The crate. Each child adds the features its half needs.
fn krate() -> Dep {
    Dep::brocanth("brocanth-webhooks")
}

const AGENTS: &str = "\n\
    ## Webhooks\n\
    \n\
    A webhook secret is a password. It goes in the config file or the\n\
    environment, never in code and never in a flag. Verify every incoming\n\
    delivery through `brocanth_webhooks::Scheme` before you read its body;\n\
    do not compare a signature with `==`, which leaks how many bytes matched.\n";

/// Incoming webhooks: verified, deduplicated, typed.
pub struct WebhooksIn;

impl Module for WebhooksIn {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "webhooks-in",
            "POST /webhooks/{source}: signature, replay window and idempotency key, then your typed handler",
        )
        .category(Category::Middleware)
        .asked_in("doors", "Receive webhooks: signature, replay window, idempotency key")
        .part_of("webhooks")
        .because("it is the incoming half of `brocanth-webhooks`, which the parent brings")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "db",
            "the idempotency keys are rows every instance shares; without it they are in memory",
        )
        // The idempotency keys in the database, when there is one.
        .krate(krate().feature_with("db", "postgres"))
        // The example event derives `Deserialize`. The config plugin brings
        // serde too, but only when `config` is chosen.
        .krate(
            Dep::version("serde", "1")
                .feature("derive")
                .note("a webhook's event is decoded from JSON"),
        )
        // The plugin reads the pool from the services the `db` plugin fills.
        .after("db")
        .after("config")
        // Required: the hook is the module. A project that receives no
        // webhook removes the module, which a toggle cannot do.
        .hook(
            "receive",
            "answer POST {path}/{source} before the CSRF check",
            Hook::required(),
        )
        .absent(
            "webhooks-in",
            "no incoming webhooks; a sender's POST meets the CSRF check and is refused",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let db = project.has("db");
        s.file("src/plugins/webhooks_in/mod.rs", incoming_rs(project, db));
        s.at(Slot::PluginsMod).line("pub mod webhooks_in;");
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "webhooks-in",
                section: "webhooks_in",
                ty: "crate::plugins::webhooks_in::WebhooksIn",
                config_ty: "crate::plugins::webhooks_in::Settings",
                prelude: IN_COMMENT.to_string(),
                adjust: None,
            },
            incoming_settings(db),
        );
        s.at(Slot::AgentsSections).block(IN_AGENTS.to_string());
    }
}

const IN_COMMENT: &str = "
        // -- incoming webhooks ---------------------------------------------
        //
        // Answers POST /webhooks/{source} before the CSRF check: signature,
        // replay window, idempotency key, then the source's handler. The
        // sources are in src/plugins/webhooks_in/mod.rs.
        //
";

const IN_AGENTS: &str = "\n\
    ## Incoming webhooks\n\
    \n\
    A source is a name, a `Scheme` and a `Handler`, added in `configure` in\n\
    `src/plugins/webhooks_in/mod.rs`. Do not add a page for a webhook, and do\n\
    not add its path to the CSRF exempt list: the hook answers before the CSRF\n\
    check and only for a verified delivery. A handler returns `Err` to have\n\
    the sender retry; it runs once per idempotency key.\n";

/// The development secret: valid, and named for what it is.
const DEV_SECRET: &str = "whsec_ZGV2ZWxvcG1lbnQgb25seTogcmVwbGFjZSBtZQ==";

fn incoming_settings(db: bool) -> Vec<Setting> {
    let mut settings = vec![
        Setting::text(
            "path",
            "/webhooks",
            "Where deliveries arrive: {path}/{source}.",
        ),
        Setting::integer("tolerance", 300, "Seconds a signed timestamp may be off.").note(
            "The replay window. A captured delivery is refused once it is older than\n\
                 this. Five minutes is what Stripe and Standard Webhooks recommend.",
        ),
        Setting::integer(
            "max_bytes",
            1_048_576,
            "The largest delivery accepted, in bytes.",
        ),
        Setting::text(
            "secret",
            DEV_SECRET,
            "The `example` source's Standard Webhooks secret.",
        )
        .note(
            "Development only. Replace it with the secret the sender shows you, in\n\
             the environment or this file. No flag sets it: a flag is readable by\n\
             every user on the host.",
        )
        .secret(),
    ];
    if db {
        settings.push(
            Setting::bool(
                "migrate",
                true,
                "Create the idempotency table on the first readiness check.",
            )
            .note(
                "Off, for a project that keeps brocanth_webhooks::PgSeen::SCHEMA in a migration.",
            ),
        );
    }
    settings
}

fn incoming_rs(project: &Project, db: bool) -> String {
    let (derive, note) = settings_derive(project, "webhooks_in");
    let (migrate_field, migrate_default, store_doc, before_start, use_arc) = if db {
        (
            "    /// Create the idempotency table on the first readiness check.\n    pub migrate: bool,\n",
            "            migrate: true,\n",
            "//! The idempotency keys are rows in the `db` module's database, so every\n\
             //! instance sees the same keys and a restart forgets none.",
            BEFORE_START_DB,
            "use std::sync::Arc;\n\n",
        )
    } else {
        (
            "",
            "",
            "//! The idempotency keys are kept in this process's memory: a restart\n\
             //! forgets them, and two instances do not share them. Add the `db`\n\
             //! module and they become rows.",
            "",
            "",
        )
    };
    let signed_test = if db { "" } else { SIGNED_TEST };
    INCOMING_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@STORE_DOC@", store_doc)
        .replace("@MIGRATE_FIELD@", migrate_field)
        .replace("@MIGRATE_DEFAULT@", migrate_default)
        .replace("@BEFORE_START@", before_start)
        .replace("@USE_ARC@", use_arc)
        .replace("@SIGNED_TEST@", signed_test)
        .replace("@SECRET@", DEV_SECRET)
}

const INCOMING_RS: &str = r##"//! Incoming webhooks, as this application receives them.
//!
//! A delivery is a POST to `{path}/{source}`. The hook in this file answers
//! it before the CSRF check, and only after the signature is valid and the
//! timestamp is inside the replay window. Then the body decodes into the
//! source's event type, the idempotency key is claimed, and the handler
//! runs. A copy of a delivery that already ran is answered 200 and runs
//! nothing. A handler that fails gives the key back, and the sender's retry
//! runs it again.
//!
@STORE_DOC@
//!
//! # Adding a source
//!
//! A source is a name, a scheme and a handler. Add it in `configure`:
//!
//! ```ignore
//! .source("stripe", Scheme::stripe(&stripe_secret)?, Payments)?
//! .source("github", Scheme::github(&github_secret)?, Pushes)?
//! ```
//!
//! `Scheme::standard` is Standard Webhooks (svix, and what
//! `brocanth-webhooks` sends), `Scheme::stripe` is Stripe, `Scheme::github`
//! is GitHub, and `Scheme::hmac_sha256` is any sender that puts a hex MAC of
//! the body in one header. A scheme with no timestamp has no replay window;
//! only the idempotency key stops a replay there.

@USE_ARC@use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnAsyncRequest};
use brocanth::plugin_order::Order;
use brocanth_webhooks::{Delivery, Handler, Incoming, Receiver, Rejection, Scheme};

// ---------------------------------------------------------------------------
// A source
// ---------------------------------------------------------------------------

/// What the `example` source sends: a Standard Webhooks envelope. Replace
/// it with the event your sender documents. Fields it does not name are
/// ignored.
#[derive(Debug, serde::Deserialize)]
pub struct ExampleEvent {
    /// The event type: `invoice.paid`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// Handles the `example` source.
pub struct Example;

impl Handler for Example {
    type Event = ExampleEvent;

    async fn handle(&self, event: ExampleEvent, delivery: &Delivery) -> Result<(), Rejection> {
        // Do the work, or record it, before the `Ok`: the sender stops
        // retrying at the 200.
        let (kind, id) = (event.kind, &delivery.id);
        crate::plugins::say!(info, "webhook {kind} received ({id})"; %kind, %id);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// Where deliveries arrive: `{path}/{source}`.
    pub path: String,
    /// Seconds a signed timestamp may be off: the replay window.
    pub tolerance: i64,
    /// The largest delivery accepted, in bytes.
    pub max_bytes: usize,
    /// The `example` source's Standard Webhooks secret, `whsec_...`.
    pub secret: String,
@MIGRATE_FIELD@}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path: "/webhooks".to_string(),
            tolerance: 300,
            max_bytes: 1_048_576,
            secret: "@SECRET@".to_string(),
@MIGRATE_DEFAULT@        }
    }
}

pub struct WebhooksIn {
    settings: Settings,
    /// Built at `configure`, because a secret can be wrong and
    /// `Plugin::new` cannot fail.
    receiver: Option<Receiver>,
}

impl Plugin for WebhooksIn {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "webhooks_in";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            receiver: None,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let example = Scheme::standard(&self.settings.secret)
            .map_err(|e| PluginError::new(format!("[webhooks_in] secret: {e}")))?;
        let receiver = Incoming::new(&self.settings.path)
            .tolerance(self.settings.tolerance)
            .max_bytes(self.settings.max_bytes)
            .source("example", example, Example)?
            .build()?;
        self.receiver = Some(receiver);
        Ok(())
    }
@BEFORE_START@}

impl HookSource for WebhooksIn {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        if let Some(receiver) = &self.receiver {
            // Before the CSRF check, which would refuse a POST with no
            // token. After the proxy reader and the rate limiter, so the
            // client's address is read and a flood is refused first.
            registrar.request_async(
                "receive",
                Order::new()
                    .before_if_present("csrf.check")
                    .after_if_present("proxy-headers.read")
                    .after_if_present("rate_limit.count"),
                receiver.clone(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    #[tokio::test]
    async fn an_unsigned_delivery_is_refused() {
        let app = crate::app::build().expect("the application builds");
        let mut request = Request::new(Verb::Post, "/webhooks/example")
            .with_header("content-type", "application/json");
        request.body = br#"{"type":"invoice.paid"}"#.to_vec().into();
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 401);
    }
@SIGNED_TEST@}
"##;

const BEFORE_START_DB: &str = r#"
    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // The pool the `db` plugin opened. It is lazy, so nothing here
        // waits; the first readiness check creates the table.
        let (Some(receiver), Some(db)) = (
            &self.receiver,
            lc.services().get::<brocanth_db::Db>().cloned(),
        ) else {
            return Ok(());
        };
        let seen = Arc::new(brocanth_webhooks::PgSeen::new(db));
        receiver.set_seen(seen.clone());
        if self.settings.migrate {
            let migrated = Arc::new(tokio::sync::OnceCell::new());
            lc.ready_when(move || {
                let seen = Arc::clone(&seen);
                let migrated = Arc::clone(&migrated);
                async move {
                    migrated
                        .get_or_try_init(|| seen.migrate())
                        .await
                        .map(drop)
                        .map_err(|e| {
                            format!(
                                "creating the webhook keys table: {e}\n\
                                 help: check `[db] url`; with `migrate = false`, run \
                                 `brocanth_webhooks::PgSeen::SCHEMA` in a migration"
                            )
                        })
                }
            });
        }
        Ok(())
    }
"#;

const SIGNED_TEST: &str = r##"
    #[tokio::test]
    async fn a_signed_delivery_runs_once() {
        let app = crate::app::build().expect("the application builds");
        let body = br#"{"type":"invoice.paid"}"#;
        let key = brocanth_webhooks::sign::decode_secret(&super::Settings::default().secret)
            .expect("the development secret is valid");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970")
            .as_secs() as i64;
        let signature = brocanth_webhooks::sign::sign_standard(&key, "msg_test", now, body);
        let delivery = || {
            let mut request = Request::new(Verb::Post, "/webhooks/example")
                .with_header("content-type", "application/json")
                .with_header("webhook-id", "msg_test")
                .with_header("webhook-timestamp", &now.to_string())
                .with_header("webhook-signature", &signature);
            request.body = body.to_vec().into();
            request
        };
        let first = crate::app::handle(&app, delivery()).await;
        assert_eq!(first.status, 200);
        let again = crate::app::handle(&app, delivery()).await;
        assert_eq!(again.status, 200);
        let text = again.body.bytes().expect("a whole body");
        assert_eq!(text, "already received");
    }
"##;

/// Outgoing webhooks: signed, retried, logged.
pub struct WebhooksOut;

impl Module for WebhooksOut {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "webhooks-out",
            "signed outgoing webhooks: endpoints and a delivery log in Postgres, retries through jobs",
        )
        .category(Category::Enhancer)
        .asked_in("doors", "Send signed webhooks, retried through jobs")
        .part_of_but_off("webhooks")
        .because("it is the outgoing half of `brocanth-webhooks`, which the parent brings")
        .needs(
            "db",
            "the endpoints and the delivery log are rows in the `db` module's database",
        )
        .needs(
            "jobs",
            "a delivery is a job: its retries, backoff and dead letter are the queue's",
        )
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .krate(
            krate()
                .feature("out")
                .feature_with("config", "config")
                .feature_with("admin", "admin"),
        )
        .krate(
            Dep::version("serde_json", "1").note("an outgoing event's payload is JSON"),
        )
        .after("db")
        .after("jobs")
        .after("config")
        .integration(
            "admin",
            "a tab at /admin/webhooks with the endpoints, the deliveries and a Send again button",
            true,
        )
        .absent(
            "webhooks-out",
            "no outgoing webhooks; a partner polls for what changed",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/webhooks_out/mod.rs", OUTGOING_RS.to_string());
        s.at(Slot::PluginsMod).line("pub mod webhooks_out;");
        configurable(
            s,
            project,
            "webhooks_out",
            "WebhooksOut",
            outgoing_settings(),
        );
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("admin") {
            integration(s, "webhooks-out", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        s.at(Slot::AgentsSections).block(OUT_AGENTS.to_string());
    }
}

const ADMIN_TAB: &str = "// The outgoing deliveries, with a Send again button, as a tab.
config = config.panel(brocanth_webhooks::admin::DeliveriesPanel::installed());";

fn outgoing_settings() -> Vec<Setting> {
    vec![
        Setting::bool(
            "migrate",
            true,
            "Create the two tables on the first readiness check.",
        )
        .note("Off, for a project that keeps brocanth_webhooks::Outgoing::SCHEMA in a migration."),
        Setting::bool("worker", true, "Run the delivery worker in this process."),
        Setting::integer("concurrency", 4, "How many deliveries run at once."),
        Setting::integer("timeout_secs", 10, "Seconds an attempt may take."),
        Setting::bool(
            "allow_private",
            true,
            "Accept an endpoint on a loopback or private address.",
        )
        .note(
            "True here because this file is the development one, and the receiver is\n\
             often on this machine. Production reads the default, false: a webhook URL\n\
             is chosen by whoever registers it, and this stops it reaching your network.",
        ),
    ]
}

const OUTGOING_RS: &str = r#"//! Outgoing webhooks, as this application sends them.
//!
//! An endpoint is a URL, a secret and the events it wants. An event goes to
//! every active endpoint that wants it: one row in the delivery log and one
//! job each. The job posts the event, signed with Standard Webhooks, and the
//! queue retries a failure with backoff, eight times over about a day.
//!
//! A page reaches it through the context:
//!
//! ```ignore
//! let hooks = ctx.provided::<brocanth_webhooks::Outgoing, _>().expect("dispatched");
//!
//! // Once, when a partner registers. Show the secret to them, once.
//! let endpoint = hooks
//!     .add_endpoint("https://partner.example/hooks", &["invoice.*"], "billing")
//!     .await?;
//!
//! // Whenever something happens.
//! hooks.emit("invoice.paid", &serde_json::json!({ "id": 42 })).await?;
//! ```
//!
//! A row change becomes `<collection>.created`, `.updated` or `.deleted`
//! through `hooks.sink(&["invoices"])`, a `ChangeSink` the model layer
//! writes through. A change sink cannot fail the write, so an event that must
//! not be lost is emitted by the code that wrote the row.
//!
//! The deliveries are retried on the `webhooks` queue by a worker this
//! plugin runs, over the `db` module's pool. With the admin, they are a tab
//! at `/admin/webhooks`.

pub use brocanth_webhooks::{OutgoingConfig as Settings, OutgoingPlugin as WebhooksOut};
"#;

const OUT_AGENTS: &str = "\n\
    ## Outgoing webhooks\n\
    \n\
    Send an event with `Outgoing::emit`, from the code that changed the data.\n\
    Do not post to a partner's URL from a page: the delivery log, the\n\
    signature and the retries come from `emit`. Show an endpoint's secret\n\
    once, to the person who registered it, and never log it.\n";
