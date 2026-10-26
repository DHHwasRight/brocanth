//! From the grammar to the two things a route source owes a project: a
//! `mount` function that fills the one table, and a `path()` per route.
//!
//! # Why the walk carries a prefix and a helper path
//!
//! A `scope` or a `resources` block changes two things about everything
//! inside it: where it answers, and what its helper is called. Both are
//! accumulated here, at expansion time, so the emitted table is a flat list
//! of `routes.route(..)` calls with no nesting left in it. Nothing at run
//! time has to walk a tree, and the expansion reads like the table it is.

use std::collections::{BTreeMap, HashSet};

use brocanth::routing::{generate_helpers, RouteHelper, RouteTemplate, Verb};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::parse::{Action, BlockKind, Group, Item, Resource, Route, Routes};

/// One registration: one path, one verb, one handler.
struct Emit {
    /// The path in brocanth syntax, without the mount prefix.
    template: String,
    verb: Verb,
    /// The expression that produces a `Handler`.
    handler: TokenStream2,
    /// Where this route's `path()` helper goes.
    helper: Vec<String>,
    /// The line that asked for it, for every message about it.
    span: Span,
}

/// Where the walk currently is.
#[derive(Clone, Default)]
struct Walk {
    /// The URL prefix accumulated so far, `""` at the top.
    prefix: String,
    /// The helper module path accumulated so far.
    helper: Vec<String>,
    /// Set by `shallow: true` on an ancestor: where a nested resource hangs
    /// its member routes instead of under the parent.
    shallow: Option<Anchor>,
}

/// The position a shallow nesting falls back to.
#[derive(Clone)]
struct Anchor {
    prefix: String,
    helper: Vec<String>,
}

/// The whole expansion, and every complaint it has.
#[derive(Default)]
struct State {
    rows: Vec<Emit>,
    errors: Option<syn::Error>,
}

impl State {
    fn complain(&mut self, span: Span, message: impl std::fmt::Display) {
        let error = syn::Error::new(span, message);
        match &mut self.errors {
            Some(existing) => existing.combine(error),
            None => self.errors = Some(error),
        }
    }

    fn items(&mut self, items: &[Item], walk: &Walk) {
        for item in items {
            match item {
                Item::Route(route) => self.route(route, walk),
                Item::Group(group) => self.group(group, walk),
                Item::Resource(resource) => self.resource(resource, walk),
                Item::Block(block) => self.complain(
                    block.span,
                    format!(
                        "`{}` only means something inside a `resources` or `resource` block",
                        match block.kind {
                            BlockKind::Member => "member",
                            BlockKind::Collection => "collection",
                        }
                    ),
                ),
            }
        }
    }

