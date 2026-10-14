//! `html!`, which puts a page inside the layouts its directory puts it in.
//!
//! # The rule
//!
//! `layout.rsx` wraps every page in its directory and in every directory
//! below it, and they stack from the root down. A page never names a layout,
//! because which layout covers a page is a property of **where the page is**,
//! not of what it says. That is the rule Crystal's `page_router` uses for
//! `layout.cr`, and this macro is how it is read off the tree in Rust.
//!
//! ```ignore
//! // src/pages/users/[id].rs
//! pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
//!     let id: u64 = ctx.require("id")?.parse()?;
//!     html!(views::users::_id_::User { id })
//! }
//! ```
//!
//! With `src/pages/layout.rsx` and `src/pages/users/layout.rsx` present, that
//! expands to the root layout wrapping the `users` layout wrapping the page.
//! Delete `users/layout.rsx` and the same line expands to one layer. Nothing
//! in the page changes either way, and there is no chain to keep in step.
//!
//! # How it knows
//!
//! `Span::call_site().local_file()` gives the path of the file the macro was
//! written in, relative to the package root. Everything else is reading
//! directories: walk up from there to `src/`, and collect a `layout.rsx`
//! wherever one is.
//!
//! Each layout found is also `include_bytes!`d by the expansion, which is
//! what makes cargo rebuild when one is edited. Adding a layout where there
//! was none is the case cargo cannot see on its own -- nothing yet references
//! the file -- so touch any source file in the crate after creating it.
//!
//! # What it assumes
//!
//! Four things, all of them what `brocanth create --with rsx` writes:
//!
//! - pages live under `src/pages`;
//! - `src/pages/mod.rs` has `pub mod views { rsx::load_dir!("src/pages"); }`;
//! - a layout is a file called `layout.rsx`;
//! - **a layout component takes exactly one parameter, `children: Element`.**
//!
//! The last one is the real constraint. This macro fills `children` and
//! nothing else, because it cannot know what another parameter would mean.
//! A layout that needs a value reads it from a `const` or a function; a value
//! that belongs to one request belongs on the page component, which the
//! handler builds and which already has the context.

use std::path::{Path, PathBuf};

use proc_macro::{Span, TokenStream};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};

/// Renders a page inside the layouts that cover it.
///
/// See the crate documentation for the rule and what it assumes.
#[proc_macro]
pub fn html(input: TokenStream) -> TokenStream {
    let content = TokenStream2::from(input);

    let Some(file) = Span::call_site().local_file() else {
        return error(
            "`html!` cannot tell which file it is in, so it cannot tell which \
             layouts cover this page; call `pages::render` and name the layout \
             yourself",
        );
    };

    let layouts = layouts_above(&file);
    expand(&layouts, &content).into()
}

/// The layouts covering `file`, outermost first.
///
/// Walking up rather than down: a page is covered by what is above it, and
/// the directories above a file are a fact about its path rather than
/// something to search for.
fn layouts_above(file: &Path) -> Vec<Layout> {
    let root = manifest_dir();
    let mut found = Vec::new();

    let mut dir = file.parent();
    while let Some(current) = dir {
        // `src` is the ceiling. Above it is the package, which holds
        // `Cargo.toml` and no markup.
        if current.file_name().is_some_and(|name| name == "src") || current.as_os_str().is_empty() {
            break;
        }
        let candidate = root.join(current).join("layout.rsx");
        if candidate.is_file() {
            found.push(Layout {
                module: module_path_from(current),
                file: candidate,
            });
        }
        dir = current.parent();
    }

    // Collected leaf to root, applied root to leaf.
    found.reverse();
    found
}

struct Layout {
    /// The directory, as the module path `rsx::load_dir!` gave it. Empty for
    /// the pages root itself.
    module: Vec<String>,
    file: PathBuf,
}

/// `src/pages/users` -> `["pages", "views", "users"]`.
///
/// The `views` sits where it does because `load_dir!` is written inside a
/// module of that name: the generated `pub mod index` would otherwise collide
/// with the `pub mod index;` naming the page.
fn module_path_from(dir: &Path) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut seen_src = false;
    for component in dir.components() {
        let name = component.as_os_str().to_string_lossy();
        if !seen_src {
            if name == "src" {
                seen_src = true;
            }
            continue;
        }
        parts.push(to_module_name(&name));
        // `views` is declared in the pages root, so it comes directly after
        // it and before any directory below.
        if parts.len() == 1 {
            parts.push("views".to_string());
        }
    }
    parts
}

