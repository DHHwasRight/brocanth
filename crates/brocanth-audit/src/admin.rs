//! The history, in `brocanth-admin`: a tab, a journal and a link.
//!
//! - [`AuditPanel`]: the tab at `{admin}/history`. The newest changes of
//!   every table, or of one record with `?table=post&row=7`. Read-only.
//! - [`AuditJournal`]: hears every write the admin makes and records it
//!   with who made it and the values before and after.
//! - [`install`]: both, and a "History" link on every record's detail
//!   screen. Django's History button.
//!
//! Locked like every tab: reading is `Action::ViewPanel` on `history`,
//! which the admin's shipped `DenyAll` refuses until the application says
//! who may.
//!
//! The tab lists changes to every table, so `history` alone does not grant
//! a read of them. Each entry's values and label show only when the admin
//! says this person may read that record (`PanelRequest::may_read`: the
//! `ViewDetail` policy and the model's view rule). Any other entry shows
//! when, who, the table, the key and the column names, with its values
//! hidden.

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::journal::{AdminWrite, Journal, Written};
use brocanth_admin::late::Late;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts::{self, encode, escape};
use brocanth_admin::AdminConfig;

use crate::entry::{format_time, Entry, Field, Kind};
use crate::store::Query;
use crate::trail::Trail;

/// The panel's key: its URL segment and its policy subject.
pub const KEY: &str = "history";

/// Entries on one page.
const PAGE: usize = 50;

/// Adds the history tab, the journal and the "History" link to `config`,
/// over a trail that arrives later, in `trail`: for a generated admin,
/// which mounts before the audit plugin is built. The admin fills the cell
/// from the first request that carries the [`Trail`].
///
/// The admin has one journal, so this replaces one set before.
///
/// ```
/// use brocanth_admin::late::Late;
///
/// let config = brocanth_audit::admin::install(brocanth_admin::AdminConfig::new(), Late::new());
/// # let _ = config;
/// ```
pub fn install(config: AdminConfig, trail: Late<Trail>) -> AdminConfig {
    config
        .panel(AuditPanel {
            trail: trail.clone(),
        })
        .journal(AuditJournal { trail })
        .record_link("History", |r| {
            Some(format!(
                "{}/{KEY}?table={}&row={}",
                r.base_path,
                encode(r.table),
                encode(r.key)
            ))
        })
}

/// [`install`] over this trail, for a test or an application that builds
/// the trail itself.
pub fn install_with(config: AdminConfig, trail: Trail) -> AdminConfig {
    install(config, Late::ready(trail))
}

/// Records every admin write in the trail.
pub struct AuditJournal {
    trail: Late<Trail>,
}

impl AuditJournal {
    /// A journal over this trail.
    pub fn new(trail: Trail) -> AuditJournal {
        AuditJournal {
            trail: Late::ready(trail),
        }
    }

    /// A journal over a trail that arrives later, in `cell`. A write made
    /// before the cell is filled is not recorded.
    pub fn late(cell: Late<Trail>) -> AuditJournal {
        AuditJournal { trail: cell }
    }
}

impl Journal for AuditJournal {
    fn record(&self, write: &AdminWrite<'_>) {
        let Some(trail) = self.trail.get() else {
            return;
        };
        trail.record(Entry {
            id: 0,
            at: 0,
            actor: write.who.map(str::to_string),
            source: "admin".to_string(),
            table: write.table.to_string(),
            row: write.key.to_string(),
            label: write.label.to_string(),
            kind: match write.kind {
                Written::Created => Kind::Created,
                Written::Updated => Kind::Updated,
                Written::Deleted => Kind::Deleted,
            },
            fields: write
                .fields
                .iter()
                .map(|f| Field {
                    name: f.name.clone(),
                    before: f.before.clone(),
                    after: f.after.clone(),
                })
                .collect(),
        });
    }
}

/// The history tab.
pub struct AuditPanel {
    trail: Late<Trail>,
}

impl AuditPanel {
    /// A tab over this trail.
    pub fn new(trail: Trail) -> AuditPanel {
        AuditPanel {
            trail: Late::ready(trail),
        }
    }

    /// A tab over a trail that arrives later, in `cell`: for a generated
    /// admin, which mounts before the plugin is built.
    pub fn late(cell: Late<Trail>) -> AuditPanel {
        AuditPanel { trail: cell }
    }
}

#[async_trait]
impl Panel for AuditPanel {
    fn key(&self) -> &str {
        KEY
    }

