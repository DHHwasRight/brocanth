//! Accounts, as a tab in `brocanth-admin`.
//!
//! Two screens. A list of accounts, and one account with the sessions open on
//! it. It is the most used screen in Django's admin (`docs/backoffice.md` J8)
//! and PocketBase's user management (P5), and until [`AccountDirectory`]
//! arrived it could not be built: [`AuthStore`](crate::storage::AuthStore)
//! could fetch an account by id and by address and could not list them.
//!
//! # Why this lives here and not in the admin
//!
//! `brocanth-admin` has four dependencies and no password hasher. An accounts
//! panel written there would put argon2, a JWT library and an OAuth client in
//! the binary of every application that has a backoffice. The admin owns the
//! seam -- [`Panel`] -- and the crate that owns the data owns the screens,
//! behind an `admin` feature that is off.
//!
//! # What is not here, and why
//!
//! **No create-account form and no change-password form.** Hashing a password
//! is this crate's job, through [`Passwords`](crate::password::Passwords),
//! and the admin has no widget for a secret: no confirmation field, no
//! strength rule, no "do not log this" on the submission, and nothing that
//! stops the value reaching a request log on its way here. A screen that takes
//! a password needs all four of those decided first, and they are decisions
//! about the admin's form layer rather than about accounts.
//!
//! What an operator can do instead is end a session, which is the action that
//! matters at three in the morning, and which
//! [`AuthStore::delete_sessions_for_account`] already supports.
//!
//! **No session count in the list.** One count per row is one query per row,
//! and the trait has no batched count. The row links to the account, and the
//! account's own screen lists its sessions in one call.
//!
//! **No search box and no filter.** [`AccountDirectory::accounts`] pages and
//! does not search. A search over addresses is a second read and it is worth
//! asking for deliberately rather than faking with a client-side filter over
//! one page.
//!
//! # The buttons are POSTs
//!
//! Ending every session of an account changes something, so it is a form and
//! not a link: a crawler, a prefetching browser or a pasted URL must not be
//! able to fire it. The admin asks the policy for
//! [`Action::ActOnPanel`](brocanth_admin::policy::Action::ActOnPanel) before
//! this code runs, and a reader who may see the tab but not press its buttons
//! gets the tables with the buttons missing.
//!
//! [`AuthStore::delete_sessions_for_account`]:
//!     crate::storage::AuthStore::delete_sessions_for_account

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::clock::UnixTime;
use crate::identity::{Account, AccountId};
use crate::session::Privilege;
use crate::storage::{AccountDirectory, SessionRecord, StoreError};

/// The panel's stylesheet, built from `style/panel.css`. Inlined by every
/// screen, because the admin mounts no asset route and a panel is in no
/// position to add one. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// How many accounts a page shows when nothing says otherwise.
const PER_PAGE: u32 = 50;

/// Accounts as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(AccountsPanel::new(store.clone()))
/// ```
///
/// `store` is the same `Arc` the application handed
/// [`AuthConfig`](crate::plugin::AuthConfig), so the tab shows the accounts
/// the application authenticates against and not a second copy of them.
pub struct AccountsPanel<S: AccountDirectory> {
    store: Arc<S>,
    key: String,
    label: String,
    per_page: u32,
}

impl<S: AccountDirectory> AccountsPanel<S> {
    /// The panel, at `accounts`, labelled "Accounts".
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            key: "accounts".to_string(),
            label: "Accounts".to_string(),
            per_page: PER_PAGE,
        }
    }

    /// A different key, for an application that already has an `accounts`
    /// model.
    #[must_use]
    pub fn at(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// A different tab label. "Users" and "Members" are the usual two.
    #[must_use]
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// How many accounts a page shows. Zero is treated as one.
    #[must_use]
    pub fn per_page(mut self, per_page: u32) -> Self {
        self.per_page = per_page.max(1);
        self
    }
}

#[async_trait]
impl<S: AccountDirectory> Panel for AccountsPanel<S> {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post {
            return self.act(&r).await;
        }
        match r.path {
            "" => self.list(&r).await.map(Some),
            "account" => self.account(&r).await,
            _ => Ok(None),
        }
    }
}

impl<S: AccountDirectory> AccountsPanel<S> {
    // ------------------------------------------------------------ screens

    async fn list(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let total = self.store.count_accounts().await.map_err(failed)?;
        let page = r
            .param("page")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1);
        let offset = (page - 1).saturating_mul(self.per_page);
        let accounts = self
            .store
            .accounts(self.per_page, offset)
            .await
            .map_err(failed)?;

