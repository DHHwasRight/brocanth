//! `mail` -- what `brocanth create` writes when somebody asks to send mail.
//!
//! Compiled only under the `scaffold` feature, which is off. The module
//! writes text and never names this crate's types, so the CLI links none of
//! the runtime.
//!
//! # What the project gets
//!
//! One file, `src/plugins/mail/mod.rs`, with a plugin that builds a
//! [`Mail`](crate::Mail) from `[mail]` and puts an `Outbox` on the request
//! context. A page sends with `outbox.send(&message)` or queues with
//! `outbox.deliver_later(message)`.
//!
//! # Decisions
//!
//! * **The transport is a setting, and the default is `log`.** A
//!   development configuration that is copied to production then sends no
//!   mail to real people. The plugin warns at startup when the transport is
//!   not `smtp`. Reason: a mail that went to a customer from a laptop cannot
//!   be called back; a mail that went to the log can be sent again.
//! * **The SMTP password is a secret.** It has no flag, because every user on
//!   the host can read a process's arguments. It comes from the file or from
//!   `$<APP>_MAIL__SMTP_PASSWORD`.
//! * **`deliver_later` with `jobs` is a job on a queue of its own, `mail`,
//!   run by a worker this plugin starts.** The jobs module owns its registry
//!   in its own file, and a module cannot write into another module's file.
//!   So the mail job registers with a second worker, which takes the
//!   `Queue` that the jobs plugin publishes in the services. A slow relay
//!   then does not hold up the application's other jobs either.
//! * **Without `jobs`, `deliver_later` sends at once.** A page is written
//!   once, and works in both shapes: adding `jobs` later moves the send off
//!   the request with no change to the page.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// Mail, as a choice on the `brocanth create` menu.
pub struct Mail;

impl Module for Mail {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "mail",
            "send mail through SMTP, or to the log or a folder in development",
        )
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "jobs",
            "`deliver_later` puts the message on the `mail` queue and a worker sends it",
        )
        .asked_in("behaviour", "Send mail")
        .krate(
            Dep::brocanth("brocanth-mail")
                .feature("smtp")
                .feature("serde")
                .note("the `Outbox` in src/plugins/mail, and SMTP with rustls"),
        )
        .after("config")
        .after("jobs")
        .absent(
            "brocanth-mail",
            "no mail; a password reset or a verification link cannot leave the server",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/mail/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod mail;");
        // Where the `folder` transport writes. Mail holds reset links, and a
        // reset link in git is a password in git.
        s.at(Slot::GitIgnore).line("/tmp/mail");
        configurable(s, project, "mail", "MailPlugin", settings(project));
        if !project.has("config") {
            // The literal form calls `Plugin::new`, a trait method.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The settings of `[mail]`.
///
/// The development file says `log`, so a new project prints each message
/// and sends nothing.
fn settings(project: &Project) -> Vec<Setting> {
    let mut out = vec![
        Setting::text(
            "transport",
            "log",
            "Where mail goes: `smtp`, `log` or `folder`.",
        )
        .note(
            "`log` writes each message to the log and sends nothing. `folder` writes\n\
             each one as an .eml file into `folder`. Only `smtp` delivers.",
        ),
        Setting::text(
            "from",
            "no-reply@example.com",
            "The address every message comes from.",
        ),
        Setting::text(
            "from_name",
            project.title,
            "The name shown beside that address.",
        ),
        Setting::text(
            "folder",
            "tmp/mail",
            "Where `folder` writes its .eml files.",
        ),
        Setting::text("smtp_host", "localhost", "The SMTP relay."),
        Setting::integer("smtp_port", 587, "The relay's port."),
        Setting::text(
            "smtp_security",
            "starttls",
            "`tls`, `starttls`, or `plain` for a local catcher only.",
        ),
        Setting::text(
            "smtp_username",
            "",
            "The relay's user name. Empty for none.",
        ),
        Setting::text("smtp_password", "", "The relay's password. Empty for none.")
            .note(
                "A secret: no flag sets it. Put it in $<APP>_MAIL__SMTP_PASSWORD rather\n\
             than in a file that is in git.",
            )
            .secret(),
    ];
    if project.has("jobs") {
        out.push(Setting::bool(
            "worker",
            true,
            "Run the worker that sends queued mail in this process.",
        ));
    }
    out
}

/// What the generated `AGENTS.md` says about mail.
const AGENTS: &str = "\n\
    ## Mail\n\
    \n\
    `src/plugins/mail/mod.rs` registers the mail plugin. A page reads the\n\
    `Outbox` with `ctx.provided::<crate::plugins::mail::Outbox, _>()` and\n\
    builds a message with `outbox.message()`, which sets `From`.\n\
    \n\
    Use `outbox.deliver_later(message)` in a request, not `send`: with the\n\
    `jobs` module the message goes on a queue and a slow relay does not hold\n\
    the response. Never build an `Smtp` in a page, and never put the SMTP\n\
    password in a file that is in git or on a command line.\n\
    \n\
    In development `[mail] transport` is `log`: each message is in the log,\n\
    and nothing is sent.\n";

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "mail");
    let jobs = project.has("jobs");
    let mut text = PLUGIN
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@TITLE@", project.title);
    for (marker, with, without) in JOBS_EDITS {
        assert!(text.contains(marker), "the mail scaffold lost `{marker}`");
        text = text.replace(marker, if jobs { with } else { without });
    }
    text
}