    fn route(&mut self, route: &Route, walk: &Walk) {
        let Some(template) = self.join(&walk.prefix, &route.path, route.path_span) else {
            return;
        };
        let helper = local_helper(&walk.helper, route.name.as_ref(), &route.path);
        let handler = &route.handler;
        // A bare path is a function, and a function is not a `Handler` until
        // `page` wraps it -- that wrapping is where the compiler checks the
        // signature and the return type. Anything else is taken as already
        // being a `Handler`, which is the escape hatch for a wrapper a project
        // writes itself, such as one that reads a body.
        let handler = if matches!(handler, syn::Expr::Path(_)) {
            quote_spanned!(handler.span()=> page(#handler))
        } else {
            quote!(#handler)
        };
        self.rows.push(Emit {
            template,
            verb: route.verb,
            handler,
            helper,
            span: route.path_span,
        });
    }

    fn group(&mut self, group: &Group, walk: &Walk) {
        let Some(prefix) = self.join(&walk.prefix, &group.path, group.path_span) else {
            return;
        };
        // A group nests the URL and the helper modules together, so a helper
        // path always reads like the URL it builds -- the same promise the
        // file side keeps, where `admin/users.rs` is `/admin/users`. Neither
        // group touches the handler path: a handler is an item the author
        // wrote and an editor can follow, and a macro that silently prepended
        // `admin::` to it would name a module that appears in no file.
        let helper = match &group.name {
            Some(name) => push(&walk.helper, &sanitize(&name.to_string())),
            None => extend(&walk.helper, &segments_of(&group.path)),
        };
        self.items(
            &group.items,
            &Walk {
                prefix,
                helper,
                shallow: walk.shallow.clone(),
            },
        );
    }

    fn resource(&mut self, resource: &Resource, walk: &Walk) {
        let actions = self.actions(resource);
        let Some(base) = self.join(&walk.prefix, &resource.path, resource.path_span) else {
            return;
        };
        let base_helper = local_helper(&walk.helper, resource.name.as_ref(), &resource.path);

        // Rails calls the member parameter `:id`, and the same parameter
        // `:post_id` once something is nested under it, because a template
        // cannot name `{id}` twice. brocanth's `RouteTemplate` says the same
        // thing with an error, so the two names are not a style choice.
        let member_param = resource
            .param
            .as_ref()
            .map_or_else(|| "id".to_owned(), ToString::to_string);
        let nested_param = resource.param.as_ref().map_or_else(
            || format!("{}_id", singularize(&last_literal(&resource.path))),
            ToString::to_string,
        );

        // A shallow nesting moves only the member routes, and only up to
        // where the shallow ancestor sits.
        let (member_root, member_root_helper) = match (&walk.shallow, resource.singular) {
            (Some(anchor), false) => {
                let Some(root) = self.join(&anchor.prefix, &resource.path, resource.path_span)
                else {
                    return;
                };
                (
                    root,
                    local_helper(&anchor.helper, resource.name.as_ref(), &resource.path),
                )
            }
            _ => (base.clone(), base_helper.clone()),
        };
        let (member, member_helper) = if resource.singular {
            (member_root.clone(), member_root_helper.clone())
        } else {
            (
                child(&member_root, &format!("{{{member_param}}}")),
                push(&member_root_helper, &sanitize(&member_param)),
            )
        };

        for action in actions {
            let function = Ident::new(action.function(), resource.controller_span);
            let controller = &resource.controller;
            let handler = quote_spanned!(resource.controller_span=> page(#controller::#function));
            let (template, helper) = match action {
                Action::Index | Action::Create => (base.clone(), base_helper.clone()),
                Action::New => (child(&base, "new"), push(&base_helper, "new")),
                Action::Show | Action::Update | Action::Destroy => {
                    (member.clone(), member_helper.clone())
                }
                Action::Edit => (child(&member, "edit"), push(&member_helper, "edit")),
            };
            for &verb in verbs_of(action) {
                self.rows.push(Emit {
                    template: template.clone(),
                    verb,
                    // One box per verb: `Handler` is a boxed closure, so
                    // PATCH and PUT cannot share one.
                    handler: handler.clone(),
                    helper: helper.clone(),
                    span: resource.span,
                });
            }
        }

        if resource.items.is_empty() {
            return;
        }
        // Anything written in the block is nested under one member, because
        // `/posts/{post_id}/comments` is what a nested resource means. The
        // `member` and `collection` blocks are the two exceptions, and they
        // say so by name.
        let (nested_prefix, nested_helper) = if resource.singular {
            (member_root.clone(), member_root_helper.clone())
        } else {
            (
                child(&member_root, &format!("{{{nested_param}}}")),
                push(&member_root_helper, &sanitize(&nested_param)),
            )
        };
        let anchor = if resource.shallow {
            walk.shallow.clone().or_else(|| {
                Some(Anchor {
                    prefix: walk.prefix.clone(),
                    helper: walk.helper.clone(),
                })
            })
        } else {
            walk.shallow.clone()
        };
        for item in &resource.items {
            match item {
                Item::Block(block) => {
                    let walk = match block.kind {
                        BlockKind::Member => Walk {
                            prefix: member.clone(),
                            helper: member_helper.clone(),
                            shallow: None,
                        },
                        BlockKind::Collection => Walk {
                            prefix: base.clone(),
                            helper: base_helper.clone(),
                            shallow: None,
                        },
                    };
                    self.items(&block.items, &walk);
                }
                other => self.items(
                    std::slice::from_ref(other),
                    &Walk {
                        prefix: nested_prefix.clone(),
                        helper: nested_helper.clone(),
                        shallow: anchor.clone(),
                    },
                ),
            }
        }
    }

    /// Which of the seven a resource keeps.
    fn actions(&mut self, resource: &Resource) -> Vec<Action> {
        // A singular resource is one of a thing, so there is nothing to list.
        let available: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|action| !(resource.singular && *action == Action::Index))
            .collect();

        if let (Some((_, only)), Some(_)) = (&resource.only, &resource.except) {
            self.complain(
                *only,
                "`only:` and `except:` are two answers to the same question; write one",
            );
            return available;
        }

        if let Some((wanted, span)) = &resource.only {
            for action in wanted {
                if !available.contains(action) {
                    self.complain(
                        *span,
                        format!(
                            "a singular `resource` has no `{}`; it is one of a thing",
                            action.function()
                        ),
                    );
                }
            }
            return available
                .into_iter()
                .filter(|action| wanted.contains(action))
                .collect();
        }

        if let Some((unwanted, _)) = &resource.except {
            return available
                .into_iter()
                .filter(|action| !unwanted.contains(action))
                .collect();
        }
        available
    }

    /// Append `path` to `prefix`, complaining if `path` is not a path.
    fn join(&mut self, prefix: &str, path: &str, span: Span) -> Option<String> {
        if !path.starts_with('/') {
            self.complain(span, format!("a path starts with `/`; `{path}` does not"));
            return None;
        }
        Some(join(prefix, path))
    }
}

/// The verbs one action answers.
///
/// `update` answers both PATCH and PUT, as Rails does: PATCH is the correct
/// verb for a partial update and PUT is what older clients and plain HTML
/// tooling send.
fn verbs_of(action: Action) -> &'static [Verb] {
    match action {
        Action::Index | Action::New | Action::Show | Action::Edit => &[Verb::Get],
        Action::Create => &[Verb::Post],
        Action::Update => &[Verb::Patch, Verb::Put],
        Action::Destroy => &[Verb::Delete],
    }
}

/// The entry point: tokens in, a module out.
pub fn expand(input: TokenStream2) -> TokenStream2 {
    let routes = match syn::parse2::<Routes>(input) {
        Ok(routes) => routes,
        Err(error) => return error.to_compile_error(),
    };

    let mut state = State::default();
    state.items(&routes.items, &Walk::default());
    if let Some(error) = state.errors {
        return error.to_compile_error();
    }

    let helpers = match check(&state.rows) {
        Ok(helpers) => helpers,
        Err(error) => return error.to_compile_error(),
    };

    let rows = state.rows.iter().map(|row| {
        let template = &row.template;
        let verb = Ident::new(verb_ident(row.verb), row.span);
        let handler = &row.handler;
        quote_spanned! {row.span=>
            routes.route(&at(prefix, #template), ::brocanth::routing::Verb::#verb, #handler)?;
        }
    });
    let rows: Vec<TokenStream2> = rows.collect();

    let body = if rows.is_empty() {
        // An empty table is legal -- a project may declare its routes later
        // -- but both arguments have to be used, or the project that mounts
        // it gets a warning it did not write.
        quote! {
            let _ = (routes, prefix);
        }
    } else {
        quote! {
            /// The mount prefix is a run-time value, so the table is joined
            /// here rather than in the macro.
            fn at(prefix: &str, path: &str) -> ::std::string::String {
                let mut out = ::std::string::String::with_capacity(prefix.len() + path.len() + 1);
                out.push_str(prefix.trim_end_matches('/'));
                if path != "/" {
                    out.push_str(path);
                }
                if out.is_empty() {
                    out.push('/');
                }
                out
            }
            #(#rows)*
        }
    };

    quote! {
        #helpers

        /// Add every declared route to `routes`, under `prefix`.
        ///
        /// Generated by `brocanth_routes::routes!`. `Handler` and `page` come
        /// from the project's `pages` module and have to be in scope here.
        ///
        /// # Errors
        ///
        /// Returns [`brocanth::routing::BuildError`] when a path does not
        /// parse under `prefix`, or when another source already claimed one
        /// of these paths for the same verb. One table means the second
        /// claim is reported rather than applied.
        pub fn mount(
            routes: &mut ::brocanth::routing::RouterBuilder<Handler>,
            prefix: &str,
        ) -> ::core::result::Result<(), ::brocanth::routing::BuildError> {
            #body
            ::core::result::Result::Ok(())
        }
    }
}

/// Validate the collected table, then build the helper module for it.
fn check(rows: &[Emit]) -> Result<TokenStream2, syn::Error> {
    let mut errors: Option<syn::Error> = None;
    let mut seen: HashSet<(String, Verb)> = HashSet::new();
    let mut parsed: BTreeMap<String, RouteTemplate> = BTreeMap::new();
    // Routes that share a path share one helper, because a helper answers
    // "what is this URL" and the verb does not change the answer.
    let mut grouped: BTreeMap<Vec<String>, (String, Vec<Verb>)> = BTreeMap::new();

    // The complaints are collected in one scope, so every one of them is
    // reported rather than only the first.
    {
        let mut complain = |span: Span, message: String| {
            let error = syn::Error::new(span, message);
            match &mut errors {
                Some(existing) => existing.combine(error),
                None => errors = Some(error),
            }
        };

        for row in rows {
            if let std::collections::btree_map::Entry::Vacant(slot) =
                parsed.entry(row.template.clone())
            {
                match RouteTemplate::parse(&row.template) {
                    Ok(template) => {
                        slot.insert(template);
                    }
                    Err(error) => {
                        complain(
                            row.span,
                            format!("`{}` is not a route: {error}", row.template),
                        );
                        continue;
                    }
                }
            }
            // Two declarations of one path and verb: the second would replace
            // the first, so which handler answers would depend on the order of
            // the file. That is the failure this macro exists to stop.
            if !seen.insert((row.template.clone(), row.verb)) {
                complain(
                    row.span,
                    format!(
                        "`{} {}` is declared twice in this table",
                        verb_name(row.verb),
                        row.template
                    ),
                );
            }
        }

        for row in rows {
            let entry = grouped
                .entry(row.helper.clone())
                .or_insert_with(|| (row.template.clone(), Vec::new()));
            if entry.0 != row.template {
                let module = if row.helper.is_empty() {
                    "root".to_owned()
                } else {
                    row.helper.join("::")
                };
                let message = format!(
                    "`{}` and `{}` both want the helper `routes::{module}`; \
                     give one of them `as: <name>`",
                    entry.0, row.template
                );
                complain(row.span, message);
            }
            if !entry.1.contains(&row.verb) {
                entry.1.push(row.verb);
            }
        }
    }

    if let Some(error) = errors {
        return Err(error);
    }

    let mut helpers = Vec::with_capacity(grouped.len());
    for (module, (template, verbs)) in grouped {
        let Some(template) = parsed.get(&template) else {
            continue;
        };
        helpers.push(RouteHelper::new(module, template.clone()).with_verbs(verbs));
    }

    let source = generate_helpers(&helpers)
        .map_err(|error| syn::Error::new(Span::call_site(), error.to_string()))?;
    let body: TokenStream2 = source.parse().map_err(|error| {
        syn::Error::new(
            Span::call_site(),
            format!("the generated route helpers do not parse: {error}"),
        )
    })?;

    Ok(quote! {
        /// One function per route: `routes::posts::id::path(7)`.
        ///
        /// Generated from the table below it, so a link to a route that is
        /// not declared does not compile, and neither does one missing a
        /// parameter. The paths are relative to the prefix `mount` was given.
        pub mod routes {
            #body
        }
    })
}

fn verb_ident(verb: Verb) -> &'static str {
    match verb {
        Verb::Get => "Get",
        Verb::Head => "Head",
        Verb::Post => "Post",
        Verb::Put => "Put",
        Verb::Patch => "Patch",
        Verb::Delete => "Delete",
        Verb::Options => "Options",
    }
}

fn verb_name(verb: Verb) -> &'static str {
    match verb {
        Verb::Get => "GET",
        Verb::Head => "HEAD",
        Verb::Post => "POST",
        Verb::Put => "PUT",
        Verb::Patch => "PATCH",
        Verb::Delete => "DELETE",
        Verb::Options => "OPTIONS",
    }
}

/// Where a route's helper goes: the enclosing modules, then either the name
/// it was given or the segments of its own path.
fn local_helper(base: &[String], name: Option<&Ident>, path: &str) -> Vec<String> {
    match name {
        Some(name) => push(base, &sanitize(&name.to_string())),
        None => extend(base, &segments_of(path)),
    }
}

fn push(base: &[String], segment: &str) -> Vec<String> {
    let mut out = base.to_vec();
    out.push(segment.to_owned());
    out
}

fn extend(base: &[String], segments: &[String]) -> Vec<String> {
    let mut out = base.to_vec();
    out.extend_from_slice(segments);
    out
}

/// The module names a path contributes. `/users/{id}` gives `users`, `id`,
/// which is what the file convention gives for `users/[id].rs`: one URL, one
/// helper, whichever half of the router declared it.
fn segments_of(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let inner = segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
                .map_or(segment, |name| name.strip_prefix('*').unwrap_or(name));
            sanitize(inner)
        })
        .collect()
}

/// The last segment of a path that is not a parameter, for singularising.
fn last_literal(path: &str) -> String {
    path.split('/')
        .rfind(|segment| !segment.is_empty() && !segment.starts_with('{'))
        .unwrap_or("record")
        .to_owned()
}

/// Turn a plural into a singular, well enough for a parameter name.
///
/// Four rules, not an inflector: the name it produces appears in the URL, so
/// a wrong guess is visible in the first test rather than hidden in a table.
/// `param: <name>` is the override, and the only thing an irregular plural
/// costs.
fn singularize(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies") {
        if !stem.is_empty() {
            return format!("{stem}y");
        }
    }
    for suffix in ["sses", "shes", "ches", "xes", "zes"] {
        if let Some(stem) = word.strip_suffix("es") {
            if word.ends_with(suffix) {
                return stem.to_owned();
            }
        }
    }
    if let Some(stem) = word.strip_suffix('s') {
        if !stem.is_empty() && !stem.ends_with('s') {
            return stem.to_owned();
        }
    }
    word.to_owned()
}

