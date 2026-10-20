//! The configuration, as a tab in `brocanth-admin`.
//!
//! One screen, read only, and that is the useful half of PocketBase's
//! settings page (`docs/backoffice.md` P7). The values came from files on
//! disk, so showing them answers the question an operator actually has at
//! three in the morning: *which configuration is this process running*. Not
//! "which configuration is in git", and not "which configuration the deploy
//! was supposed to write" — this one, in this process, with the file and the
//! line that decided each value.
//!
//! # Why it is read only
//!
//! There is no hot reload, on purpose: `lib.rs` sets out why. A plugin is
//! *built* from its configuration, so a value changed under a running process
//! turns every `&Config` into a lock and every `Plugin::new(config)` into a
//! lie. A form here would either not take effect, which is worse than no
//! form, or demand the lock. Restarting is the supported way to change a
//! configuration, and this tab is how you check what the restart gave you.
//!
//! # Secrets
//!
//! Every value goes through [`Redaction`] before it reaches the page, and so
//! does every `was ...` layer under it, because a password that was
//! overridden is still a password. A tab that printed a database URL with a
//! password in it would be a disclosure bug wearing a feature's clothes. The
//! rule, and the two alternatives it beat, are in [`crate::redact`].
//!
//! # What is not here
//!
//! **Plugin defaults.** The panel is constructed before `AppBuilder::build()`,
//! and a plugin's defaults only become part of the document inside
//! [`ConfigPlugin::configure`](crate::ConfigPlugin), which runs after. So this
//! screen shows the configuration **as found** — the files and the
//! environment — and says so. `Effective::dump()` is the one with the
//! defaults under it, and an application that wants them on a screen can
//! print that.
//!
//! **Any button.** Nothing here changes anything, so
//! [`PanelRequest::can_act`] is not read: a reader and an operator see the
//! same page.

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::config::{Config, Source};
use crate::redact::Redaction;
use crate::value::{Provenance, Value};

/// The panel's stylesheet, built from `style/panel.css`. Inlined by the
/// screen, because the admin mounts no asset route and a panel is in no
/// position to add one. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// The configuration as a tab.
///
/// ```ignore
/// let config = Loader::new("my_app").load()?;
/// AdminConfig::new().panel(SettingsPanel::new(config.clone()))
/// ```
///
/// It is the same document the application installed with
/// [`with_config`](crate::AppBuilderConfigExt::with_config), so the tab shows
/// what the plugins were built from and not a second reading of the files.
pub struct SettingsPanel {
    config: Config,
    key: String,
    label: String,
    redaction: Redaction,
}

impl SettingsPanel {
    /// The panel, at `settings`, labelled "Settings".
    pub fn new(config: Config) -> Self {
        Self {
            config,
            key: "settings".to_string(),
            label: "Settings".to_string(),
            redaction: Redaction::standard(),
        }
    }

    /// A different key, for an application that already has a `settings`
    /// model.
    #[must_use]
    pub fn at(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// A different tab label. "Configuration" is the other usual one.
    #[must_use]
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// A different redaction rule, for an application whose own vocabulary
    /// the standard one does not know: `.redacting(Redaction::standard().
    /// also("sas"))`.
    ///
    /// [`Redaction::none`] is accepted and is a decision somebody took on
    /// purpose. It puts secrets on a web page.
    #[must_use]
    pub fn redacting(mut self, redaction: Redaction) -> Self {
        self.redaction = redaction;
        self
    }
}

#[async_trait]
impl Panel for SettingsPanel {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        // Nothing here changes anything, so a POST is not a button this panel
        // forgot to handle: it is a request this panel does not serve.
        if r.post || !r.path.is_empty() {
            return Ok(None);
        }
        let ctx = self.settings();
        let html = render(
            &|ctx: &SettingsCtx, w: &mut rsx::Writer<'_>| {
                use rsx::Template as _;
                crate::admin_templates::settings::Settings { ctx }.render(w)
            },
            &ctx,
        )?;
        Ok(Some(PanelPage::body("Settings", html)))
    }
}