        let last = total.div_ceil(u64::from(self.per_page)).max(1);
        let ctx = ListCtx {
            volatile: !self.store.is_durable(),
            total: plural(total, "account", "accounts"),
            rows: accounts.iter().map(|a| self.row(a, r)).collect(),
            summary: format!("Page {page} of {last}"),
            previous: (page > 1).then(|| page_url(r, page - 1)),
            next: (u64::from(page) < last).then(|| page_url(r, page + 1)),
        };
        self.page("Accounts", ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::accounts::Accounts { ctx }.render(w)
        })
    }

    async fn account(&self, r: &PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(id) = r.param("id").map(AccountId::new) else {
            return Ok(None);
        };
        let Some(account) = self.store.account(&id).await.map_err(failed)? else {
            // The panel's own 404. An account that was deleted while
            // somebody held the link is the ordinary way to arrive here.
            return Ok(None);
        };
        let sessions = self.store.sessions_for_account(&id).await.map_err(failed)?;

        let ctx = AccountCtx {
            volatile: !self.store.is_durable(),
            back: r.url(""),
            id: account.id.to_string(),
            email: address_of(&account),
            has_email: account.email.is_some(),
            verified: account.email_verified,
            password: account.password_hash.is_some(),
            created: utc(account.created_at),
            can_act: r.can_act,
            end_href: r.url("account/end-sessions"),
            sessions: sessions.iter().map(session_row).collect(),
        };
        self.page("Account", ctx, |ctx, w| {
            use rsx::Template as _;
            crate::admin_templates::account::Account { ctx }.render(w)
        })
        .map(Some)
    }

    // ------------------------------------------------------------ buttons

    /// Every POST. The admin has already asked the policy whether this person
    /// may press a button here, so what is left is which button.
    ///
    /// The account is in the form body and not in the path, because an
    /// account id is opaque: the trait says so, and a store is free to use one
    /// with a `/` or a `%` in it. A hidden field carries any of them.
    async fn act(&self, r: &PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.path != "account/end-sessions" {
            return Ok(None);
        }
        let Some(raw) = r.submission.get("id") else {
            return Ok(None);
        };
        let id = AccountId::new(raw);
        if self.store.account(&id).await.map_err(failed)?.is_none() {
            return Ok(None);
        }
        self.store
            .delete_sessions_for_account(&id)
            .await
            .map_err(failed)?;
        // Back to the account, and a redirect rather than a re-render, so a
        // reload does not press it again.
        Ok(Some(PanelPage::redirect(account_url(r, raw))))
    }

    // ------------------------------------------------------------ helpers

    fn page<C>(
        &self,
        heading: &str,
        ctx: C,
        view: impl Fn(&C, &mut rsx::Writer<'_>) -> rsx::Result + Send + Sync + 'static,
    ) -> Result<PanelPage, AdminError>
    where
        C: 'static,
    {
        Ok(PanelPage::body(heading, render(&view, &ctx)?))
    }

    fn row(&self, a: &Account, r: &PanelRequest<'_>) -> AccountRow {
        AccountRow {
            id: a.id.to_string(),
            email: address_of(a),
            has_email: a.email.is_some(),
            verified: a.email_verified,
            password: a.password_hash.is_some(),
            created: utc(a.created_at),
            href: account_url(r, a.id.as_str()),
        }
    }
}

fn session_row(s: &SessionRecord) -> SessionRow {
    SessionRow {
        // A prefix, because the whole identifier is the bearer token's lookup
        // key and a screenshot of this page should not carry one. A prefix is
        // enough to tell two rows apart, which is all this column is for.
        id: short(s.id.as_str()),
        privilege: match s.privilege {
            Privilege::Anonymous => "anonymous",
            Privilege::Authenticated => "signed in",
            Privilege::Elevated => "re-proved",
        }
        .to_string(),
        factors: match s.factors.is_empty() {
            // Empty is a pre-login session, or an impersonated one. Both are
            // real and neither proved anything, so the column says so rather
            // than leaving a blank somebody reads as "password".
            true => "none".to_string(),
            false => s
                .factors
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        },
        started: utc(s.created_at),
        last_seen: utc(s.last_seen_at),
        expires: utc(s.absolute_expires_at),
        operator: s.impersonation.as_ref().map(|i| {
            format!(
                "{} — {}",
                short(i.operator.as_str()),
                truncate(&i.reason, 60)
            )
        }),
    }
}

/// The address, or the words that say there is not one.
///
/// An account created from a provider that returned no email has none, and an
/// empty cell in that column reads as a bug rather than as a fact.
fn address_of(a: &Account) -> String {
    match &a.email {
        Some(email) => email.to_string(),
        None => "no address".to_string(),
    }
}

fn account_url(r: &PanelRequest<'_>, id: &str) -> String {
    format!("{}?id={}", r.url("account"), encode(id))
}

