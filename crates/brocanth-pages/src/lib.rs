//! `src/pages`, read while the crate compiles.
//!
//! Two macros, one idea: **a route is a file**, so the directory tree is the
//! source of truth and nothing restates it. `pages!` turns the tree into the
//! route table; `html!` turns a page's position in it into the layouts that
//! wrap the page.
//!
//! ```ignore
//! // src/pages/mod.rs
//! brocanth_pages::pages!("src/pages");   // the mod lines, and router()
//! ```
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

mod naming;
mod table;

use std::path::{Path, PathBuf};

use proc_macro::{Span, TokenStream};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use naming::{ident, to_module_name};

/// The route table, read off the directory tree.
///
/// Takes the pages root, relative to the package: `pages!("src/pages")`. It
/// emits the `mod` lines and a `router()`, which is both halves of what was
/// written by hand before.
///
/// A route is a file, so the set of routes is the set of files. Adding a
/// page is adding a file; renaming one renames its route; deleting one
/// deletes it. There is no list, so there is no list to keep in step.
///
/// The verb comes off the handler's name -- `handle` is GET, `handle_post`
/// is POST -- and the page's source is parsed to find them, because which
/// verbs a page answers is a fact about its Rust and not about its name. A
/// `pub fn handle_<something>` that is not a verb is an error naming the
/// ones that are, rather than a handler that compiles and is never reached.
///
/// Two things it will not do. `mod.rs` is skipped, because it is this table
/// rather than a page; and a page with no handler at all is an error, not an
/// empty row.
#[proc_macro]
pub fn pages(input: TokenStream) -> TokenStream {
    let raw = input.to_string();
    let Some(root) = string_literal(&raw) else {
        return error(
            "expected a string literal with the pages root, as in \
             `brocanth_pages::pages!(\"src/pages\")`",
        );
    };
    table::expand(Path::new(&root), &manifest_dir()).into()
}

/// The path out of `pages!("...")`, without pulling in a parser for it.
fn string_literal(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('\\')).then(|| inner.to_string())
}

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

fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
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
