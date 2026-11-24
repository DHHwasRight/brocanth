//! `markdown` -- what `brocanth create` writes when somebody asks to render
//! Markdown. Compiled only under the `scaffold` feature.
//!
//! The module writes no code, as `sanitize` writes none. A page calls
//! `brocanth_markdown::render` (the site's own text) or `render_user` (a
//! visitor's) where it shows Markdown, so a generated call site would be a
//! guess about a page that does not exist yet. What it writes is the
//! dependency, with its `rsx` feature beside `rsx`, and the rule an agent
//! must follow. `content` is the module that renders files with it.

use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Project, Scaffold, Slot,
};

/// Markdown to cleaned HTML, as a choice on the menu.
pub struct MarkdownModule;

impl Module for MarkdownModule {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "markdown",
            "Markdown to cleaned HTML: GFM tables and footnotes, heading ids, TOML front matter; links html5ever",
        )
        .contributes::<AgentsSection>(
            Contribution::new("markdown", "how Markdown becomes HTML on a page"),
            true,
        )
        .category(Category::Renderer)
        .asked_in("pages", "Render Markdown to HTML, cleaned")
        .krate(Dep::brocanth("brocanth-markdown").feature_with("rsx", "rsx"))
        .adapts_to(
            "rsx",
            "the result is an rsx template, so a page writes it with `{! body }`",
        )
        // A library a page calls. A static export renders pages too, and a
        // page made from a Markdown file needs it there as much.
        .outside_runtime()
        .absent(
            "brocanth-markdown",
            "no Markdown; a page shows Markdown as text, or links a parser and a sanitizer by hand",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let write = if project.has("rsx") {
            AGENTS_RSX
        } else {
            AGENTS_STRING
        };
        s.at(Slot::AgentsSections)
            .block(AGENTS.replace("@WRITE@", write));
    }
}

/// What the generated `AGENTS.md` says about Markdown.
const AGENTS: &str = "\n\
    ## Markdown\n\
    \n\
    Markdown becomes HTML only through `brocanth-markdown`, which cleans\n\
    the result with the sanitizer's allow-list:\n\
    `brocanth_markdown::render(text)` for text the site's authors wrote,\n\
    `brocanth_markdown::render_user(text)` for text a visitor typed. Both\n\
    return a `SafeHtml`. @WRITE@ Do not call a Markdown parser directly:\n\
    its raw HTML and `javascript:` links reach the page.\n\
    \n\
    Front matter is TOML between two `+++` lines, read into a type with\n\
    `brocanth_markdown::front_matter::<T>(text)`. Give the type\n\
    `#[serde(deny_unknown_fields)]`, so a misspelt key is an error.\n";

/// How a page writes it, in an rsx template.
const AGENTS_RSX: &str = "Write it with `{! body }`.";

/// How a page writes it, into the HTML string it returns.
const AGENTS_STRING: &str = "Push `body.as_str()` into the page string.";
