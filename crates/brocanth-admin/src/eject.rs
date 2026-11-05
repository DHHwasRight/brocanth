//! The shipped screens as source an application can own.
//!
//! `brocanth admin eject <screen>` copies one shipped template into a
//! project and installs it as the override. This module is the half of
//! that command that is a statement about this crate: which screens exist,
//! which template and component draw each, which context it takes, which
//! `Views` and `ModelAdmin` methods install it, and how its source changes
//! so it compiles outside the crate. The CLI does the file system.
//!
//! # What changes in an ejected copy
//!
//! The paths, and nothing else. A shipped template names this crate as
//! `crate::`. A copy in an application names it as `brocanth_admin::`, and
//! reaches the private assets through their public names:
//!
//! | Shipped | Ejected |
//! |---|---|
//! | `crate::templates::parts` | [`brocanth_admin::parts`](crate::parts) |
//! | `crate::view`, `crate::form`, `crate::paging` | the same modules, public |
//! | `crate::assets::{SCRIPT, STYLE}` | [`INLINE_SCRIPT`](crate::INLINE_SCRIPT), [`STYLESHEET`](crate::STYLESHEET) |
//! | `brocanth_middleware::dp::SCRIPT` | [`MUTATE_SCRIPT`](crate::MUTATE_SCRIPT) |
//!
//! So an ejected screen renders byte for byte what the shipped one does
//! until somebody edits it. `tests/eject.rs` holds the crate to that: it
//! compiles the copies in `tests/ejected/` as an outside crate would and
//! compares every screen's HTML with the default's.

/// One screen that can be ejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    /// The name on the command line, and the `.rsx` file's stem.
    pub name: &'static str,
    /// The component the template defines.
    pub component: &'static str,
    /// The context type in [`crate::view`].
    pub context: &'static str,
    /// The [`Views`](crate::Views) method that installs it for every model.
    pub every_model: &'static str,
    /// The [`ModelAdmin`](crate::ModelAdmin) method that installs it for one
    /// model. `None` for a screen that shows no one model.
    pub one_model: Option<&'static str>,
    /// The shipped template, as written in this crate.
    shipped: &'static str,
}

/// Every screen, in the order a page is built: the shell first, the two
/// blocks of the shell last.
pub const SCREENS: &[Screen] = &[
    Screen {
        name: "layout",
        component: "Layout",
        context: "LayoutCtx",
        every_model: "with_layout",
        one_model: None,
        shipped: include_str!("templates/layout.rsx"),
    },
    Screen {
        name: "index",
        component: "Index",
        context: "IndexCtx",
        every_model: "with_index",
        one_model: None,
        shipped: include_str!("templates/index.rsx"),
    },
    Screen {
        name: "list",
        component: "List",
        context: "ListCtx",
        every_model: "with_list",
        one_model: Some("list_view"),
        shipped: include_str!("templates/list.rsx"),
    },
    Screen {
        name: "detail",
        component: "Detail",
        context: "DetailCtx",
        every_model: "with_detail",
        one_model: Some("detail_view"),
        shipped: include_str!("templates/detail.rsx"),
    },
    Screen {
        name: "form",
        component: "FormPage",
        context: "FormCtx",
        every_model: "with_form",
        one_model: Some("form_view"),
        shipped: include_str!("templates/form.rsx"),
    },
    Screen {
        name: "delete",
        component: "Delete",
        context: "DeleteCtx",
        every_model: "with_confirm_delete",
        one_model: Some("confirm_delete_view"),
        shipped: include_str!("templates/delete.rsx"),
    },
    Screen {
        name: "error",
        component: "ErrorPage",
        context: "ErrorCtx",
        every_model: "with_error",
        one_model: None,
        shipped: include_str!("templates/error.rsx"),
    },
    Screen {
        name: "brand",
        component: "Brand",
        context: "Chrome",
        every_model: "with_brand",
        one_model: None,
        shipped: include_str!("templates/brand.rsx"),
    },
    Screen {
        name: "userlinks",
        component: "Userlinks",
        context: "Chrome",
        every_model: "with_userlinks",
        one_model: None,
        shipped: include_str!("templates/userlinks.rsx"),
    },
];

