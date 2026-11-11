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

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::parts;

use crate::flags::Flags;
use crate::plugin::FlagsPlugin;
use crate::rule::{valid_key, Flag, Rule};

/// The flags tab.
pub struct FlagsPanel {
    source: Source,
    declared: &'static [Flag],
}

enum Source {
    Flags(Flags),
    Installed,
}

impl FlagsPanel {
    /// A tab over this copy, listing `declared`.
    pub fn new(flags: Flags, declared: &'static [Flag]) -> FlagsPanel {
        FlagsPanel {
            source: Source::Flags(flags),
            declared,
        }
    }

    /// A tab over [`FlagsPlugin::installed`], resolved when it renders: for
    /// a generated admin, which mounts before the plugin is built.
    pub fn installed(declared: &'static [Flag]) -> FlagsPanel {
        FlagsPanel {
            source: Source::Installed,
            declared,
        }
    }

    fn flags(&self) -> Option<Flags> {
        match &self.source {
            Source::Flags(flags) => Some(flags.clone()),
            Source::Installed => FlagsPlugin::installed(),
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
        "flags"
    }

    fn label(&self) -> String {
        "Feature flags".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        let Some(flags) = self.flags() else {
            return Ok(Some(PanelPage::body(
                "Feature flags",
                format!(
                    "<p class=\"{}\">The flags plugin is not registered in this application.</p>",
                    parts::muted()
                ),
            )));
        };
        if r.path.is_empty() {
            return Ok((!r.post).then(|| self.list(&flags, &r)));
        }
        let key = r.path;
        // A declared flag, or one with a stored rule. Any other key is a
        // 404, so the form cannot create a row for a flag no code reads.
        if !valid_key(key) || (self.description(key).is_none() && flags.rule(key).is_none()) {
            return Ok(None);
        }
        if !r.post {
            let rule = flags.rule(key).unwrap_or_default();
            return Ok(Some(self.form(&r, key, &rule, None)));
        }
        if r.submission.has("remove") {
            return Ok(Some(match flags.remove(key).await {
                Ok(()) => PanelPage::redirect(r.url("")),
                Err(e) => self.form(&r, key, &Rule::default(), Some(e.to_string())),
            }));
        }
        let rule = match read(&r) {
            Ok(rule) => rule,
            Err(message) => {
                let rule = flags.rule(key).unwrap_or_default();
                return Ok(Some(self.form(&r, key, &rule, Some(message))));
            }
        };
        Ok(Some(match flags.set(key, rule.clone()).await {
            Ok(()) => PanelPage::redirect(r.url("")),
            Err(e) => self.form(&r, key, &rule, Some(e.to_string())),
        }))
    }
}

impl FlagsPanel {
    fn list(&self, flags: &Flags, r: &PanelRequest<'_>) -> PanelPage {
        let stored = flags.rules();
        let mut rows = String::new();
        for flag in self.declared {
            let rule = flags.rule(flag.key()).unwrap_or_default();
            rows.push_str(&row(r, flag.key(), flag.description(), &rule));
        }
        // A stored rule no code declares: a flag that was removed from the
        // code, or a typo. Listed so somebody can remove the row.
        for (key, rule) in &stored {
            if self.description(key).is_none() {
                rows.push_str(&row(r, key, "Not declared in the code.", rule));
            }
        }
        let mut html = String::new();
        if let Some(e) = flags.last_error() {
            html.push_str(&format!(
                "<p class=\"{}\">The last read of the table failed: {}. The rules below are the \
                 last ones read.</p>",
                parts::muted(),
                escape(&e)
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
