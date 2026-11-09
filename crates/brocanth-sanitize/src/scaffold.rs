//! `sanitize` -- what `brocanth create` writes when somebody asks to show
//! HTML a user wrote. Compiled only under the `scaffold` feature.
//!
//! The module writes no code. A sanitizer is called by the page that shows
//! the HTML, with the policy that page needs, so a generated call site
//! would be a guess about a page that does not exist yet. What it writes is
//! the dependency and the rule an agent must follow: user HTML reaches a
//! template as a `SafeHtml`, and nothing else is written raw.

use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

/// The sanitizer, as a choice on the menu.
pub struct Sanitize;

impl Module for Sanitize {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sanitize",
            "an allow-list HTML sanitizer and a SafeHtml type; links html5ever",
        )
        .category(Category::Renderer)
        .asked_in("pages", "Show HTML that users write, cleaned")
        .krate(Dep::brocanth("brocanth-sanitize").feature_with("rsx", "rsx"))
        // A library a page calls. A static export renders pages too, and a
        // page that shows Markdown from a file needs it there as much.
        .outside_runtime()
        .absent(
            "sanitize",
            "no sanitizer; HTML a user wrote can only be shown escaped, as text",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// What the generated `AGENTS.md` says about user HTML.
const AGENTS: &str = "\n\
    ## HTML from users\n\
    \n\
    HTML a user wrote reaches a template only as a\n\
    `brocanth_sanitize::SafeHtml`. Make one with a policy:\n\
    `brocanth_sanitize::comment(&row.body)` for a comment,\n\
    `brocanth_sanitize::rich_text(&row.body)` for editor output, or a\n\
    `Policy` built once in a `static` for anything else. Write it with\n\
    `{! value }`.\n\
    \n\
    Never write a `String` with `{! }`, and never widen a policy with\n\
    `style`, `class`, `id`, an `on*` attribute or a `javascript:` URL.\n\
    Store what the user wrote and clean it when the page renders, so a\n\
    stricter policy also covers old rows.\n";