/// The screen with this name.
pub fn screen(name: &str) -> Option<&'static Screen> {
    SCREENS.iter().find(|s| s.name == name)
}

/// Every screen name, for a help line.
pub fn names() -> Vec<&'static str> {
    SCREENS.iter().map(|s| s.name).collect()
}

/// The paths a shipped template uses, and what a copy outside the crate
/// writes instead. Order matters only where one is a prefix of another.
const REWRITES: &[(&str, &str)] = &[
    ("use crate::templates::parts;", "use brocanth_admin::parts;"),
    (
        "use crate::assets::{SCRIPT, STYLE};",
        "use brocanth_admin::{INLINE_SCRIPT as SCRIPT, STYLESHEET as STYLE};",
    ),
    (
        "brocanth_middleware::dp::SCRIPT",
        "brocanth_admin::MUTATE_SCRIPT",
    ),
    ("use crate::", "use brocanth_admin::"),
];

/// The version of this crate, which a copy's header line names.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The first line of every copy, up to the version.
const HEADER: &str = "// Ejected from brocanth-admin ";

/// The version a copy names in its header line, when it has one.
///
/// `brocanth admin diff` reads it to say which release a copy came from.
pub fn ejected_version(copy: &str) -> Option<&str> {
    let rest = copy.lines().next()?.strip_prefix(HEADER)?;
    rest.split_whitespace().next()
}

/// A copy without the header [`Screen::source`] puts on top: the template
/// as the developer owns it. A file with no header comes back whole.
pub fn without_header(copy: &str) -> &str {
    if !copy.starts_with(HEADER) {
        return copy;
    }
    // The header is a run of `//` lines, then one empty line.
    match copy.find("\n\n") {
        Some(at) => &copy[at + 2..],
        None => copy,
    }
}

/// A shipped template's text, rewritten to compile outside the crate.
///
/// Public so `brocanth admin diff` can rewrite the template of the version
/// a project has installed, which may not be the version the CLI embeds.
pub fn rewrite(shipped: &str) -> String {
    let mut text = shipped.to_string();
    for (from, to) in REWRITES {
        text = text.replace(from, to);
    }
    text
}

impl Screen {
    /// The template as this crate ships it, without the rewrite.
    pub fn shipped(&self) -> &'static str {
        self.shipped
    }

    /// The template, rewritten to compile in an application, under a
    /// header that says where it came from.
    pub fn source(&self) -> String {
        let text = rewrite(self.shipped);
        format!(
            "// Ejected from brocanth-admin {version} by `brocanth admin eject {name}`.\n\
             //\n\
             // Yours now: an upgrade of brocanth-admin does not change this file.\n\
             // After one, `brocanth admin diff` compares it with the shipped\n\
             // `{name}.rsx`. Utility classes that the shipped stylesheet does not\n\
             // contain have no effect: add a stylesheet of your own for those\n\
             // (`AdminConfig::stylesheet`).\n\
             \n{text}",
            version = VERSION,
            name = self.name,
        )
    }
}

/// A screen an application has ejected: for every model, or for one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ejected {
    /// The screen's name.
    pub screen: &'static str,
    /// The model key, when the copy serves one model only.
    pub model: Option<String>,
}

impl Ejected {
    /// The template's path under the screens directory: `list.rsx`, or
    /// `models/<model>/list.rsx`.
    ///
    /// Under `models/` so a model may be named like a screen: `rsx` turns
    /// each directory into a module, and `list.rsx` beside `list/` would be
    /// two modules with one name.
    pub fn file(&self) -> String {
        match &self.model {
            None => format!("{}.rsx", self.screen),
            Some(model) => format!("models/{model}/{}.rsx", self.screen),
        }
    }

    fn module(&self) -> String {
        match &self.model {
            None => format!("screens::{}", self.screen),
            Some(model) => format!("screens::models::{model}::{}", self.screen),
        }
    }
}

