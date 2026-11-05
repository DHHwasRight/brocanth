#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! # Design notes
//!
//! **A builder and not a template.** robots.txt is small, but its grammar
//! has two mistakes a hand-written file makes often: a rule path that does
//! not start with `/` (crawlers ignore it, so the page it was meant to hide
//! is indexed), and a relative `Sitemap:` URL (ignored too). [`Robots::render`]
//! refuses both, so the generated plugin fails at startup instead of serving
//! a file that says something other than what was meant.
//!
//! **Closed is a value, not a rule list.** [`Robots::closed`] is the whole
//! file `User-agent: *` / `Disallow: /`. A staging server answers it, and
//! nothing else is written, because one `Allow:` left over from the
//! production list would open that path on staging.
//!
//! **No dependencies.** The format is RFC 9309: groups of `User-agent:`
//! lines followed by `Allow:` and `Disallow:` rules, and `Sitemap:` lines
//! outside any group. Writing it is a loop; checking it is a few `starts_with`.

use std::fmt;

#[cfg(feature = "scaffold")]
pub mod scaffold;
#[cfg(feature = "scaffold")]
mod scaffold_sitemap;
pub mod sitemap;

pub use sitemap::{Sitemap, SitemapError};

/// One rule inside a group.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    Allow(String),
    Disallow(String),
}

/// A group: the crawlers it names, and what they may read.
///
/// A group with no rules allows everything. It renders as an empty
/// `Disallow:`, which is how RFC 9309 spells "nothing is disallowed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    agents: Vec<String>,
    rules: Vec<Rule>,
}

impl Group {
    /// A group for one crawler, by its product token: `*`, `Googlebot`,
    /// `GPTBot`.
    pub fn new(agent: impl Into<String>) -> Self {
        Self {
            agents: vec![agent.into()],
            rules: Vec::new(),
        }
    }

    /// One more crawler that follows the same rules.
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agents.push(agent.into());
        self
    }

    /// The crawlers may read `path`, even under a disallowed prefix.
    pub fn allow(mut self, path: impl Into<String>) -> Self {
        push_once(&mut self.rules, Rule::Allow(path.into()));
        self
    }

    /// The crawlers may not read `path` or anything under it.
    pub fn disallow(mut self, path: impl Into<String>) -> Self {
        push_once(&mut self.rules, Rule::Disallow(path.into()));
        self
    }
}

fn push_once(rules: &mut Vec<Rule>, rule: Rule) {
    if !rules.contains(&rule) {
        rules.push(rule);
    }
}

/// A robots.txt.
///
/// [`Robots::new`] starts with one group for every crawler (`*`) that
/// allows everything. [`Robots::allow`] and [`Robots::disallow`] add rules
/// to that group; [`Robots::group`] adds a group for named crawlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Robots {
    groups: Vec<Group>,
    sitemaps: Vec<String>,
    closed: bool,
}

impl Default for Robots {
    fn default() -> Self {
        Self::new()
    }
}

impl Robots {
    /// Every crawler may read everything.
    pub fn new() -> Self {
        Self {
            groups: vec![Group::new("*")],
            sitemaps: Vec::new(),
            closed: false,
        }
    }

    /// No crawler may read anything: `User-agent: *` and `Disallow: /`.
    ///
    /// For a server that must not be indexed, such as staging. Rules and
    /// sitemaps added after this are kept but not written.
    pub fn closed() -> Self {
        Self {
            closed: true,
            ..Self::new()
        }
    }

    /// Every crawler may read `path`, even under a disallowed prefix.
    pub fn allow(mut self, path: impl Into<String>) -> Self {
        self.groups[0] = self.groups[0].clone().allow(path);
        self
    }

    /// No crawler may read `path` or anything under it.
    pub fn disallow(mut self, path: impl Into<String>) -> Self {
        self.groups[0] = self.groups[0].clone().disallow(path);
        self
    }

    /// A group for named crawlers, written after the `*` group.
    pub fn group(mut self, group: Group) -> Self {
        self.groups.push(group);
        self
    }

    /// Where the sitemap is: an absolute `http://` or `https://` URL.
    pub fn sitemap(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !self.sitemaps.contains(&url) {
            self.sitemaps.push(url);
        }
        self
    }

    /// Whether this is [`Robots::closed`].
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The text, or the first line that would be wrong.
    ///
    /// # Errors
    ///
    /// [`RobotsError`] when a rule path does not start with `/` or `*`,
    /// when a crawler token is empty or holds a space or a `:`, when a
    /// sitemap is not an absolute `http(s)` URL, or when any value holds a
    /// line break.
    pub fn render(&self) -> Result<String, RobotsError> {
        if self.closed {
            return Ok("User-agent: *\nDisallow: /\n".to_string());
        }
        let mut out = String::new();
        for (at, group) in self.groups.iter().enumerate() {
            if at > 0 {
                out.push('\n');
            }
            for agent in &group.agents {
                check_agent(agent)?;
                out.push_str("User-agent: ");
                out.push_str(agent);
                out.push('\n');
            }
            if group.rules.is_empty() {
                out.push_str("Disallow:\n");
            }
            for rule in &group.rules {
                let (field, path) = match rule {
                    Rule::Allow(path) => ("Allow", path),
                    Rule::Disallow(path) => ("Disallow", path),
                };
                check_path(path)?;
                out.push_str(field);
                out.push_str(": ");
                out.push_str(path);
                out.push('\n');
            }
        }
        if !self.sitemaps.is_empty() {
            out.push('\n');
        }
        for url in &self.sitemaps {
            check_sitemap(url)?;
            out.push_str("Sitemap: ");
            out.push_str(url);
            out.push('\n');
        }
        Ok(out)
    }
}