/// The core's rule for turning a path segment into a module name, repeated
/// here because it is `pub(crate)` there. Same rule, so a declared `/users/
/// {id}` and a `users/[id].rs` land in the same module.
fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    for (index, ch) in raw.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if index == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    if is_rust_keyword(&out) {
        out.insert_str(0, "r#");
    }
    out
}

fn is_rust_keyword(word: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
        "where", "while", "async", "await", "abstract", "become", "box", "do", "final", "macro",
        "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
    ];
    KEYWORDS.contains(&word)
}

/// Put `path` under `prefix`. `/` means "the prefix itself".
fn join(prefix: &str, path: &str) -> String {
    let mut out = prefix.trim_end_matches('/').to_owned();
    if path != "/" {
        out.push_str(path.trim_end_matches('/'));
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Add one segment below `base`.
fn child(base: &str, segment: &str) -> String {
    if base == "/" {
        format!("/{segment}")
    } else {
        format!("{base}/{segment}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expansion as text, which is how a macro's output is read.
    fn table(source: &str) -> String {
        expand(source.parse().expect("the test source is tokens")).to_string()
    }

    /// Just the `mount` body, so a `path()` helper cannot match by accident.
    fn mount(source: &str) -> String {
        let out = table(source);
        let at = out.find("pub fn mount").expect("mount is always emitted");
        out[at..].to_owned()
    }

    #[test]
    fn a_route_is_a_verb_a_path_and_a_handler() {
        let out = mount("get \"/about\" => pages::about::handle;");
        assert!(out.contains(r#"at (prefix , "/about")"#), "{out}");
        assert!(out.contains("Verb :: Get"), "{out}");
        assert!(out.contains("page (pages :: about :: handle)"), "{out}");
    }

    #[test]
    fn root_is_the_slash() {
        let out = mount("root => pages::index::handle;");
        assert!(out.contains(r#"at (prefix , "/")"#), "{out}");
    }

    #[test]
    fn a_handler_that_is_already_a_handler_is_not_wrapped_twice() {
        // The escape hatch for a project's own wrapper, such as one that
        // reads a body before calling the function.
        let out = mount("post \"/signup\" => page_with_body(pages::signup::handle_post);");
        assert!(
            out.contains("page_with_body (pages :: signup :: handle_post)"),
            "{out}"
        );
        assert!(!out.contains("page (page_with_body"), "{out}");
    }

    #[test]
    fn a_resource_is_seven_routes_on_four_paths() {
        let out = mount("resources \"/posts\" => ctrl;");
        for expected in [
            r#"at (prefix , "/posts") , :: brocanth :: routing :: Verb :: Get , page (ctrl :: index)"#,
            r#"at (prefix , "/posts") , :: brocanth :: routing :: Verb :: Post , page (ctrl :: create)"#,
            r#"at (prefix , "/posts/new") , :: brocanth :: routing :: Verb :: Get , page (ctrl :: new)"#,
            r#"at (prefix , "/posts/{id}") , :: brocanth :: routing :: Verb :: Get , page (ctrl :: show)"#,
            r#"at (prefix , "/posts/{id}/edit") , :: brocanth :: routing :: Verb :: Get , page (ctrl :: edit)"#,
            r#"at (prefix , "/posts/{id}") , :: brocanth :: routing :: Verb :: Patch , page (ctrl :: update)"#,
            r#"at (prefix , "/posts/{id}") , :: brocanth :: routing :: Verb :: Put , page (ctrl :: update)"#,
            r#"at (prefix , "/posts/{id}") , :: brocanth :: routing :: Verb :: Delete , page (ctrl :: destroy)"#,
        ] {
            assert!(out.contains(expected), "missing `{expected}` in {out}");
        }
    }

    #[test]
    fn only_keeps_the_actions_it_names() {
        let out = mount("resources \"/tags\" => ctrl, only: [index, show];");
        assert!(out.contains("ctrl :: index"), "{out}");
        assert!(out.contains("ctrl :: show"), "{out}");
        assert!(!out.contains("ctrl :: destroy"), "{out}");
        assert!(!out.contains("ctrl :: new"), "{out}");
    }

    #[test]
    fn except_drops_the_actions_it_names() {
        let out = mount("resources \"/tags\" => ctrl, except: [destroy, update];");
        assert!(out.contains("ctrl :: index"), "{out}");
        assert!(!out.contains("ctrl :: destroy"), "{out}");
        assert!(!out.contains("ctrl :: update"), "{out}");
    }

    #[test]
    fn only_and_except_together_are_a_question_with_two_answers() {
        let out = table("resources \"/tags\" => ctrl, only: [index], except: [show];");
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("two answers"), "{out}");
    }

    #[test]
    fn a_singular_resource_has_no_index() {
        let out = mount("resource \"/profile\" => ctrl;");
        assert!(!out.contains("ctrl :: index"), "{out}");
        assert!(
            out.contains(r#"at (prefix , "/profile") , :: brocanth :: routing :: Verb :: Get , page (ctrl :: show)"#),
            "{out}"
        );
        assert!(out.contains(r#"at (prefix , "/profile/edit")"#), "{out}");
        assert!(!out.contains("{id}"), "{out}");
    }

    #[test]
    fn a_scope_moves_the_url_and_the_helpers_with_it() {
        let out = table("scope \"/admin\" { get \"/\" => dash; }");
        assert!(out.contains(r#"at (prefix , "/admin")"#), "{out}");
        // The helper reads like the URL, which is what the file side also
        // promises. Without it, `/admin` would want the module `root`.
        assert!(out.contains("pub mod admin"), "{out}");
    }

    #[test]
    fn a_namespace_is_a_scope_written_as_one_word() {
        let scoped = table("scope \"/admin\" { get \"/users\" => h; }");
        let named = table("namespace admin { get \"/users\" => h; }");
        assert_eq!(scoped, named);
    }

    #[test]
    fn a_group_does_not_move_the_handler() {
        // The handler is exactly what was written. A macro that prepended
        // `admin::` would name a module that appears in no file.
        let out = mount("namespace admin { get \"/users\" => handlers::users; }");
        assert!(out.contains(r#"at (prefix , "/admin/users")"#), "{out}");
        assert!(out.contains("page (handlers :: users)"), "{out}");
    }

    #[test]
    fn member_and_collection_hang_off_the_right_path() {
        let out = mount(
            "resources \"/posts\" => ctrl {
                 member { get \"/preview\" => ctrl::preview; }
                 collection { get \"/search\" => ctrl::search; }
             }",
        );
        assert!(
            out.contains(r#"at (prefix , "/posts/{id}/preview")"#),
            "{out}"
        );
        assert!(out.contains(r#"at (prefix , "/posts/search")"#), "{out}");
    }

    #[test]
    fn a_block_outside_a_resource_says_so() {
        let out = table("member { get \"/x\" => h; }");
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("only means something inside"), "{out}");
    }

    #[test]
    fn a_nested_resource_renames_the_parent_parameter() {
        // `{id}` twice in one template is an error in the core, so the
        // parent's parameter has to change name once something is under it.
        let out = mount("resources \"/posts\" => p { resources \"/comments\" => c; }");
        assert!(
            out.contains(r#"at (prefix , "/posts/{post_id}/comments")"#),
            "{out}"
        );
        assert!(
            out.contains(r#"at (prefix , "/posts/{post_id}/comments/{id}")"#),
            "{out}"
        );
    }

    #[test]
    fn param_renames_both_the_member_and_the_nesting() {
        let out = mount("resources \"/posts\" => p, param: slug { resources \"/comments\" => c; }");
        assert!(out.contains(r#"at (prefix , "/posts/{slug}")"#), "{out}");
        assert!(
            out.contains(r#"at (prefix , "/posts/{slug}/comments")"#),
            "{out}"
        );
    }

    #[test]
    fn shallow_moves_only_the_nested_member_routes() {
        let out =
            mount("resources \"/posts\" => p, shallow: true { resources \"/comments\" => c; }");
        assert!(
            out.contains(r#"at (prefix , "/posts/{post_id}/comments")"#),
            "{out}"
        );
        assert!(out.contains(r#"at (prefix , "/comments/{id}")"#), "{out}");
        assert!(
            out.contains(r#"at (prefix , "/comments/{id}/edit")"#),
            "{out}"
        );
        assert!(
            !out.contains(r#""/posts/{post_id}/comments/{id}""#),
            "{out}"
        );
    }

    #[test]
    fn the_same_path_and_verb_twice_is_an_error_rather_than_a_winner() {
        let out = table("get \"/about\" => a; get \"/about\" => b;");
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("declared twice"), "{out}");
    }

    #[test]
    fn the_same_path_with_two_verbs_is_one_route() {
        let out = table("get \"/signup\" => a; post \"/signup\" => b;");
        assert!(!out.contains("compile_error !"), "{out}");
        assert!(out.contains(r#""GET" , "POST""#), "{out}");
    }

    #[test]
    fn a_path_without_a_leading_slash_is_not_a_path() {
        let out = table("get \"about\" => a;");
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("starts with `/`"), "{out}");
    }

    #[test]
    fn a_template_the_core_rejects_is_rejected_here() {
        let out = table("get \"/users/{id}/{id}\" => a;");
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("is not a route"), "{out}");
    }

    #[test]
    fn two_routes_that_want_one_helper_ask_for_a_name() {
        // Deriving the helper from the path cannot collide; two `as:` naming
        // the same module can, and silently losing one would break a link
        // the compiler is supposed to check.
        let out = table("get \"/a\" => x, as: thing; get \"/b\" => y, as: thing;");
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("as: <name>"), "{out}");
    }

    #[test]
    fn as_names_the_helper() {
        let out = table("get \"/a/b/c\" => h, as: deep;");
        assert!(out.contains("pub mod deep"), "{out}");
        assert!(!out.contains("pub mod a"), "{out}");
    }

    #[test]
    fn a_route_gets_a_path_function_taking_its_parameters() {
        let out = table("resources \"/posts\" => ctrl;");
        assert!(out.contains("pub mod posts"), "{out}");
        assert!(out.contains("pub mod id"), "{out}");
        assert!(out.contains("pub fn path (id : impl"), "{out}");
    }

    #[test]
    fn an_empty_table_still_mounts() {
        // A project may declare its routes later. Both arguments stay used,
        // so mounting an empty table warns about nothing.
        let out = mount("");
        assert!(out.contains("let _ = (routes , prefix)"), "{out}");
    }

    #[test]
    fn the_expansion_does_not_change_between_runs() {
        let source = "resources \"/posts\" => p; get \"/about\" => a; root => r;";
        assert_eq!(table(source), table(source));
    }

    #[test]
    fn singular_enough_for_a_parameter_name() {
        assert_eq!(singularize("posts"), "post");
        assert_eq!(singularize("categories"), "category");
        assert_eq!(singularize("boxes"), "box");
        assert_eq!(singularize("addresses"), "address");
        assert_eq!(singularize("media"), "media");
    }

    #[test]
    fn a_path_joins_without_doubling_a_slash() {
        assert_eq!(join("", "/"), "/");
        assert_eq!(join("", "/about"), "/about");
        assert_eq!(join("/admin", "/"), "/admin");
        assert_eq!(join("/admin/", "/posts"), "/admin/posts");
    }
}