    fn label(&self) -> String {
        "History".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if !r.path.is_empty() || r.post {
            return Ok(None);
        }
        let Some(trail) = self.trail.get() else {
            return Ok(Some(PanelPage::body(
                "History",
                format!(
                    "<p class=\"{}\">The audit plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        let table = r.param("table").filter(|t| !t.is_empty());
        let row = r.param("row").filter(|t| !t.is_empty());
        let before = r.param("before").and_then(|b| b.parse::<i64>().ok());
        let query = Query {
            table: table.clone(),
            row: row.clone(),
            before,
            limit: PAGE,
        };
        let mut html = String::new();
        let entries = match trail.find(&query).await {
            Ok(entries) => entries,
            Err(e) => {
                html.push_str(&format!(
                    "<p role=\"alert\" class=\"{}\">The history could not be read: {}</p>",
                    parts::muted(),
                    escape(&e.to_string())
                ));
                Vec::new()
            }
        };
        let heading = match (&table, &row) {
            (Some(t), Some(r)) => format!("History of {t} {r}"),
            (Some(t), None) => format!("History of {t}"),
            _ => "History".to_string(),
        };
        if table.is_some() {
            html.push_str(&format!(
                "<p class=\"{crumbs}\"><a class=\"{action}\" href=\"{all}\">All changes</a></p>",
                crumbs = parts::crumbs(),
                action = parts::action(),
                all = escape(&r.url("")),
            ));
        }
        if let Some(e) = trail.last_error() {
            html.push_str(&format!(
                "<p class=\"{}\">The last write of the history failed: {}. Changes wait in \
                 memory and are written at the next try.</p>",
                parts::muted(),
                escape(&e)
            ));
        }
        let dropped = trail.dropped();
        if dropped > 0 {
            html.push_str(&format!(
                "<p role=\"alert\" class=\"{}\">{dropped} changes were not recorded: the \
                 queue was full while the history could not be written.</p>",
                parts::muted(),
            ));
        }
        if !trail.is_durable() {
            html.push_str(&format!(
                "<p class=\"{}\">The history is kept in memory and is lost at restart: the \
                 application has no database.</p>",
                parts::muted(),
            ));
        }
        if entries.is_empty() {
            html.push_str(&format!(
                "<div class=\"{}\"><div class=\"{}\">No changes recorded.</div></div>",
                parts::card(),
                parts::empty()
            ));
            return Ok(Some(PanelPage::body(heading, html)));
        }
        // Asked once per record, not per entry: a page often holds several
        // changes to one row.
        let mut readable: Vec<((&str, &str), bool)> = Vec::new();
        let mut rows = String::new();
        for e in &entries {
            let at = (e.table.as_str(), e.row.as_str());
            let may = match readable.iter().find(|(k, _)| *k == at) {
                Some((_, may)) => *may,
                None => {
                    let may = r.may_read(&e.table, &e.row).await;
                    readable.push((at, may));
                    may
                }
            };
            rows.push_str(&line(&r, e, may));
        }
        html.push_str(&format!(
            "<div class=\"{card}\"><table class=\"{table}\"><thead><tr>\
             <th class=\"{th}\">When</th><th class=\"{th}\">Who</th>\
             <th class=\"{th}\">Record</th><th class=\"{th}\">Change</th>\
             </tr></thead><tbody>{rows}</tbody></table></div>",
            card = parts::card(),
            table = parts::table(),
            th = parts::th(),
        ));
        if entries.len() == PAGE {
            let last = entries.last().map_or(0, |e| e.id);
            let mut next = format!("?before={last}");
            if let Some(t) = &table {
                next.push_str(&format!("&table={}", encode(t)));
            }
            if let Some(rw) = &row {
                next.push_str(&format!("&row={}", encode(rw)));
            }
            html.push_str(&format!(
                "<p class=\"{bar}\"><a class=\"{button}\" href=\"{href}\">Older changes</a></p>",
                bar = parts::bar(),
                button = parts::button(),
                href = escape(&format!("{}{next}", r.url(""))),
            ));
        }
        Ok(Some(PanelPage::body(heading, html)))
    }
}

/// One entry as a table row. With `readable` false, the label and the
/// values are left out: the column names stay.
fn line(r: &PanelRequest<'_>, e: &Entry, readable: bool) -> String {
    let who = match &e.actor {
        Some(a) => escape(a),
        None => format!("<span class=\"{}\">nobody signed in</span>", parts::muted()),
    };
    let record = if e.label.is_empty() || !readable {
        format!("{} {}", e.table, e.row)
    } else {
        format!("{} {} ({})", e.table, e.row, e.label)
    };
    let href = format!(
        "{}?table={}&row={}",
        r.url(""),
        encode(&e.table),
        encode(&e.row)
    );
    let mut change = format!("{} via {}", e.kind.as_str(), escape(&e.source));
    if !e.fields.is_empty() {
        change.push_str("<ul>");
        for f in &e.fields {
            let f = match readable {
                true => f.clone(),
                false => Field {
                    name: f.name.clone(),
                    before: None,
                    after: None,
                },
            };
            change.push_str(&format!("<li>{}</li>", field(e.kind, &f)));
        }
        change.push_str("</ul>");
    }
    if !readable
        && e.fields
            .iter()
            .any(|f| f.before.is_some() || f.after.is_some())
    {
        change.push_str(&format!(
            " <span class=\"{}\">(values hidden)</span>",
            parts::muted()
        ));
    }
    format!(
        "<tr><td class=\"{td}\">{when}</td><td class=\"{td}\">{who}</td>\
         <td class=\"{td}\"><a class=\"{action}\" href=\"{href}\">{record}</a></td>\
         <td class=\"{td}\">{change}</td></tr>",
        td = parts::td(),
        action = parts::action(),
        when = format_time(e.at),
        href = escape(&href),
        record = escape(&record),
    )
}

/// One column: `title: Old → New`, or the name alone when the values are
/// not known.
fn field(kind: Kind, f: &Field) -> String {
    let name = escape(&f.name);
    let show = |v: &Option<String>| match v {
        Some(v) => format!("<q>{}</q>", escape(v)),
        None => "empty".to_string(),
    };
    match (kind, f.before.is_some() || f.after.is_some()) {
        (_, false) => name,
        (Kind::Created, true) => format!("{name}: {}", show(&f.after)),
        (Kind::Deleted, true) => format!("{name}: was {}", show(&f.before)),
        (Kind::Updated, true) => {
            format!("{name}: {} \u{2192} {}", show(&f.before), show(&f.after))
        }
    }
}
