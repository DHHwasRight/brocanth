//! The effective configuration, printed for a human at three in the morning.
//!
//! This is what makes a plugin's defaults legitimate under PLAN.md §8. The
//! headline feature of this crate is not that a plugin can supply defaults —
//! any configuration library does that. It is that **the application can print
//! every resolved value with the origin of every key**, so "the framework
//! decided for you and you cannot see where" does not apply: you can see where.
//!
//! # What the layout is for
//!
//! Someone is reading this in a terminal, under pressure, looking for one
//! wrong value. So:
//!
//! * **One line per key**, never wrapped, never a nested indent that has to be
//!   counted. The key's full path is on its own line, so the line can be
//!   grepped and pasted into a chat window and still mean something.
//! * **A `*` in the first column** for every value that is not the plugin's
//!   default. The eye scans one column down the page and finds every decision a
//!   human made. Nothing else uses that column.
//! * **The origin in the right-hand column, always**, in a form that can be
//!   acted on: a file and a line to open, a `$VARIABLE` to echo, or
//!   `default (logger)` to go and read that plugin. A file is written
//!   `[2] production.yaml:5` — its number in the source table at the top, then
//!   its name and its line. An absolute deployment path is often eighty
//!   characters, and a dump whose lines wrap is a dump nobody reads; the
//!   number carries the full path, printed once, at the top.
//! * **`was ...` for an overridden value.** The question after "what is the
//!   port" is "what was it before", and the merge already knows.
//! * **No colour.** A dump goes into a ticket, a log line and a pipe at least
//!   as often as it goes to a terminal, and an escape sequence in any of those
//!   is noise.

use std::fmt;

use crate::config::Config;
use crate::plugin::SectionClaim;
use crate::value::{Provenance, Value};
use crate::CORE_SECTIONS;

/// A printable rendering of a resolved configuration.
///
/// Built by [`Config::dump`] or [`Effective::dump`](crate::Effective::dump) and
/// printed with `{}`.
pub struct Dump<'a> {
    pub(crate) config: &'a Config,
    pub(crate) claims: &'a [SectionClaim],
    pub(crate) history: bool,
}

/// What number a file has in the source table, so an origin can cite it.
fn source_number(config: &Config, path: &std::path::Path) -> Option<usize> {
    let offset = usize::from(!config.env_vars().is_empty());
    config
        .sources()
        .iter()
        .position(|s| s.path == path)
        .map(|i| i + 1 + offset)
}

impl Dump<'_> {
    /// Leaves out the `was ...` line under an overridden value.
    ///
    /// On by default, because it answers the second question an incident asks.
    #[must_use]
    pub fn without_history(mut self) -> Self {
        self.history = false;
        self
    }
}

const KEY_COLUMN: usize = 30;
const VALUE_COLUMN: usize = 26;

impl fmt::Display for Dump<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} · {}", self.config.app(), self.config.env())?;
        writeln!(f)?;
        self.sources(f)?;
        writeln!(f)?;

        let root = self.config.root();
        let sections: Vec<(&String, &Value)> = root
            .as_table()
            .map(|t| t.iter().collect())
            .unwrap_or_default();

        let mut total = 0usize;
        let mut defaulted = 0usize;

        for (name, section) in sections {
            writeln!(f, "  [{name}]{}", self.claim_note(name))?;
            for (leaf, value) in section.leaves() {
                let path = if leaf.is_empty() {
                    name.clone()
                } else {
                    format!("{name}.{leaf}")
                };
                total += 1;
                let is_default = value.provenance().is_default();
                if is_default {
                    defaulted += 1;
                }
                let marker = if is_default { ' ' } else { '*' };
                let shown = value.display();
                writeln!(
                    f,
                    "  {marker} {path:<KEY_COLUMN$} = {shown:<VALUE_COLUMN$}  {}",
                    self.origin(value.provenance())
                )?;
                if self.history {
                    self.history_lines(f, value)?;
                }
            }
            writeln!(f)?;
        }

        let overridden = total - defaulted;
        writeln!(
            f,
            "  {total} keys · {defaulted} at their default · {overridden} overridden (marked *)"
        )
    }
}

impl Dump<'_> {
    fn sources(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "  sources, highest precedence first")?;
        let mut n = 0;

        let vars = self.config.env_vars();
        if !vars.is_empty() {
            n += 1;
            writeln!(
                f,
                "  {n:>2}  environment    {:<36}  {} variable{}",
                format!("{}_*", self.config.app().as_str().to_ascii_uppercase()),
                vars.len(),
                if vars.len() == 1 { "" } else { "s" }
            )?;
        }

        for source in self.config.sources() {
            n += 1;
            writeln!(
                f,
                "  {n:>2}  {:<13}  {:<36}  {}",
                source.origin.to_string(),
                source.path.display().to_string(),
                source.format
            )?;
        }

        if !self.claims.is_empty() {
            n += 1;
            let names: Vec<&str> = self.claims.iter().map(|c| c.section.as_str()).collect();
            writeln!(
                f,
                "  {n:>2}  defaults       {:<36}  {}",
                format!(
                    "{} plugin{}",
                    self.claims.len(),
                    if self.claims.len() == 1 { "" } else { "s" }
                ),
                names.join(", ")
            )?;
        }

        if n == 0 {
            writeln!(f, "      (none — every value below is a default)")?;
        }
        Ok(())
    }

    /// One line per layer this value beat, in the same columns as the key
    /// above it, so the value column reads `8080`, `was 3000`, `was 3000`
    /// straight down and the origin column says who set each one.
    fn history_lines(&self, f: &mut fmt::Formatter<'_>, value: &Value) -> fmt::Result {
        for earlier in value.shadowed() {
            let was = format!("was {}", earlier.display());
            writeln!(
                f,
                "    {:<KEY_COLUMN$}   {was:<VALUE_COLUMN$}  {}",
                "",
                self.origin(earlier.provenance())
            )?;
        }
        Ok(())
    }

    /// A file origin as `[2] production.yaml:5`; everything else as itself.
    fn origin(&self, from: &Provenance) -> String {
        match from {
            Provenance::File { file, line, .. } => {
                let name = file.path.file_name().map_or_else(
                    || file.path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                match source_number(self.config, &file.path) {
                    Some(n) => format!("[{n}] {name}:{line}"),
                    None => format!("{name}:{line}"),
                }
            }
            other => other.to_string(),
        }
    }

    fn claim_note(&self, section: &str) -> String {
        if let Some(claim) = self.claims.iter().find(|c| c.section == section) {
            return format!("  claimed by `{}`", claim.plugin);
        }
        if CORE_SECTIONS.contains(&section) {
            return "  the framework's own".to_string();
        }
        "  no defaults declared — keys unchecked".to_string()
    }
}

#[cfg(all(test, feature = "toml"))]
mod tests {
    use crate::{AppName, Config};

    #[test]
    fn the_dump_marks_what_is_not_a_default_and_counts_the_rest() {
        let config = Config::from_toml(
            AppName::new("my_app"),
            "production",
            "[server]\nhost = \"127.0.0.1\"\nport = 8080\n",
        )
        .unwrap();

        let text = config.dump().to_string();
        assert!(text.contains("my_app · production"), "{text}");
        assert!(text.contains("[server]"), "{text}");
        assert!(text.contains("* server.port"), "{text}");
        assert!(
            text.contains("2 keys · 0 at their default · 2 overridden"),
            "{text}"
        );
    }
}
