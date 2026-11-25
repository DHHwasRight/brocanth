//! `notifications` -- what `brocanth create` writes when somebody asks for
//! in-app notifications. Compiled only under the `scaffold` feature.
//!
//! A project gets:
//!
//! - `src/plugins/notifications/mod.rs`: the plugin, `account(ctx)` (the
//!   signed-in account), and `notify(ctx, account, notice, email)`, which
//!   stores the row and, with `mail`, sends it by mail and, with
//!   `realtime`, publishes it as a change;
//! - `src/pages/notifications.rs`: `GET /notifications`, the signed-in
//!   account's inbox, and `POST /notifications`, mark one or all read;
//! - `[notifications]` in the config file, or flags without one.

use brocanth_scaffold::snippet::configurable;
use brocanth_scaffold::{
    AgentsSection, Category, ConfigCheck, Contribution, Dep, Manifest, Module, PgPool, Project,
    Scaffold, Setting, Slot,
};

/// In-app notifications, as a choice on the menu.
pub struct Notifications;

impl Module for Notifications {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "notifications",
            "in-app notifications: an inbox per account at /notifications, unread count, mark read",
        )
        .tables(&["brocanth_notifications"])
        .category(Category::Routes)
        .category(Category::Enhancer)
        .requires::<PgPool>("the notifications are rows in the database the `db` module opens")
        .needs(
            "auth",
            "a notification belongs to an account, and the inbox shows the signed-in one",
        )
        .needs(
            "middleware-csrf",
            "the inbox marks read with a form post: without the CSRF check any site marks a reader's notifications read",
        )
        .adapts_to(
            "mail",
            "`notify` with an address sends the notification by mail too, through the outbox",
        )
        .adapts_to(
            "realtime",
            "`notify` publishes the new row as a change on `brocanth_notifications`, with its account",
        )
        .adapts_to(
            "policy",
            "the inbox page ANDs a policy rule over `NotificationColumn` into its `WHERE`",
        )
        .after("auth")
        .after("mail")
        .after("realtime")
        .contributes::<ConfigCheck>(
            Contribution::new("notifications", "`deploy/migrate` for `[notifications]`"),
            true,
        )
        .contributes::<AgentsSection>(
            Contribution::new("notifications", "how to notify an account, and who reads it"),
            true,
        )
        .asked_in(
            "behaviour",
            "In-app notifications, with mail and live push when present",
        )
        .krate(
            Dep::brocanth("brocanth-notifications")
                .feature_with("config", "config")
                .feature_with("realtime", "realtime")
                .note("the notifications table, one inbox per account"),
        )
        .absent(
            "brocanth-notifications",
            "no in-app notifications; an event reaches a person by mail or not at all",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let mail = project.has("mail");
        let realtime = project.has("realtime");
        let policy = project.has("policy");
        s.file(
            "src/plugins/notifications/mod.rs",
            plugin_rs(mail, realtime),
        );
        s.at(Slot::PluginsMod).line("pub mod notifications;");
        configurable(s, project, "notifications", "Notifications", settings(mail));
        s.settings_type(
            "notifications",
            "brocanth_notifications::NotificationsConfig",
        );
        s.file("src/pages/notifications.rs", page_rs(policy));
        s.at(Slot::PagesRoutes).line("/notifications");
        s.at(Slot::ConfigChecks)
            .line(r#"Entry::migrates("notifications", "`brocanth_notifications::SCHEMA`"),"#);
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn settings(mail: bool) -> Vec<Setting> {
    let mut out = vec![
        Setting::bool(
            "migrate",
            true,
            "Create the brocanth_notifications table on the first readiness check.",
        )
        .note("Off, for a project that keeps brocanth_notifications::SCHEMA in a migration."),
        Setting::integer("per_page", 20, "Notifications per page on /notifications."),
    ];
    if mail {
        out.push(Setting::bool(
            "mail",
            true,
            "Send a notification by mail too when notify() is given an address.",
        ));
    }
    out
}

fn plugin_rs(mail: bool, realtime: bool) -> String {
    let (uses, signature, send) = match mail {
        true => (MAIL_USES, MAIL_SIGNATURE, MAIL_SEND),
        false => ("", PLAIN_SIGNATURE, ""),
    };
    PLUGIN_RS
        .replace("@USES@", uses)
        .replace("@SIGNATURE@", signature)
        .replace("@SEND@", send)
        .replace("@PUSH@", if realtime { PUSH } else { "" })
        .replace(
            "@NOTIFY_DOC@",
            match (mail, realtime) {
                (true, true) => "\n//! It also sends the notification by mail when you give an address\n//! (`[notifications] mail`), and publishes the row on the realtime broker.",
                (true, false) => "\n//! It also sends the notification by mail when you give an address\n//! (`[notifications] mail`).",
                (false, true) => "\n//! It also publishes the row on the realtime broker.",
                (false, false) => "",
            },
        )
        .replace(
            "@EXAMPLE_EMAIL@",
            if mail { ", Some(\"ada@example.com\")" } else { "" },
        )
}

const MAIL_USES: &str = "use crate::plugins::mail::Outbox;\n";

const PLAIN_SIGNATURE: &str =
    "pub async fn notify(ctx: &Ctx, account: &str, notice: Notice) -> Result<i64, Error> {";

const MAIL_SIGNATURE: &str = "pub async fn notify(
    ctx: &Ctx,
    account: &str,
    notice: Notice,
    email: Option<&str>,
) -> Result<i64, Error> {";

const MAIL_SEND: &str = r#"    // By mail too, when the caller knows the address and `mail` is on. The
    // outbox queues it with `jobs`, or sends it at once without.
    if let (Some(email), Some(outbox)) = (email, ctx.provided::<Outbox, _>()) {
        if store(ctx)?.config().mail {
            let mut text = notice.body.clone();
            if let Some(url) = &notice.url {
                text.push_str("\n\n");
                text.push_str(url);
            }
            let to =
                brocanth_mail::Address::parse(email).map_err(|e| Error::new(422, e.to_string()))?;
            let message = outbox
                .message()
                .to(to)
                .subject(&notice.title)
                .text(text)
                .build()
                .map_err(|e| Error::internal(e.to_string()))?;
            outbox
                .deliver_later(message)
                .await
                .map_err(|e| Error::internal(e.to_string()))?;
        }
    }
"#;

const PUSH: &str = r#"    // Live: the new row as a change, with its account, so a subscription
    // whose rule is `account = <the reader>` receives it.
    if let Some(realtime) = ctx.provided::<crate::plugins::realtime::Broadcast, _>() {
        brocanth_notifications::publish(&realtime.broker, account, id, &notice);
    }
"#;

const PLUGIN_RS: &str = r#"//! In-app notifications: the plugin, who is reading, and `notify`.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! ```ignore
//! use brocanth_notifications::Notice;
//!
//! let notice = Notice::new("comment", "Ada replied to your post").url("/posts/7");
//! crate::plugins::notifications::notify(ctx, &author_id, notice@EXAMPLE_EMAIL@).await?;
//! ```
//!
//! `notify` stores a row in `brocanth_notifications` for that account.@NOTIFY_DOC@
//! The account reads it at `/notifications`.

@USES@use brocanth_auth::session::Privilege;
use brocanth_notifications::{Notice, NotificationStore};

use crate::pages::{Ctx, Error};
use crate::plugins::auth::Services;

/// The plugin, from the library. `[notifications]` configures it.
pub use brocanth_notifications::NotificationsPlugin as Notifications;

/// The signed-in account, as the notifications store it, or `None` for a
/// visitor with no live session.
pub async fn account(ctx: &Ctx) -> Option<String> {
    // No cookie, no session: no read of the session store.
    let cookie = ctx.header("cookie")?;
    let auth = ctx.provided::<Services, _>()?;
    match auth.sessions().from_cookie_header(cookie).await {
        Ok(session) if session.privilege != Privilege::Anonymous => {
            Some(session.account.to_string())
        }
        _ => None,
    }
}

/// The store, on every dispatched request.
pub fn store(ctx: &Ctx) -> Result<&NotificationStore, Error> {
    ctx.provided::<NotificationStore, _>().ok_or_else(|| {
        Error::new(
            500,
            "no notification store on this context\nhelp: register the notifications plugin",
        )
    })
}

/// Tells `account` about something: one row in its inbox.
///
/// Call it after the change it reports is committed: a notification about
/// a write that rolled back is a lie the reader acts on.
#[allow(dead_code)]
@SIGNATURE@
    let id = store(ctx)?
        .notify(account, &notice)
        .await
        .map_err(|e| Error::internal(e.to_string()))?;
@SEND@@PUSH@    Ok(id)
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    #[tokio::test]
    async fn a_visitor_with_no_session_has_no_inbox() {
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, "/notifications")).await;
        assert_eq!(response.status, 401);
    }

    #[tokio::test]
    async fn marking_read_without_the_csrf_token_is_refused() {
        let app = crate::app::build().expect("the application builds");
        let mut request = Request::new(Verb::Post, "/notifications")
            .with_header("content-type", "application/x-www-form-urlencoded");
        request.body = b"all=1".to_vec().into();
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 403);
    }
}
"#;

