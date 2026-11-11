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

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::journal::{AdminWrite, Journal, Written};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::{parts, AdminConfig};

use crate::entry::{format_time, Entry, Field, Kind};
use crate::plugin::AuditPlugin;
use crate::store::Query;
use crate::trail::Trail;

/// The panel's key: its URL segment and its policy subject.
pub const KEY: &str = "history";

/// Entries on one page.
const PAGE: usize = 50;

/// Adds the history tab, the journal and the "History" link to `config`,
/// over [`AuditPlugin::installed`].
///
/// The admin has one journal, so this replaces one set before.
///
/// ```
/// let config = brocanth_audit::admin::install(brocanth_admin::AdminConfig::new());
/// # let _ = config;
/// ```
pub fn install(config: AdminConfig) -> AdminConfig {
    install_over(config, None)
}

/// [`install`] over this trail, for a test or an application that builds
/// the trail itself.
pub fn install_with(config: AdminConfig, trail: Trail) -> AdminConfig {
    install_over(config, Some(trail))
}

fn install_over(config: AdminConfig, trail: Option<Trail>) -> AdminConfig {
    let (panel, journal) = match trail {
        Some(trail) => (AuditPanel::new(trail.clone()), AuditJournal::new(trail)),
        None => (AuditPanel::installed(), AuditJournal::installed()),
    };
    config
        .panel(panel)
        .journal(journal)
        .record_link("History", |r| {
            Some(format!(
                "{}/{KEY}?table={}&row={}",
                r.base_path,
                encode(r.table),
                encode(r.key)
            ))
        })
}

/// Where the trail comes from.
#[derive(Clone)]
enum Source {
    Trail(Trail),
    Installed,
}

impl Source {
    fn trail(&self) -> Option<Trail> {
        match self {
            Source::Trail(t) => Some(t.clone()),
            Source::Installed => AuditPlugin::installed(),
        }
    }
}

/// Records every admin write in the trail.
pub struct AuditJournal {
    source: Source,
}

impl AuditJournal {
    /// A journal over this trail.
    pub fn new(trail: Trail) -> AuditJournal {
        AuditJournal {
            source: Source::Trail(trail),
        }
    }

    /// A journal over [`AuditPlugin::installed`], resolved at each write.
    /// A write made while no plugin is installed is not recorded.
    pub fn installed() -> AuditJournal {
        AuditJournal {
            source: Source::Installed,
        }
    }
}

impl Journal for AuditJournal {
    fn record(&self, write: &AdminWrite<'_>) {
        let Some(trail) = self.source.trail() else {
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
    source: Source,
}

impl AuditPanel {
    /// A tab over this trail.
    pub fn new(trail: Trail) -> AuditPanel {
        AuditPanel {
            source: Source::Trail(trail),
        }
    }

    /// A tab over [`AuditPlugin::installed`], resolved when it renders:
    /// for a generated admin, which mounts before the plugin is built.
    pub fn installed() -> AuditPanel {
        AuditPanel {
            source: Source::Installed,
        }
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
        let Some(trail) = self.source.trail() else {
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
        if entries.is_empty() {
            html.push_str(&format!(
                "<div class=\"{}\"><div class=\"{}\">No changes recorded.</div></div>",
                parts::card(),
                parts::empty()
            ));
            return Ok(Some(PanelPage::body(heading, html)));
        }
        let rows: String = entries.iter().map(|e| line(&r, e)).collect();
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

/// One entry as a table row.
fn line(r: &PanelRequest<'_>, e: &Entry) -> String {
    let who = match &e.actor {
        Some(a) => escape(a),
        None => format!("<span class=\"{}\">nobody signed in</span>", parts::muted()),
    };
    let record = if e.label.is_empty() {
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
            change.push_str(&format!("<li>{}</li>", field(e.kind, f)));
        }
        change.push_str("</ul>");
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

/// Percent-encodes a query value.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