/// The parts of the file that change with `jobs`: the marker, the text with
/// `jobs`, the text without it.
const JOBS_EDITS: &[(&str, &str, &str)] = &[
    (
        "@HEADER@\n",
        "//! # Later, on a queue\n\
         //!\n\
         //! With `jobs`, `deliver_later` puts the message on the `mail` queue and\n\
         //! returns. A worker that this plugin starts sends it, and retries it\n\
         //! when the relay refuses. It is a second worker, beside the one in\n\
         //! `src/plugins/jobs/mod.rs`, so a slow relay holds up no other job.\n\
         //! `[mail] worker = false` turns it off in a process that only serves.\n",
        "//! # Later\n\
         //!\n\
         //! There is no queue in this project, so `deliver_later` sends at once.\n\
         //! Add the `jobs` module and it puts the message on a queue instead, with\n\
         //! no change to the pages that call it.\n",
    ),
    (
        "@USES@\n",
        "use std::sync::{Arc, OnceLock};\n\
         \n\
         use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};\n\
         use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};\n\
         use brocanth::plugin_shutdown::ShutdownCx;\n\
         use brocanth_jobs::{Job, JobContext, Queue, Worker, WorkerHandle};\n\
         use brocanth_mail::{\n\
         \x20   Address, Folder, Log, Mail, MailError, Message, MessageBuilder, Security, Smtp, SmtpConfig,\n\
         };\n\
         use serde::{Deserialize, Serialize};\n",
        "use std::sync::Arc;\n\
         \n\
         use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};\n\
         use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};\n\
         use brocanth_mail::{\n\
         \x20   Address, Folder, Log, Mail, MailError, Message, MessageBuilder, Security, Smtp, SmtpConfig,\n\
         };\n",
    ),
    (
        "@WORKER_FIELD@\n",
        "    /// Run the worker that sends queued mail in this process.\n\
         \x20   pub worker: bool,\n",
        "",
    ),
    (
        "@WORKER_DEFAULT@\n",
        "            worker: true,\n",
        "",
    ),
    (
        "@OUTBOX_FIELD@\n",
        "    /// The queue, once the jobs plugin opened it. Empty in a context built\n\
         \x20   /// before `before_start`, which sends at once.\n\
         \x20   queue: Arc<OnceLock<Queue>>,\n",
        "",
    ),
    (
        "@DELIVER_LATER@\n",
        "    /// Sends the message from a worker, after this request.\n\
         \x20   ///\n\
         \x20   /// The message goes on the `mail` queue, and the worker retries a send\n\
         \x20   /// the relay refused. Before the queue is open -- a context built by\n\
         \x20   /// hand -- it sends at once.\n\
         \x20   pub async fn deliver_later(&self, message: Message) -> Result<(), DeliverError> {\n\
         \x20       match self.queue.get() {\n\
         \x20           Some(queue) => {\n\
         \x20               queue.enqueue(DeliverMail { message }).await?;\n\
         \x20               Ok(())\n\
         \x20           }\n\
         \x20           None => Ok(self.mail.send(&message).await?),\n\
         \x20       }\n\
         \x20   }\n",
        "    /// Sends the message. This project has no queue, so it sends at once;\n\
         \x20   /// with the `jobs` module it goes on a queue.\n\
         \x20   pub async fn deliver_later(&self, message: Message) -> Result<(), MailError> {\n\
         \x20       self.mail.send(&message).await\n\
         \x20   }\n",
    ),
    ("@OUTBOX_NEW@\n", "            queue: Arc::default(),\n", ""),
    ("@JOB@\n", JOB, "\n"),
    (
        "@PLUGIN_FIELDS@\n",
        "    /// The worker that sends queued mail, kept to stop it.\n\
         \x20   worker: Option<WorkerHandle>,\n",
        "",
    ),
    ("@PLUGIN_NEW@\n", "            worker: None,\n", ""),
    (
        "@REMEMBER@\n",
        "        // The job reads the mail from here: a worker has no request\n\
         \x20       // context. The first application built in this process decides.\n\
         \x20       let _ = SENDER.set(mail.clone());\n",
        "",
    ),
    ("@STAGES@\n", STAGES, "\n"),
];