fn has_space(value: &str) -> bool {
    value.chars().any(char::is_whitespace)
}

fn check_agent(agent: &str) -> Result<(), RobotsError> {
    if agent.is_empty() || has_space(agent) || agent.contains(':') {
        return Err(RobotsError::Agent(agent.to_string()));
    }
    Ok(())
}

fn check_path(path: &str) -> Result<(), RobotsError> {
    if !(path.starts_with('/') || path.starts_with('*')) || has_space(path) {
        return Err(RobotsError::Path(path.to_string()));
    }
    Ok(())
}

fn check_sitemap(url: &str) -> Result<(), RobotsError> {
    let absolute = url.starts_with("https://") || url.starts_with("http://");
    if !absolute || has_space(url) {
        return Err(RobotsError::Sitemap(url.to_string()));
    }
    Ok(())
}

/// A value [`Robots::render`] refuses to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RobotsError {
    /// A rule path that does not start with `/` or `*`, or holds a space.
    Path(String),
    /// A crawler token that is empty, or holds a space or a `:`.
    Agent(String),
    /// A sitemap that is not an absolute `http(s)` URL, or holds a space.
    Sitemap(String),
}

impl fmt::Display for RobotsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(
                f,
                "robots.txt rule path {path:?} does not start with `/`, so crawlers ignore it\n\
                 help: write it from the root of the site, for example \"/{}\"",
                path.trim_start_matches(['.', '/'])
            ),
            Self::Agent(agent) => write!(
                f,
                "robots.txt crawler {agent:?} is not a product token\n\
                 help: name one crawler with no spaces or `:`, such as \"Googlebot\", or \"*\" for all"
            ),
            Self::Sitemap(url) => write!(
                f,
                "robots.txt sitemap {url:?} is not an absolute URL, so crawlers ignore it\n\
                 help: give the full address, for example \"https://example.com/sitemap.xml\""
            ),
        }
    }
}

impl std::error::Error for RobotsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_file_allows_everything() {
        assert_eq!(
            Robots::new().render().unwrap(),
            "User-agent: *\nDisallow:\n"
        );
    }

    #[test]
    fn closed_writes_two_lines_and_nothing_else() {
        let robots = Robots::closed()
            .allow("/public")
            .sitemap("https://example.com/sitemap.xml");
        assert_eq!(robots.render().unwrap(), "User-agent: *\nDisallow: /\n");
        assert!(robots.is_closed());
    }

    #[test]
    fn rules_groups_and_sitemaps_render_in_order() {
        let robots = Robots::new()
            .disallow("/admin")
            .allow("/admin/help")
            .disallow("/admin")
            .group(Group::new("GPTBot").agent("CCBot").disallow("/"))
            .sitemap("https://example.com/sitemap.xml");
        assert_eq!(
            robots.render().unwrap(),
            "User-agent: *\n\
             Disallow: /admin\n\
             Allow: /admin/help\n\
             \n\
             User-agent: GPTBot\n\
             User-agent: CCBot\n\
             Disallow: /\n\
             \n\
             Sitemap: https://example.com/sitemap.xml\n"
        );
    }

    #[test]
    fn a_wildcard_path_is_a_path() {
        let robots = Robots::new().disallow("*.pdf$");
        assert_eq!(
            robots.render().unwrap(),
            "User-agent: *\nDisallow: *.pdf$\n"
        );
    }

    #[test]
    fn a_relative_path_is_refused_with_the_fix() {
        let error = Robots::new().disallow("admin").render().unwrap_err();
        assert_eq!(error, RobotsError::Path("admin".to_string()));
        assert!(error
            .to_string()
            .contains("help: write it from the root of the site, for example \"/admin\""));
    }

    #[test]
    fn a_line_break_cannot_add_a_rule() {
        let error = Robots::new().disallow("/a\nAllow: /").render().unwrap_err();
        assert!(matches!(error, RobotsError::Path(_)));
    }

    #[test]
    fn a_relative_sitemap_and_a_bad_agent_are_refused() {
        assert!(matches!(
            Robots::new().sitemap("/sitemap.xml").render(),
            Err(RobotsError::Sitemap(_))
        ));
        assert!(matches!(
            Robots::new().group(Group::new("Bad Bot")).render(),
            Err(RobotsError::Agent(_))
        ));
    }
}