fn expand(layouts: &[Layout], content: &TokenStream2) -> TokenStream2 {
    // Innermost outwards, so the first layout found ends up outermost.
    let mut nested = quote!(#content);
    for layout in layouts.iter().rev() {
        let path = layout.module.iter().map(|part| ident(part));
        nested = quote! {
            crate:: #(#path ::)* layout::Layout { children: #nested }
        };
    }

    // The same trick rsx uses: a file nothing references is a file cargo does
    // not watch, and a layout read at expansion time is referenced by nothing.
    let watched = layouts.iter().map(|layout| {
        let path = layout.file.to_string_lossy().into_owned();
        quote!(
            const _: &[u8] = include_bytes!(#path);
        )
    });

    quote! {
        {
            #(#watched)*
            crate::pages::render(#nested)
        }
    }
}

/// `format_ident!` panics on `r#match`, which is a name a directory may
/// legitimately have, so raw identifiers are built as raw identifiers.
fn ident(part: &str) -> proc_macro2::Ident {
    match part.strip_prefix("r#") {
        Some(bare) => proc_macro2::Ident::new_raw(bare, proc_macro2::Span::call_site()),
        None => format_ident!("{}", part),
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
}

/// rsx's rule for turning a file or directory name into a module name, so
/// `[id]` reaches `_id_` and the two agree about a route like `/users/{id}`.
fn to_module_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        out.push(if ch.is_alphanumeric() || ch == '_' {
            ch
        } else {
            '_'
        });
    }
    if out.chars().next().is_some_and(|c| c.is_numeric()) {
        out.insert(0, '_');
    }
    if is_keyword(&out) {
        return format!("r#{out}");
    }
    out
}

fn is_keyword(word: &str) -> bool {
    const KEYWORDS: [&str; 49] = [
        "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
        "unsafe", "use", "where", "while", "async", "await", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "typeof", "unsized", "virtual",
    ];
    KEYWORDS.contains(&word)
}

fn error(message: &str) -> TokenStream {
    quote!(compile_error!(#message);).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pages_root_reaches_the_views_module_and_stops() {
        assert_eq!(module_path_from(Path::new("src/pages")), ["pages", "views"]);
    }

    #[test]
    fn a_directory_below_it_is_named_under_views() {
        assert_eq!(
            module_path_from(Path::new("src/pages/users")),
            ["pages", "views", "users"]
        );
        assert_eq!(
            module_path_from(Path::new("src/pages/blog/archive")),
            ["pages", "views", "blog", "archive"]
        );
    }

    #[test]
    fn a_directory_whose_name_is_not_an_identifier_gets_the_one_rsx_gave_it() {
        // `[category]` is a route parameter on disk and `_category_` in the
        // module tree. The two have to agree or the layout is not found.
        assert_eq!(
            module_path_from(Path::new("src/pages/blog/[category]")),
            ["pages", "views", "blog", "_category_"]
        );
    }

    #[test]
    fn a_directory_named_for_a_keyword_is_escaped_rather_than_renamed() {
        assert_eq!(
            module_path_from(Path::new("src/pages/match")),
            ["pages", "views", "r#match"]
        );
    }

    #[test]
    fn a_keyword_directory_survives_being_made_into_a_path() {
        // `format_ident!` panics on `r#match`, so this would be a panic
        // inside the compiler rather than an error anyone could read.
        let layouts = [Layout {
            module: module_path_from(Path::new("src/pages/match")),
            file: PathBuf::from("/tmp/layout.rsx"),
        }];
        let out = expand(&layouts, &quote!(Page {})).to_string();
        assert!(out.contains("r#match"), "{out}");
    }

    #[test]
    fn the_layouts_nest_outermost_first() {
        let layouts = [
            Layout {
                module: module_path_from(Path::new("src/pages")),
                file: PathBuf::from("/tmp/a.rsx"),
            },
            Layout {
                module: module_path_from(Path::new("src/pages/users")),
                file: PathBuf::from("/tmp/b.rsx"),
            },
        ];
        let out = expand(&layouts, &quote!(Page {})).to_string();
        let root = out.find("views :: layout").expect("the root layout");
        let users = out.find("views :: users :: layout").expect("the users one");
        assert!(root < users, "the root layout must be the outer one: {out}");
        // And each one is watched, or an edit to it changes nothing.
        assert_eq!(out.matches("include_bytes !").count(), 2, "{out}");
    }

    #[test]
    fn no_layout_at_all_is_the_page_and_nothing_else() {
        let out = expand(&[], &quote!(Page {})).to_string();
        assert!(!out.contains("layout"), "{out}");
        assert!(out.contains("crate :: pages :: render"), "{out}");
    }

    #[test]
    fn a_name_starting_with_a_digit_is_still_an_identifier() {
        assert_eq!(
            module_path_from(Path::new("src/pages/2024")),
            ["pages", "views", "_2024"]
        );
    }
}
