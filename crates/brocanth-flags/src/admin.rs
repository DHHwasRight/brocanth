//! The flags, as a tab in `brocanth-admin`.
//!
//! Two screens: the list of every declared flag and every stored rule, and
//! one flag's form. The form is a plain POST, so it works with no script;
//! the generated admin stamps its CSRF field into it like every admin form.
//!
//! Locked like every tab: reading is `Action::ViewPanel` on `flags`, and
//! saving is `Action::ActOnPanel`, which the admin asks before this panel
//! runs. The shipped `DenyAll` refuses both until the application says who
//! may.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::remote::{PanelServe, PanelWire};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;
use brocanth_admin::policy::{Level, Principal};
use serde::{Deserialize, Serialize};

use crate::flags::Flags;
use crate::rule::{valid_key, Flag, Rule};

/// The tab's key: its RBAC resource, and its name on the backoffice wire.
pub const TAB: &str = "flags";

/// What the flags tab reads and writes (docs/backoffice.md §7.6, P1).
///
/// [`Flags`] implements it in the process that holds the copy.
/// [`RemoteFlags`] implements it over the backoffice wire, and
/// [`FlagsServe`] answers it there. `who` is the request's principal: the
/// copy ignores it, and the wire turns it into the claim the data side
/// checks the grant of.
#[async_trait]
pub trait FlagsData: Send + Sync + 'static {
    /// Every stored rule, and what the last refresh failed with.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn read(&self, who: &Principal<'_>) -> Result<FlagsView, String>;

    /// Stores `rule` for `key`.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn set(&self, who: &Principal<'_>, key: &str, rule: Rule) -> Result<(), String>;

    /// Removes the rule for `key`, so the flag is off.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn remove(&self, who: &Principal<'_>, key: &str) -> Result<(), String>;
}

/// The stored rules, as the tab shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagsView {
    /// Every stored rule, by key.
    pub rules: Vec<(String, Rule)>,
    /// What the last refresh of the copy failed with, while it fails.
    pub last_error: Option<String>,
}

impl FlagsView {
    /// The stored rule for `key`.
    pub fn rule(&self, key: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, rule)| rule)
    }
}

/// One call of the flags contract, as the wire carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlagsCall {
    /// [`FlagsData::read`]. Needs `flags.read`.
    Read,
    /// [`FlagsData::set`]. Needs `flags.edit`.
    Set {
        /// The flag.
        key: String,
        /// Its new rule.
        rule: Rule,
    },
    /// [`FlagsData::remove`]. Needs `flags.edit`.
    Remove {
        /// The flag.
        key: String,
    },
}

impl FlagsCall {
    /// The level the call needs on [`TAB`].
    pub fn level(&self) -> Level {
        match self {
            FlagsCall::Read => Level::Read,
            FlagsCall::Set { .. } | FlagsCall::Remove { .. } => Level::Edit,
        }
    }
}

/// What a [`FlagsCall`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlagsAnswer {
    /// To `Read`.
    View(FlagsView),
    /// To `Set` and `Remove`.
    Done,
}

#[async_trait]
impl FlagsData for Flags {
    async fn read(&self, _who: &Principal<'_>) -> Result<FlagsView, String> {
        Ok(FlagsView {
            rules: self.rules(),
            last_error: self.last_error(),
        })
    }

    async fn set(&self, _who: &Principal<'_>, key: &str, rule: Rule) -> Result<(), String> {
        Flags::set(self, key, rule).await.map_err(|e| e.to_string())
    }

    async fn remove(&self, _who: &Principal<'_>, key: &str) -> Result<(), String> {
        Flags::remove(self, key).await.map_err(|e| e.to_string())
    }
}

/// The data side of the flags tab on the backoffice wire: decodes a
/// [`FlagsCall`] and runs it on a [`FlagsData`].
pub struct FlagsServe(Arc<dyn FlagsData>);

impl FlagsServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn FlagsData>) -> FlagsServe {
        FlagsServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for FlagsServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FlagsServe")
    }
}