fn page_url(r: &PanelRequest<'_>, page: u32) -> String {
    format!("{}?page={page}", r.url(""))
}

/// Percent-encodes everything that is not unreserved.
///
/// Thirty characters of a table rather than a dependency, for the same reason
/// `brocanth_admin::query` writes its own decoder: an account id is opaque and
/// this is the only place in the crate that puts one in a URL.
fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// "412 accounts", "1 account".
fn plural(n: u64, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        _ => format!("{n} {many}"),
    }
}

fn short(id: &str) -> String {
    truncate(id, 12)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// `2023-11-14 22:13 UTC`.
///
/// Written out rather than taken from `chrono`, which this crate does not
/// depend on and should not start depending on to print a column. The clock is
/// not consulted: every time on these screens is an absolute instant, so
/// nothing here changes between two renders of the same rows.
fn utc(at: UnixTime) -> String {
    let (year, month, day) = civil_from_days(at.div_euclid(86_400));
    let seconds = at.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds / 3600,
        (seconds % 3600) / 60
    )
}

/// Howard Hinnant's `civil_from_days`, for days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The store's errors are not the admin's, and the admin's error type has no
/// arm for them. They arrive as a render failure carrying the text, which is
/// what a screen can actually show somebody.
fn failed(e: StoreError) -> AdminError {
    AdminError::Render(rsx::Error::custom(format!("the account store failed: {e}")))
}

// ---------------------------------------------------------------------------
// What the templates are given
// ---------------------------------------------------------------------------
//
// Every field is already a string the screen prints. No formatting happens in
// a template, which is what lets somebody replace one without reimplementing
// the date arithmetic above.

/// One row of the account list.
pub struct AccountRow {
    /// The account's identifier, in full.
    pub id: String,
    /// The address, or the words that say there is not one.
    pub email: String,
    /// Whether there is an address at all, so the screen can grey out the
    /// words rather than print them as if they were one.
    pub has_email: bool,
    /// Whether we have proof the address reaches this person.
    pub verified: bool,
    /// Whether the account has a password. `false` is an account that can only
    /// sign in through a provider, and it is not a broken account.
    pub password: bool,
    /// When it was created.
    pub created: String,
    /// Where the account's own screen is.
    pub href: String,
}

/// The account list.
pub struct ListCtx {
    /// Whether these accounts go when the process does.
    pub volatile: bool,
    /// "412 accounts".
    pub total: String,
    /// The rows of this page.
    pub rows: Vec<AccountRow>,
    /// "Page 2 of 9".
    pub summary: String,
    /// The page before, when there is one.
    pub previous: Option<String>,
    /// The page after, when there is one.
    pub next: Option<String>,
}

/// One session open on an account.
pub struct SessionRow {
    /// The first few characters of the identifier. Never the whole one: see
    /// the comment where it is built.
    pub id: String,
    /// "anonymous", "signed in" or "re-proved".
    pub privilege: String,
    /// What proved it, comma separated, or "none".
    pub factors: String,
    /// When it was opened. A rotation resets this, because a rotation starts a
    /// new session.
    pub started: String,
    /// When it was last used.
    pub last_seen: String,
    /// The hard deadline, which a rotation carries over rather than extends.
    pub expires: String,
    /// The operator behind the session, when somebody is acting as this
    /// account, with the reason they typed.
    pub operator: Option<String>,
}

/// One account.
pub struct AccountCtx {
    /// Whether these accounts go when the process does.
    pub volatile: bool,
    /// The list this came from.
    pub back: String,
    /// The identifier, in full.
    pub id: String,
    /// The address, or the words that say there is not one.
    pub email: String,
    /// Whether there is an address at all.
    pub has_email: bool,
    /// Whether the address is proven.
    pub verified: bool,
    /// Whether the account has a password.
    pub password: bool,
    /// When it was created.
    pub created: String,
    /// Whether to draw the button.
    pub can_act: bool,
    /// Where the "End every session" button posts.
    pub end_href: String,
    /// Every session open on the account, newest first.
    pub sessions: Vec<SessionRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_prints_as_an_instant_a_human_recognises() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_700_000_000), "2023-11-14 22:13 UTC");
        // A leap day, which is where a hand-written calendar goes wrong.
        assert_eq!(utc(1_709_208_000), "2024-02-29 12:00 UTC");
        // Before the epoch, because `UnixTime` is signed and a bad row is
        // better printed than panicked on.
        assert_eq!(utc(-1), "1969-12-31 23:59 UTC");
    }

    #[test]
    fn an_opaque_id_survives_being_put_in_a_url() {
        assert_eq!(encode("abcDEF-_123.~"), "abcDEF-_123.~");
        assert_eq!(encode("a/b c&d=e"), "a%2Fb%20c%26d%3De");
    }
}
