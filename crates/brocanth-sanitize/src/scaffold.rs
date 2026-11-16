//! `sanitize` -- what `brocanth create` writes when somebody asks to show
//! HTML a user wrote. Compiled only under the `scaffold` feature.
//!
//! The module writes no code. A sanitizer is called by the page that shows
//! the HTML, with the policy that page needs, so a generated call site
//! would be a guess about a page that does not exist yet. What it writes is
//! the dependency and the rule an agent must follow: user HTML reaches a
//! template as a `SafeHtml`, and nothing else is written raw.

use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Project, Scaffold, Slot,
};

/// The sanitizer, as a choice on the menu.
pub struct Sanitize;

impl Module for Sanitize {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sanitize",
            "an allow-list HTML sanitizer and a SafeHtml type; links html5ever",
        )
        .contributes::<AgentsSection>(
            Contribution::new(
                "sanitize",
                "how HTML from users is cleaned before it is shown",
            ),
            true,
        )
        .category(Category::Renderer)
        .asked_in("pages", "Show HTML that users write, cleaned")
        .krate(Dep::brocanth("brocanth-sanitize").feature_with("rsx", "rsx"))
        .adapts_to(
            "rsx",
            "`SafeHtml` is an rsx template, so a page writes it with `{! value }`",
        )
        // A library a page calls. A static export renders pages too, and a
        // page that shows Markdown from a file needs it there as much.
        .outside_runtime()
        .absent(
            "sanitize",
            "no sanitizer; HTML a user wrote can only be shown escaped, as text",
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

/// What the generated `AGENTS.md` says about user HTML.
const AGENTS: &str = "\n\
    ## HTML from users\n\
    \n\
    HTML a user wrote reaches a page only as a\n\
    `brocanth_sanitize::SafeHtml`. Make one with a policy:\n\
    `brocanth_sanitize::comment(&row.body)` for a comment,\n\
    `brocanth_sanitize::rich_text(&row.body)` for editor output, or a\n\
    `Policy` built once in a `static` for anything else. @WRITE@\n\
    \n\
    Never widen a policy with `style`, `class`, `id`, an `on*` attribute\n\
    or a `javascript:` URL. Store what the user wrote and clean it when\n\
    the page renders, so a stricter policy also covers old rows.\n";

/// How a page writes it, in an rsx template.
const AGENTS_RSX: &str = "Write it with\n\
    `{! value }`, and never write a `String` with `{! }`.";

/// How a page writes it, into the HTML string it returns.
const AGENTS_STRING: &str = "Push\n\
    `safe.as_str()` into the page string, and escape every other value\n\
    with `SafeHtml::escape`.";