fn decode(call: &str) -> Result<FlagsCall, String> {
    serde_json::from_str(call).map_err(|e| format!("a flags call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for FlagsServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|call| call.level())
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let answer = match decode(call)? {
            FlagsCall::Read => FlagsAnswer::View(self.0.read(who).await?),
            FlagsCall::Set { key, rule } => {
                self.0.set(who, &key, rule).await?;
                FlagsAnswer::Done
            }
            FlagsCall::Remove { key } => {
                self.0.remove(who, &key).await?;
                FlagsAnswer::Done
            }
        };
        serde_json::to_string(&answer).map_err(|e| format!("a flags answer did not encode: {e}"))
    }
}

/// The flags tab's data in another process, through the backoffice wire.
pub struct RemoteFlags(Arc<dyn PanelWire>);

impl RemoteFlags {
    /// The flags behind `wire`.
    pub fn new(wire: Arc<dyn PanelWire>) -> RemoteFlags {
        RemoteFlags(wire)
    }

    async fn ask(&self, who: &Principal<'_>, call: &FlagsCall) -> Result<FlagsAnswer, String> {
        let text =
            serde_json::to_string(call).map_err(|e| format!("a flags call did not encode: {e}"))?;
        let answer = self.0.send(who, TAB, text).await?;
        serde_json::from_str(&answer).map_err(|e| format!("a flags answer did not decode: {e}"))
    }

    async fn done(&self, who: &Principal<'_>, call: FlagsCall) -> Result<(), String> {
        match self.ask(who, &call).await? {
            FlagsAnswer::Done => Ok(()),
            FlagsAnswer::View(_) => Err("the data side answered a write with the rules".into()),
        }
    }
}

impl std::fmt::Debug for RemoteFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteFlags")
    }
}

#[async_trait]
impl FlagsData for RemoteFlags {
    async fn read(&self, who: &Principal<'_>) -> Result<FlagsView, String> {
        match self.ask(who, &FlagsCall::Read).await? {
            FlagsAnswer::View(view) => Ok(view),
            FlagsAnswer::Done => Err("the data side answered a read with no rules".into()),
        }
    }

    async fn set(&self, who: &Principal<'_>, key: &str, rule: Rule) -> Result<(), String> {
        let key = key.to_string();
        self.done(who, FlagsCall::Set { key, rule }).await
    }

    async fn remove(&self, who: &Principal<'_>, key: &str) -> Result<(), String> {
        let key = key.to_string();
        self.done(who, FlagsCall::Remove { key }).await
    }
}

/// The copy in a cell, as the wire's data side reads it: empty until the
/// first request that carries the [`Flags`].
struct LateFlags(Late<Flags>);

impl LateFlags {
    fn flags(&self) -> Result<&Flags, String> {
        self.0.get().ok_or_else(|| {
            "the flags plugin is not registered in this application\nhelp: register it, \
             which puts the flags on the request context"
                .to_string()
        })
    }
}

#[async_trait]
impl FlagsData for LateFlags {
    async fn read(&self, who: &Principal<'_>) -> Result<FlagsView, String> {
        FlagsData::read(self.flags()?, who).await
    }

    async fn set(&self, who: &Principal<'_>, key: &str, rule: Rule) -> Result<(), String> {
        FlagsData::set(self.flags()?, who, key, rule).await
    }

    async fn remove(&self, who: &Principal<'_>, key: &str) -> Result<(), String> {
        FlagsData::remove(self.flags()?, who, key).await
    }
}

/// Where the tab reads its flags.
enum Source {
    /// The copy, which arrives with the first request.
    Copy(Late<Flags>),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn FlagsData>),
}

/// The flags tab.
pub struct FlagsPanel {
    source: Source,
    declared: &'static [Flag],
}

impl FlagsPanel {
    /// A tab over this copy, listing `declared`.
    pub fn new(flags: Flags, declared: &'static [Flag]) -> FlagsPanel {
        FlagsPanel::late(Late::ready(flags), declared)
    }

    /// A tab over a copy that arrives later, in `cell`: for a generated
    /// admin, which mounts before the plugin is built. The admin fills the
    /// cell from the first request that carries the [`Flags`].
    pub fn late(cell: Late<Flags>, declared: &'static [Flag]) -> FlagsPanel {
        FlagsPanel {
            source: Source::Copy(cell),
            declared,
        }
    }

    /// A tab over any [`FlagsData`]: [`RemoteFlags`] in the backoffice
    /// binary, which reads the flags through the wire (§7.6).
    pub fn over(data: Arc<dyn FlagsData>, declared: &'static [Flag]) -> FlagsPanel {
        FlagsPanel {
            source: Source::Data(data),
            declared,
        }
    }