/// Whether a model key can name a directory that `rsx` turns into a module.
///
/// A Rust identifier in snake case. A key outside that, such as
/// `blog-post`, is refused with a reason rather than mangled into a module
/// name the developer did not write.
pub fn model_key_is_a_module_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !matches!(
            key,
            "_" | "as"
                | "crate"
                | "fn"
                | "impl"
                | "in"
                | "let"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "type"
                | "use"
                | "where"
                | "async"
                | "await"
                | "dyn"
                | "loop"
                | "for"
                | "if"
                | "else"
                | "while"
                | "return"
                | "true"
                | "false"
                | "enum"
                | "const"
                | "extern"
                | "unsafe"
                | "break"
                | "continue"
        )
}

/// The Rust file that compiles the ejected templates and installs them.
///
/// `dir` is the screens directory relative to the package root, as
/// `rsx::load_dir!` resolves it. The text is already in rustfmt's shape, so
/// a project's first `cargo fmt` does not rewrite a file nobody touched.
pub fn wiring(dir: &str, ejected: &[Ejected]) -> String {
    let mut ejected = ejected.to_vec();
    ejected.sort();
    ejected.dedup();

    let mut contexts: Vec<&str> = ejected
        .iter()
        .filter_map(|e| screen(e.screen))
        .map(|s| s.context)
        .collect();
    contexts.sort_unstable();
    contexts.dedup();
    let contexts = match contexts.as_slice() {
        [one] => (*one).to_string(),
        // rustfmt's shape: one line up to 100 columns, else the list on a
        // line of its own.
        many if many.join(", ").len() + "use brocanth_admin::view::{};".len() <= 100 => {
            format!("{{{}}}", many.join(", "))
        }
        many => format!("{{\n    {},\n}}", many.join(", ")),
    };

    let mut body = String::new();
    for e in &ejected {
        let Some(s) = screen(e.screen) else { continue };
        let (open, receiver, method) = match (&e.model, s.one_model) {
            (Some(model), Some(method)) => (format!("config.model({model:?}, |m| {{"), "m", method),
            _ => (
                "config.map_views(|views| {".to_string(),
                "views",
                s.every_model,
            ),
        };
        let params = format!("|ctx: &{}, w: &mut Writer<'_>|", s.context);
        let call = format!("{}::{} {{ ctx }}.render(w)", e.module(), s.component);
        // rustfmt's rule: the closure stays on one line when that line fits
        // in 100 columns, and takes a block otherwise.
        let one_line = format!("        {receiver}.{method}({params} {call})");
        let inner = if one_line.len() <= 100 {
            format!("{one_line}\n")
        } else {
            format!(
                "        {receiver}.{method}({params} {{\n\
                 \x20           {call}\n\
                 \x20       }})\n"
            )
        };
        body.push_str(&format!("    config = {open}\n{inner}    }});\n"));
    }

    format!(
        "//! The admin screens this project ejected, installed over the shipped ones.\n\
         //!\n\
         //! Written by `brocanth admin eject`, and written again on every eject\n\
         //! from the `.rsx` files under `{dir}`. Edit those, not this.\n\
         //! A screen for one model wins over one for every model, as\n\
         //! `docs/admin-customization.md` describes.\n\
         \n\
         use brocanth_admin::view::{contexts};\n\
         use brocanth_admin::AdminConfig;\n\
         use rsx::{{Template as _, Writer}};\n\
         \n\
         mod screens {{\n\
         \x20   rsx::load_dir!({dir:?});\n\
         }}\n\
         \n\
         /// Installs every ejected screen on `config`.\n\
         pub fn apply(mut config: AdminConfig) -> AdminConfig {{\n\
         {body}\
         \x20   config\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The copies `tests/eject.rs` compiles are the copies the command
    /// writes. `BROCANTH_BLESS=1` rewrites them after a template changes.
    #[test]
    fn the_compiled_copies_are_what_eject_writes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ejected");
        let bless = std::env::var_os("BROCANTH_BLESS").is_some();
        for s in SCREENS {
            let path = dir.join(format!("{}.rsx", s.name));
            if bless {
                std::fs::write(&path, s.source()).expect("write a copy");
                continue;
            }
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                on_disk == s.source(),
                "tests/ejected/{}.rsx is out of date: run BROCANTH_BLESS=1 cargo test -p \
                 brocanth-admin --features scaffold",
                s.name
            );
        }
    }

    #[test]
    fn a_copy_names_nothing_private() {
        for s in SCREENS {
            let source = s.source();
            assert!(!source.contains("crate::"), "{} names crate::", s.name);
            assert!(
                !source.contains("brocanth_middleware"),
                "{} needs a dependency an application may not have",
                s.name
            );
        }
    }

    #[test]
    fn every_shipped_template_has_a_screen() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/templates");
        for entry in std::fs::read_dir(dir).expect("the templates") {
            let name = entry.expect("an entry").file_name();
            let stem = name.to_string_lossy().trim_end_matches(".rsx").to_string();
            // `parts` draws nothing: it is public as `brocanth_admin::parts`.
            if stem != "parts" {
                assert!(screen(&stem).is_some(), "{stem}.rsx cannot be ejected");
            }
        }
    }

    #[test]
    fn the_header_names_the_version_and_comes_off() {
        for s in SCREENS {
            let copy = s.source();
            assert_eq!(ejected_version(&copy), Some(env!("CARGO_PKG_VERSION")));
            assert_eq!(without_header(&copy), rewrite(s.shipped()));
        }
        assert_eq!(ejected_version("use x;\n"), None);
        assert_eq!(
            without_header("use x;\n\nfn a() {}\n"),
            "use x;\n\nfn a() {}\n"
        );
    }

    #[test]
    fn a_model_key_must_be_a_module_name() {
        assert!(model_key_is_a_module_name("invoice"));
        assert!(model_key_is_a_module_name("blog_post2"));
        assert!(!model_key_is_a_module_name("blog-post"));
        assert!(!model_key_is_a_module_name("Post"));
        assert!(!model_key_is_a_module_name("2fa"));
        assert!(!model_key_is_a_module_name("type"));
        assert!(!model_key_is_a_module_name(""));
    }

    /// Every screen for every model, and every per-model screen for one,
    /// in one file: the widest the wiring gets.
    fn everything() -> Vec<Ejected> {
        let mut all: Vec<Ejected> = SCREENS
            .iter()
            .map(|s| Ejected {
                screen: s.name,
                model: None,
            })
            .collect();
        all.extend(
            SCREENS
                .iter()
                .filter(|s| s.one_model.is_some())
                .map(|s| Ejected {
                    screen: s.name,
                    model: Some("invoice".into()),
                }),
        );
        all
    }

    #[test]
    fn the_wiring_is_already_formatted() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("rustfmt").arg("--version").output().is_err() {
            eprintln!("skipped: rustfmt is not on PATH");
            return;
        }
        for ejected in [
            everything(),
            vec![Ejected {
                screen: "index",
                model: None,
            }],
        ] {
            let source = wiring("src/plugins/admin/screens", &ejected);
            let mut child = Command::new("rustfmt")
                .args(["--edition", "2021", "--emit", "stdout"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("rustfmt runs");
            child
                .stdin
                .take()
                .expect("a pipe")
                .write_all(source.as_bytes())
                .expect("rustfmt reads");
            let done = child.wait_with_output().expect("rustfmt finishes");
            assert!(done.status.success(), "rustfmt could not read:\n{source}");
            let formatted = String::from_utf8_lossy(&done.stdout);
            assert_eq!(formatted, source, "the wiring is not in rustfmt's shape");
        }
    }

    #[test]
    fn the_wiring_installs_one_model_and_every_model() {
        let text = wiring(
            "src/plugins/admin/screens",
            &[
                Ejected {
                    screen: "list",
                    model: Some("invoice".into()),
                },
                Ejected {
                    screen: "layout",
                    model: None,
                },
            ],
        );
        assert!(text.contains("rsx::load_dir!(\"src/plugins/admin/screens\");"));
        assert!(text.contains("use brocanth_admin::view::{LayoutCtx, ListCtx};"));
        assert!(text.contains("views.with_layout(|ctx: &LayoutCtx"));
        assert!(text.contains("screens::layout::Layout { ctx }.render(w)"));
        assert!(text.contains("config.model(\"invoice\", |m| {"));
        assert!(text.contains("screens::models::invoice::list::List { ctx }.render(w)"));
    }
}