impl SettingsPanel {
    fn settings(&self) -> SettingsCtx {
        let config = &self.config;
        let mut keys = 0usize;
        let mut hidden = 0usize;

        let sections = config
            .root()
            .as_table()
            .map(|table| {
                table
                    .iter()
                    .map(|(name, section)| {
                        let rows: Vec<KeyRow> = section
                            .leaves()
                            .into_iter()
                            .map(|(leaf, value)| {
                                let path = match leaf.is_empty() {
                                    true => name.clone(),
                                    false => format!("{name}.{leaf}"),
                                };
                                let row = self.key_row(path, value);
                                keys += 1;
                                if row.redacted {
                                    hidden += 1;
                                }
                                row
                            })
                            .collect();
                        SectionCard {
                            name: name.clone(),
                            rows,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        SettingsCtx {
            app: config.app().as_str().to_string(),
            env: config.env().to_string(),
            sources: self.source_rows(),
            sections,
            summary: format!(
                "{keys} {}",
                match keys {
                    1 => "key",
                    _ => "keys",
                }
            ),
            hidden: match hidden {
                0 => None,
                1 => Some("1 value is hidden.".to_string()),
                n => Some(format!("{n} values are hidden.")),
            },
        }
    }

    fn key_row(&self, path: String, value: &Value) -> KeyRow {
        let shown = self.redaction.show(&path, value);
        KeyRow {
            redacted: shown.is_redacted(),
            reason: shown.reason.map(|r| r.as_str().to_string()),
            value: shown.text,
            origin: self.origin(value.provenance()),
            // The `was` lines are the layering, which is the point of the
            // screen: "it is 8080, it was 3100 in the YAML" is one glance.
            // They are redacted too, because an overridden password is still
            // a password.
            history: value
                .shadowed()
                .iter()
                .map(|earlier| Layer {
                    value: self.redaction.show(&path, earlier).text,
                    origin: self.origin(earlier.provenance()),
                })
                .collect(),
            path,
        }
    }

    fn source_rows(&self) -> Vec<SourceRow> {
        let mut rows = Vec::new();
        let vars = self.config.env_vars();
        if !vars.is_empty() {
            rows.push(SourceRow {
                rank: (rows.len() + 1).to_string(),
                origin: "environment".to_string(),
                what: format!("{}_*", self.config.app().as_str().to_ascii_uppercase()),
                detail: plural(vars.len(), "variable", "variables"),
                variables: vars.to_vec(),
            });
        }
        for source in self.config.sources() {
            rows.push(SourceRow {
                rank: (rows.len() + 1).to_string(),
                origin: source.origin.to_string(),
                what: source.path.display().to_string(),
                detail: source.format.to_string(),
                variables: Vec::new(),
            });
        }
        rows
    }

    /// A file origin as `[2] production.yaml:5`; everything else as itself.
    ///
    /// The same shape as [`Dump`](crate::Dump)'s, and for the same reason: an
    /// absolute deployment path is often eighty characters, and the number
    /// carries it, printed once, in the sources table at the top.
    fn origin(&self, from: &Provenance) -> String {
        match from {
            Provenance::File { file, line, .. } => {
                let name = file.path.file_name().map_or_else(
                    || file.path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                match self.source_number(file) {
                    Some(n) => format!("[{n}] {name}:{line}"),
                    None => format!("{name}:{line}"),
                }
            }
            other => other.to_string(),
        }
    }

    fn source_number(&self, file: &Source) -> Option<usize> {
        let offset = usize::from(!self.config.env_vars().is_empty());
        self.config
            .sources()
            .iter()
            .position(|s| s.path == file.path)
            .map(|i| i + 1 + offset)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        _ => format!("{n} {many}"),
    }
}

// ---------------------------------------------------------------------------
// What the template is given
// ---------------------------------------------------------------------------
//
// Every field is already a string the screen prints. No formatting happens in
// the template, and no redaction happens there either: a value reaches a
// template only after `Redaction` has seen it, so a replaced template cannot
// reintroduce the disclosure.

/// One of the layered sources, highest precedence first.
pub struct SourceRow {
    /// Its number, which the origin column cites.
    pub rank: String,
    /// "environment", "env var", "system dir", "working dir", "config dir".
    pub origin: String,
    /// The path, or the environment prefix.
    pub what: String,
    /// The parser that read it, or how many variables it contributed.
    pub detail: String,
    /// The names of the variables, for the environment row. Names only: a
    /// variable's *value* is a key in the table below and is redacted there.
    pub variables: Vec<String>,
}

/// One layer a value beat.
pub struct Layer {
    /// What it was.
    pub value: String,
    /// Which file, line or variable set it.
    pub origin: String,
}

/// One key.
pub struct KeyRow {
    /// The dotted path, in full, so a line can be grepped and pasted.
    pub path: String,
    /// The value, already redacted if it needed to be.
    pub value: String,
    /// Whether something was taken out.
    pub redacted: bool,
    /// Which rule took it out.
    pub reason: Option<String>,
    /// Where the winning value came from.
    pub origin: String,
    /// Every layer it beat, nearest first.
    pub history: Vec<Layer>,
}

/// One section.
pub struct SectionCard {
    /// The section name, as `[name]`.
    pub name: String,
    /// Its keys, in name order.
    pub rows: Vec<KeyRow>,
}

/// The screen.
pub struct SettingsCtx {
    /// The application's canonical name.
    pub app: String,
    /// The environment it was loaded for.
    pub env: String,
    /// The layered sources, highest precedence first.
    pub sources: Vec<SourceRow>,
    /// One card per section.
    pub sections: Vec<SectionCard>,
    /// "27 keys".
    pub summary: String,
    /// How many values were hidden, when any were.
    pub hidden: Option<String>,
}