    fn data(&self) -> Option<Arc<dyn FlagsData>> {
        match &self.source {
            Source::Copy(cell) => cell
                .get()
                .cloned()
                .map(|flags| Arc::new(flags) as Arc<dyn FlagsData>),
            Source::Data(data) => Some(Arc::clone(data)),
        }
    }

    fn description(&self, key: &str) -> Option<&'static str> {
        self.declared
            .iter()
            .find(|f| f.key() == key)
            .map(|f| f.description())
    }
}

#[async_trait]
impl Panel for FlagsPanel {
    fn key(&self) -> &str {
        TAB
    }

    fn label(&self) -> String {
        "Feature flags".to_string()
    }

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Copy(cell) => Some(FlagsServe::new(Arc::new(LateFlags(cell.clone()))).shared()),
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        Some(Arc::new(FlagsPanel::over(
            Arc::new(RemoteFlags::new(wire)),
            self.declared,
        )))
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(data) = self.data() else {
            return Ok(Some(PanelPage::body(
                "Feature flags",
                format!(
                    "<p class=\"{}\">The flags plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        let who = r.principal;
        let view = match data.read(&who).await {
            Ok(view) => view,
            Err(e) => {
                return Ok(Some(PanelPage::body(
                    "Feature flags",
                    format!(
                        "<p role=\"alert\" class=\"{}\">The flags could not be read: {}</p>",
                        parts::muted(),
                        escape(&e)
                    ),
                )))
            }
        };
        if r.path.is_empty() {
            return Ok((!r.post).then(|| self.list(&view, &r)));
        }
        let key = r.path;
        // A declared flag, or one with a stored rule. Any other key is a
        // 404, so the form cannot create a row for a flag no code reads.
        if !valid_key(key) || (self.description(key).is_none() && view.rule(key).is_none()) {
            return Ok(None);
        }
        let stored = || view.rule(key).cloned().unwrap_or_default();
        if !r.post {
            return Ok(Some(self.form(&r, key, &stored(), None)));
        }
        if r.submission.has("remove") {
            return Ok(Some(match data.remove(&who, key).await {
                Ok(()) => PanelPage::redirect(r.url("")),
                // Still stored: show the rule that is still there.
                Err(e) => self.form(&r, key, &stored(), Some(e)),
            }));
        }
        let rule = match read(&r) {
            Ok(rule) => rule,
            Err(message) => return Ok(Some(self.form(&r, key, &stored(), Some(message)))),
        };
        Ok(Some(match data.set(&who, key, rule.clone()).await {
            Ok(()) => PanelPage::redirect(r.url("")),
            Err(e) => self.form(&r, key, &rule, Some(e)),
        }))
    }
}

impl FlagsPanel {
    fn list(&self, view: &FlagsView, r: &PanelRequest<'_>) -> PanelPage {
        let mut rows = String::new();
        for flag in self.declared {
            let rule = view.rule(flag.key()).cloned().unwrap_or_default();
            rows.push_str(&row(r, flag.key(), flag.description(), &rule));
        }
        // A stored rule no code declares: a flag that was removed from the
        // code, or a typo. Listed so somebody can remove the row.
        for (key, rule) in &view.rules {
            if self.description(key).is_none() {
                rows.push_str(&row(r, key, "Not declared in the code.", rule));
            }
        }
        let mut html = String::new();
        if let Some(e) = &view.last_error {
            html.push_str(&format!(
                "<p class=\"{}\">The last read of the table failed: {}. The rules below are the \
                 last ones read.</p>",
                parts::muted(),
                escape(e)
            ));
        }
        if rows.is_empty() {
            html.push_str(&format!(
                "<div class=\"{}\"><div class=\"{}\">No flags yet. Declare one in \
                 <code>src/plugins/flags/mod.rs</code>.</div></div>",
                parts::card(),
                parts::empty()
            ));
        } else {
            html.push_str(&format!(
                "<div class=\"{card}\"><table class=\"{table}\"><thead><tr>\
                 <th class=\"{th}\">Flag</th><th class=\"{th}\">What it changes</th>\
                 <th class=\"{th}\">State</th><th class=\"{th}\" aria-label=\"Actions\"></th>\
                 </tr></thead><tbody>{rows}</tbody></table></div>",
                card = parts::card(),
                table = parts::table(),
                th = parts::th(),
            ));
        }
        PanelPage::body("Feature flags", html)
    }

    fn form(
        &self,
        r: &PanelRequest<'_>,
        key: &str,
        rule: &Rule,
        error: Option<String>,
    ) -> PanelPage {
        let description = self
            .description(key)
            .unwrap_or("Not declared in the code. Remove the row, or declare the flag.");
        let mut html = format!(
            "<p class=\"{crumbs}\"><a class=\"{action}\" href=\"{back}\">Feature flags</a>\
             <span>/</span><span>{key}</span></p><p class=\"{muted}\">{description}</p>",
            crumbs = parts::crumbs(),
            action = parts::action(),
            back = escape(&r.url("")),
            muted = parts::muted(),
            key = escape(key),
            description = escape(description),
        );
        if let Some(error) = error {
            html.push_str(&format!(
                "<p role=\"alert\" class=\"{}\">{}</p>",
                parts::muted(),
                escape(&error)
            ));
        }
        let disabled = if r.can_act { "" } else { " disabled" };
        html.push_str(&format!(
            "<form method=\"post\" action=\"{action}\" class=\"{card}\">\
             <p><label><input type=\"checkbox\" name=\"on\" value=\"1\"{checked}{disabled}> \
             On for everyone</label></p>\
             <p><label>Percentage of signed-in users \
             <input class=\"{control}\" type=\"number\" name=\"percent\" min=\"0\" max=\"100\" \
             value=\"{percent}\"{disabled}></label></p>\
             <p><label>Users, one id per line<br>\
             <textarea class=\"{control}\" name=\"actors\" rows=\"4\"{disabled}>{actors}</textarea>\
             </label></p>\
             <p><label>Groups, one per line<br>\
             <textarea class=\"{control}\" name=\"groups\" rows=\"3\"{disabled}>{groups}</textarea>\
             </label></p>",
            action = escape(&r.url(key)),
            card = parts::card(),
            control = parts::control(),
            checked = if rule.on { " checked" } else { "" },
            percent = rule.percent,
            actors = escape(&rule.actors.join("\n")),
            groups = escape(&rule.groups.join("\n")),
        ));
        if r.can_act {
            html.push_str(&format!(
                "<p class=\"{bar}\"><button class=\"{primary}\" type=\"submit\">Save</button> \
                 <button class=\"{danger}\" type=\"submit\" name=\"remove\" value=\"1\">\
                 Remove the rule</button> <a class=\"{button}\" href=\"{back}\">Cancel</a></p>",
                bar = parts::bar(),
                primary = parts::button_primary(),
                danger = parts::button_danger(),
                button = parts::button(),
                back = escape(&r.url("")),
            ));
        }
        html.push_str("</form>");
        PanelPage::body(format!("Flag {key}"), html)
    }
}

fn row(r: &PanelRequest<'_>, key: &str, description: &str, rule: &Rule) -> String {
    format!(
        "<tr><td class=\"{td}\"><code>{key}</code></td><td class=\"{td}\">{description}</td>\
         <td class=\"{td}\">{state}</td><td class=\"{td}\">\
         <a class=\"{action}\" href=\"{href}\">Edit</a></td></tr>",
        td = parts::td(),
        action = parts::action(),
        key = escape(key),
        description = escape(description),
        state = escape(&rule.summary()),
        href = escape(&r.url(key)),
    )
}

/// The rule a POST describes.
fn read(r: &PanelRequest<'_>) -> Result<Rule, String> {
    let form = r.submission;
    let percent = match form.get("percent").map(str::trim).unwrap_or("") {
        "" => 0,
        text => text
            .parse::<u8>()
            .ok()
            .filter(|p| *p <= 100)
            .ok_or_else(|| {
                format!("The percentage must be a whole number from 0 to 100, not `{text}`.")
            })?,
    };
    let lines = |name: &str| -> Vec<String> {
        let mut out: Vec<String> = form
            .get(name)
            .unwrap_or("")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        out.sort();
        out.dedup();
        out
    };
    Ok(Rule {
        on: form.get("on").is_some_and(|v| !v.is_empty()),
        percent,
        actors: lines("actors"),
        groups: lines("groups"),
    })
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