/// The job, with `jobs`.
const JOB: &str = r#"
/// What `deliver_later` can fail with: the queue refused the row, or there
/// was no queue and the relay refused the message.
#[derive(Debug)]
pub enum DeliverError {
    Queue(brocanth_jobs::Error),
    Mail(MailError),
}

impl std::fmt::Display for DeliverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queue(e) => write!(f, "the mail was not queued: {e}"),
            Self::Mail(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for DeliverError {}

impl From<brocanth_jobs::Error> for DeliverError {
    fn from(e: brocanth_jobs::Error) -> Self {
        Self::Queue(e)
    }
}

impl From<MailError> for DeliverError {
    fn from(e: MailError) -> Self {
        Self::Mail(e)
    }
}

/// The mail the worker sends, set once when the plugin is built.
static SENDER: OnceLock<Mail> = OnceLock::new();

/// One message, sent from the `mail` queue.
///
/// The message is checked again when the worker reads it back: a row that
/// was edited in the table cannot carry a newline into a header.
#[derive(Serialize, Deserialize)]
pub struct DeliverMail {
    pub message: Message,
}

impl Job for DeliverMail {
    const NAME: &'static str = "mail.deliver";
    const QUEUE: &'static str = "mail";
    type Error = MailError;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        let mail = SENDER
            .get()
            .ok_or_else(|| MailError::transport("the mail plugin was not built in this process"))?;
        mail.send(&self.message).await
    }
}

"#;

/// The worker's stages, with `jobs`.
const STAGES: &str = r#"
    fn start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // The jobs plugin publishes its queue in `before_start`, and every
        // `before_start` runs before any `start`.
        let Some(queue) = lc.services().get::<Queue>().cloned() else {
            return Err(PluginError::new(
                "[mail] the jobs plugin published no queue\n\
                 help: register the jobs plugin before the mail plugin",
            ));
        };
        if let Some(outbox) = &self.outbox {
            let _ = outbox.queue.set(queue.clone());
        }
        if !self.settings.worker {
            return Ok(());
        }
        let worker = Worker::builder(&queue)
            .register::<DeliverMail>()
            .queues(vec![DeliverMail::QUEUE.to_string()])
            .concurrency(2)
            .start()
            .map_err(|e| PluginError::new(format!("[mail] starting the worker: {e}")))?;
        let grace = crate::plugins::shutdown_grace().map_err(PluginError::new)?;
        worker.stop_when(lc.readiness().draining(), grace);
        self.worker = Some(worker);
        Ok(())
    }

    fn begin_shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        // A message that does not go out in time goes back on the queue, and
        // another process sends it.
        if let Some(worker) = &self.worker {
            worker.stop(sd.remaining().unwrap_or(crate::app::SHUTDOWN_GRACE));
        }
        Ok(())
    }

"#;

const PLUGIN: &str = r#"//! Mail, as this application sends it.
//!
//! A page reads the `Outbox` from the context and builds a message from it,
//! so `From` is always the configured sender:
//!
//! ```ignore
//! let outbox = ctx.provided::<Outbox, _>().expect("a dispatched request");
//! let message = outbox
//!     .message()
//!     .to(brocanth_mail::Address::parse("person@example.test")?)
//!     .subject("Welcome")
//!     .text("Hello")
//!     .build()?;
//! outbox.deliver_later(message).await?;
//! ```
//!
//! # Where mail goes
//!
//! `[mail] transport` decides: `log` writes each message to the log, `folder`
//! writes each one as an `.eml` file into `[mail] folder`, and `smtp` sends it
//! to `[mail] smtp_host`. Only `smtp` delivers, so a development file copied
//! to production sends nothing to real people, and the start says so.
//!
//! The SMTP password has no flag: every user on the host can read a process's
//! arguments. Set `$<APP>_MAIL__SMTP_PASSWORD`.
//!
@HEADER@
@USES@

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// `smtp`, `log` or `folder`.
    pub transport: String,
    /// The address every message comes from.
    pub from: String,
    /// The name shown beside it. Empty for none.
    pub from_name: String,
    /// Where `folder` writes.
    pub folder: String,
    /// The relay.
    pub smtp_host: String,
    /// The relay's port.
    pub smtp_port: u16,
    /// `tls`, `starttls`, or `plain` for a local catcher only.
    pub smtp_security: String,
    /// The relay's user name. Empty for none.
    pub smtp_username: String,
    /// The relay's password. Empty for none. A secret: no flag sets it.
    pub smtp_password: String,
