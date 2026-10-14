//! Turning a file or directory name into a module name.
//!
//! rsx's rule, reimplemented rather than imported: the two have to agree
//! about `[id]`, and depending on `rsx-core` to share ten lines would put the
//! whole template compiler into this crate's build.

use quote::format_ident;

/// rsx's rule for turning a file or directory name into a module name, so
/// `[id]` reaches `_id_` and the two agree about a route like `/users/{id}`.
pub fn to_module_name(name: &str) -> String {
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

/// `format_ident!` panics on `r#match`, which is a name a directory may
/// legitimately have, so raw identifiers are built as raw identifiers.
pub fn ident(part: &str) -> proc_macro2::Ident {
    match part.strip_prefix("r#") {
        Some(bare) => proc_macro2::Ident::new_raw(bare, proc_macro2::Span::call_site()),
        None => format_ident!("{}", part),
    }
}