fn page_rs(policy: bool) -> String {
    PAGE_RS
        .replace("@RULE@", if policy { RULE_POLICY } else { RULE_NONE })
        .replace(
            "@RULE_USES@",
            if policy {
                "use brocanth_notifications::policy::{Dialect, Pred};\n\
                 use brocanth_notifications::{escape, safe_url, MarkRead, Notification, NotificationColumn};\n"
            } else {
                "use brocanth_notifications::{escape, safe_url, MarkRead};\n"
            },
        )
}

const RULE_NONE: &str = "    // The inbox holds one account, so the list shows that account's rows
    // alone. A rule of your own goes here, as a policy `Condition` over
    // `brocanth_notifications::NotificationColumn`.
    let rule = None;
";

const RULE_POLICY: &str =
    "    // The rule, ANDed into the same `WHERE` as the account. The inbox holds
    // one account already; narrow it here, by kind or anything else.
    let rule = Pred::<NotificationColumn>::Always.condition::<Notification>(Dialect::Postgres)?;
    let rule = Some(&rule);
";

const PAGE_RS: &str = r#"//! `/notifications` -- the signed-in account's notifications, newest first.
//!
//! `GET` lists them, `?before=<id>` for the next page. `POST` with `id=<n>`
//! marks one read, with `all=1` every one. A visitor with no session gets a
//! 401: an inbox belongs to an account.