@WORKER_FIELD@
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            transport: "log".to_string(),
            from: "no-reply@example.com".to_string(),
            from_name: "@TITLE@".to_string(),
            folder: "tmp/mail".to_string(),
            smtp_host: "localhost".to_string(),
            smtp_port: 587,
            smtp_security: "starttls".to_string(),
            smtp_username: String::new(),
            smtp_password: String::new(),
@WORKER_DEFAULT@
        }
    }
}

/// What a page sends mail through.
#[derive(Clone)]
pub struct Outbox {
    mail: Mail,
    transport: String,
@OUTBOX_FIELD@
}

impl Outbox {
    /// A message from the configured sender.
    pub fn message(&self) -> MessageBuilder {
        self.mail.message()
    }

    /// Sends the message now, and waits for the transport to take it.
    pub async fn send(&self, message: &Message) -> Result<(), MailError> {
        self.mail.send(message).await
    }

@DELIVER_LATER@

    /// Where mail goes: `smtp`, `log` or `folder`.
    pub fn transport(&self) -> &str {
        &self.transport
    }
}

impl ContextItem for Outbox {}
@JOB@
pub struct MailPlugin {
    settings: Settings,
    /// Built at `before_start`, so this is an `Option` for the length of the
    /// build.
    outbox: Option<Outbox>,
@PLUGIN_FIELDS@
}

impl MailPlugin {
    /// The backend `[mail] transport` names.
    fn mailer(&self) -> Result<Arc<dyn brocanth_mail::Mailer>, PluginError> {
        let s = &self.settings;
        Ok(match s.transport.as_str() {
            "log" => Arc::new(Log::new(|message: &Message| {
                let to: Vec<&str> = message.to().iter().map(Address::address).collect();
                let to = to.join(", ");
                let subject = message.subject().as_str();
                let text = message.text();
                crate::plugins::say!(info, "mail to {to}: {subject}\n{text}"; %to, %subject);
            })),
            "folder" => Arc::new(Folder::new(&s.folder)),
            "smtp" => {
                let security = match s.smtp_security.as_str() {
                    "tls" => Security::ImplicitTls,
                    "starttls" => Security::StartTls,
                    "plain" => Security::PlaintextForLocalCatcherOnly,
                    other => {
                        return Err(PluginError::new(format!(
                            "[mail] smtp_security is `{other}`\n\
                             help: set it to `tls`, `starttls` or `plain`"
                        )))
                    }
                };
                let mut config = SmtpConfig::new(&s.smtp_host)
                    .with_security(security)
                    .with_port(s.smtp_port);
                if !s.smtp_username.is_empty() {
                    config = config.with_credentials(&s.smtp_username, &s.smtp_password);
                }
                Arc::new(
                    Smtp::connect(&config).map_err(|e| PluginError::new(format!("[mail] {e}")))?,
                )
            }
            other => {
                return Err(PluginError::new(format!(
                    "[mail] transport is `{other}`\nhelp: set it to `smtp`, `log` or `folder`"
                )))
            }
        })
    }
}

impl Plugin for MailPlugin {
    type Config = Settings;
    type Provides = ProvidesContext<Outbox>;
    const NAME: &'static str = "mail";

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            outbox: None,
@PLUGIN_NEW@
        }
    }

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let s = &self.settings;
        let from = if s.from_name.is_empty() {
            Address::parse(&s.from)
        } else {
            Address::named(&s.from_name, &s.from)
        }
        .map_err(|e| PluginError::new(format!("[mail] from: {e}\nhelp: set a plain address")))?;
        // The SMTP pool spawns its upkeep on the runtime, so it needs the
        // runtime entered. It opens no connection here.
        let mailer = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let _entered = runtime.enter();
                self.mailer()?
            }
            Err(_) => self.mailer()?,
        };
        if self.settings.transport != "smtp" {
            let transport = &self.settings.transport;
            crate::plugins::say!(
                warn,
                "[mail] transport is {transport}: nothing is delivered";
                %transport
            );
        }
        let mail = Mail::new(mailer, from);
@REMEMBER@
        self.outbox = Some(Outbox {
            mail,
            transport: self.settings.transport.clone(),
@OUTBOX_NEW@
        });
        Ok(())
    }
@STAGES@
    fn provide(&self, cx: &mut ContextBuilder) {
        // `before_start` runs before the server is handed the application, so
        // the outbox is here by the time any request builds a context.
        if let Some(outbox) = &self.outbox {
            cx.insert(outbox.clone());
        }
    }
}
"#;