@RULE_USES@
use crate::pages::{Ctx, Error, Response};
use crate::plugins::notifications::{account, store};

/// GET: the inbox.
pub async fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    let Some(account) = account(ctx).await else {
        return Err(Error::new(401, "sign in to read your notifications"));
    };
    let before = brocanth_notifications_query(&ctx.query, "before").and_then(|v| v.parse().ok());
@RULE@    let store = store(ctx)?;
    let inbox = store.inbox(&account);
    // `per_page` under `[notifications]`.
    let limit = store.config().per_page;
    let unread = inbox
        .unread()
        .await
        .map_err(|e| Error::internal(e.to_string()))?;
    let items = inbox
        .list(before, limit, rule)
        .await
        .map_err(|e| Error::internal(e.to_string()))?;
    let csrf = ctx
        .hooked::<brocanth_middleware::CsrfToken>()
        .map(|token| token.hidden_field())
        .unwrap_or_default();

    let mut body = format!(
        "<!doctype html><title>Notifications</title><h1>Notifications</h1>\
         <p>{unread} unread</p>"
    );
    if unread > 0 {
        body.push_str(&format!(
            "<form method=\"post\" action=\"/notifications\">{csrf}\
             <input type=\"hidden\" name=\"all\" value=\"1\">\
             <button>Mark all read</button></form>"
        ));
    }
    if items.is_empty() {
        body.push_str("<p>Nothing here yet.</p>");
    }
    body.push_str("<ol>");
    for item in &items {
        let title = match item.url.as_deref().filter(|url| safe_url(url)) {
            Some(url) => format!("<a href=\"{}\">{}</a>", escape(url), escape(&item.title)),
            None => escape(&item.title),
        };
        let state = if item.read { "read" } else { "unread" };
        body.push_str(&format!("<li data-state=\"{state}\">{title}"));
        if !item.body.is_empty() {
            body.push_str(&format!("<p>{}</p>", escape(&item.body)));
        }
        if !item.read {
            body.push_str(&format!(
                "<form method=\"post\" action=\"/notifications\">{csrf}\
                 <input type=\"hidden\" name=\"id\" value=\"{}\">\
                 <button>Mark read</button></form>",
                item.id
            ));
        }
        body.push_str("</li>");
    }
    body.push_str("</ol>");
    if items.len() == limit as usize {
        if let Some(last) = items.last() {
            body.push_str(&format!(
                "<a href=\"/notifications?before={}\">Older</a>",
                last.id
            ));
        }
    }
    Ok(Response::ok(body))
}

/// POST: mark one notification read, or all of them, then back to the
/// inbox.
pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let Some(account) = account(ctx).await else {
        return Err(Error::new(401, "sign in to read your notifications"));
    };
    let Some(asked) = MarkRead::from_form(&ctx.body) else {
        return Err(Error::new(400, "send id=<n> or all=1"));
    };
    let store = store(ctx)?;
    let inbox = store.inbox(&account);
    let done = match asked {
        MarkRead::One(id) => inbox.mark_read(id).await.map(|_| ()),
        MarkRead::All => inbox.mark_all_read().await.map(|_| ()),
    };
    done.map_err(|e| Error::internal(e.to_string()))?;
    Ok(Response::empty(303).with_header("location", "/notifications"))
}

/// One parameter of the query string, as it was sent. Only digits are read
/// from it here, so it needs no decoding.
fn brocanth_notifications_query<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}
"#;

const AGENTS: &str = "\n\
    ## Notifications\n\
    \n\
    Tell an account about something with\n\
    `crate::plugins::notifications::notify(ctx, &account, Notice::new(kind, title))`,\n\
    after the change it reports is committed. The account reads it at\n\
    `/notifications`.\n\
    \n\
    - Read and mark only through `store.inbox(&account)`, with the account\n\
    \x20 from `crate::plugins::notifications::account(ctx)`. Never take an\n\
    \x20 account id from a form or the query string: an inbox is the signed-in\n\
    \x20 account's.\n\
    - A link is a path on this site or an http(s) URL; `notify` refuses any\n\
    \x20 other, because the page writes it into an `href`.\n\
    - Do not filter a page of notifications after it is read. Put the rule in\n\
    \x20 the `list` call, so the page and its next link agree.\n";
