#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! The `policy!` macro: PLAN.md §12 item 10.
//!
//! Not depended on directly. `brocanth-policy` re-exports it behind its
//! `macro` feature, and every path the macro emits is
//! `::brocanth_policy::…`.
//!
//! # What it emits
//!
//! The code an application writes by hand today, and nothing else:
//!
//! * `entity Post in "posts" { … }` emits `struct Post`, `enum PostColumn`,
//!   `impl Column for PostColumn`, `impl Entity for Post`,
//!   `impl typed::TypedColumn for PostColumn`, and
//!   `impl Sortable for PostColumn` (the kind a keyset cursor checks).
//! * `policy PostPolicy(who: &Visitor) for Post { … }` emits
//!   `struct PostPolicy` and `impl Policy for PostPolicy`. Each rule is the
//!   method of the same name. A guard `cond => pred,` is
//!   `if cond { return pred; }`. A comparison is the `Pred` constructor of
//!   the same name, with its value passed through `typed::bind`, which
//!   checks the type and returns the same `Bound` as `value.into()`.
//! * `eval { … }` emits one `#[cfg(test)]` test that calls
//!   `agreement::policy_agrees` (PLAN.md §9 obligation 1).
//! * `exists Link(fk == col, …)` emits the `Membership::new(…).filter_by(…)`
//!   chain a person writes by hand.
//! * `#[db]` on a policy (docs/policy-rls-bridge.md) also emits
//!   `impl DbPolicy` with `fn rules()`, the rules as data, and implements
//!   each `Policy` rule as `rules().<rule>.resolve(&who.attributes())`.
//!   `Db` refuses what the database cannot check. Without `#[db]`, the
//!   expansion is the one above.
//!
//! `docs/policy.md` has the syntax and each real call site, before and after.

use std::collections::BTreeMap;

use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro2::{Spacing, Span, TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream, Parser};
use syn::spanned::Spanned;
use syn::{
    braced, bracketed, parenthesized, Attribute, Expr, Ident, LitInt, LitStr, Path, Token, Type,
    Visibility,
};

mod subject;

mod kw {
    syn::custom_keyword!(entity);
    syn::custom_keyword!(policy);
}

/// Declares entities with typed columns, and the rules over them.
///
/// ```ignore
/// brocanth_policy::policy! {
///     pub entity Post in "posts" {
///         #[key] id: i64,
///         author_id: Option<i64>,
///         status: String,
///         title: String,
///     }
///
///     pub policy PostPolicy(who: &Visitor) for Post {
///         list {
///             who.admin => always,
///             status == "published" || author_id == who.user_id else deny,
///         }
///         create: author_id == who.user_id else deny,
///         update {
///             who.admin => always,
///             author_id == who.user_id else deny,
///         }
///         writable {
///             who.admin => except [id],
///             only [title],
///         }
///         // A create may also set the owner, which `create` pins to the
///         // subject. An update may not change it.
///         writable_on_create {
///             who.admin => except [id],
///             only [title, author_id],
///         }
///     }
/// }
/// ```
///
/// See `docs/policy.md` for the whole syntax.
#[proc_macro]
pub fn policy(input: TokenStream) -> TokenStream {
    match syn::parse::<Input>(input).and_then(expand) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Implements `brocanth_policy::Subject` for a struct.
///
/// Each field marked `#[attr]` becomes a subject attribute that a rule may
/// read: an entry of `Subject::ATTRS`, keyed by the field name, and a value
/// in `Subject::attributes()`. An `Option` field is optional. A field
/// without `#[attr]` is not an attribute and never reaches the database.
///
/// ```ignore
/// #[derive(brocanth_policy::Subject)]
/// pub struct Visitor {
///     #[attr] pub user_id: Option<i64>,   // brocanth.user_id  bigint
///     #[attr] pub admin: bool,            // brocanth.admin    boolean
///     pub display_name: String,           // not an attribute
/// }
/// ```
///
/// An attribute is `bool`, `i32`, `i64`, `String`, `uuid::Uuid`,
/// `chrono::DateTime<Utc>`, or an `Option` of one of them. See
/// docs/policy-rls-bridge.md, decision 5.
///
/// It also emits a hidden struct `<Name>_attributes`, with one `&'static
/// str` field per attribute, and the const `<Name>::BROCANTH_ATTRIBUTES`.
/// A `#[db]` policy reads each key there, so a rule that reads a field
/// without `#[attr]` does not compile.
#[proc_macro_derive(Subject, attributes(attr))]
pub fn derive_subject(input: TokenStream) -> TokenStream {
    match syn::parse::<syn::DeriveInput>(input).and_then(subject::expand) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// ---------------------------------------------------------------------------
// The syntax tree
// ---------------------------------------------------------------------------

struct Input {
    entities: Vec<EntityDecl>,
    policies: Vec<PolicyDecl>,
}

struct EntityDecl {
    attrs: Vec<Attribute>,
    vis: Visibility,
    name: Ident,
    table: LitStr,
    model: Option<Path>,
    columns: Vec<ColumnDecl>,
}

struct ColumnDecl {
    attrs: Vec<Attribute>,
    key: bool,
    name: Ident,
    ty: Type,
}

impl ColumnDecl {
    /// The name as the database spells it: the field name, without `r#`.
    fn sql_name(&self) -> String {
        self.name.unraw().to_string()
    }

    fn variant(&self) -> Ident {
        Ident::new(&self.sql_name().to_upper_camel_case(), self.name.span())
    }
}

struct PolicyDecl {
    attrs: Vec<Attribute>,
    /// `#[db]`: the rules are data too (docs/policy-rls-bridge.md).
    db: Option<Span>,
    vis: Visibility,
    name: Ident,
    subject_pat: TokenStream2,
    subject_ty: Type,
    entity: Ident,
    rules: Vec<(Ident, Rule<PredExpr>)>,
    masks: Vec<(Ident, Rule<MaskExpr>)>,
    eval: Option<EvalClause>,
}

/// Guards, then a fallback. `cond => then,` is `if cond { return then; }`.
struct Rule<T> {
    /// Doc comments and other attributes, put on the method.
    attrs: Vec<Attribute>,
    /// `#[app_only("reason")]`: with `#[db]`, the rule stays Rust code.
    app_only: Option<LitStr>,
    arms: Vec<(Expr, T)>,
    fallback: Option<T>,
}

enum PredExpr {
    Always,
    Never,
    Cmp {
        column: Ident,
        op: &'static str,
        value: ValueExpr,
    },
    Null {
        column: Ident,
        negated: bool,
    },
    In {
        column: Ident,
        negated: bool,
        set: Expr,
    },
    All(Vec<PredExpr>),
    Any(Vec<PredExpr>),
    Not(Box<PredExpr>),
    Exists(ExistsExpr),
    Block(syn::Block),
}

/// `exists Link(foreign == local, filter, …)`, or `!exists …`.
struct ExistsExpr {
    /// The entity of the link table, declared in the same block.
    entity: Ident,
    /// The column of the outer entity that the link table points at.
    local: Ident,
    /// The column of the link table that matches `local`.
    foreign: Ident,
    /// Each further constraint: a `Cmp` or a `Null` over a link column.
    filter: Vec<PredExpr>,
    negated: bool,
}

struct ValueExpr {
    expr: Expr,
    /// `else deny`, `else allow` or `else null`.
    absent: Option<Ident>,
}

enum MaskExpr {
    All,
    None,
    Only(Vec<Ident>),
    Except(Vec<Ident>),
    Block(syn::Block),
}

struct EvalClause {
    span: Span,
    subjects: Expr,
    database: Expr,
    dialect: Ident,
    cases: Option<LitInt>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

const RULES: &str =
    "list, view, create, update, update_check, delete, writable, writable_on_create, eval";

impl Parse for Input {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entities = Vec::new();
        let mut policies = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(Attribute::parse_outer)?;
            let vis: Visibility = input.parse()?;
            let lookahead = input.lookahead1();
            if lookahead.peek(kw::entity) {
                input.parse::<kw::entity>()?;
                entities.push(parse_entity(input, attrs, vis)?);
            } else if lookahead.peek(kw::policy) {
                input.parse::<kw::policy>()?;
                policies.push(parse_policy(input, attrs, vis)?);
            } else {
                return Err(syn::Error::new(
                    input.span(),
                    "expected `entity` or `policy`\nhelp: start each item with `entity Name in \"table\" { … }` or `policy Name(who: &Subject) for Entity { … }`",
                ));
            }
        }
        Ok(Input { entities, policies })
    }
}

fn parse_entity(
    input: ParseStream,
    attrs: Vec<Attribute>,
    vis: Visibility,
) -> syn::Result<EntityDecl> {
    let name: Ident = input.parse()?;
    input.parse::<Token![in]>()?;
    let table: LitStr = input.parse()?;
    check_identifier(&table)?;

    let mut model = None;
    let mut kept = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("model") {
            if model.is_some() {
                return Err(syn::Error::new(
                    attr.span(),
                    "`#[model(…)]` appears twice\nhelp: name one model struct",
                ));
            }
            model = Some(attr.parse_args::<Path>()?);
        } else {
            kept.push(attr);
        }
    }

    let content;
    braced!(content in input);
    let mut columns: Vec<ColumnDecl> = Vec::new();
    let mut errors: Option<syn::Error> = None;
    while !content.is_empty() {
        let attrs = content.call(Attribute::parse_outer)?;
        let mut key = false;
        let mut kept = Vec::new();
        for attr in attrs {
            if attr.path().is_ident("key") {
                key = true;
            } else {
                kept.push(attr);
            }
        }
        let column_name = content.call(Ident::parse_any)?;
        content.parse::<Token![:]>()?;
        let ty: Type = content.parse()?;
        if !content.is_empty() {
            content.parse::<Token![,]>()?;
        }
        let column = ColumnDecl {
            attrs: kept,
            key,
            name: column_name,
            ty,
        };
        if columns.iter().any(|c| c.sql_name() == column.sql_name()) {
            push_error(
                &mut errors,
                syn::Error::new(
                    column.name.span(),
                    format!(
                        "column `{}` appears twice in entity `{name}`\nhelp: declare each column once",
                        column.sql_name(),
                    ),
                ),
            );
        }
        columns.push(column);
    }

    let keys: Vec<&ColumnDecl> = columns.iter().filter(|c| c.key).collect();
    match keys.as_slice() {
        [] => push_error(
            &mut errors,
            syn::Error::new(
                name.span(),
                format!("entity `{name}` has no key column\nhelp: put `#[key]` on the primary key column"),
            ),
        ),
        [_] => {}
        [_, second, ..] => push_error(
            &mut errors,
            syn::Error::new(
                second.name.span(),
                format!("entity `{name}` has more than one `#[key]` column\nhelp: put `#[key]` on one column only"),
            ),
        ),
    }
    if let Some(e) = errors {
        return Err(e);
    }

    Ok(EntityDecl {
        attrs: kept,
        vis,
        name,
        table,
        model,
        columns,
    })
}

fn check_identifier(table: &LitStr) -> syn::Result<()> {
    let value = table.value();
    let mut chars = value.chars();
    let ok = match chars.next() {
        Some(first) => {
            (first.is_ascii_alphabetic() || first == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    };
    if ok {
        Ok(())
    } else {
        Err(syn::Error::new(
            table.span(),
            format!("`{value}` is not a bare SQL identifier\nhelp: use letters, digits and `_` only, and start with a letter or `_`"),
        ))
    }
}

fn parse_policy(
    input: ParseStream,
    attrs: Vec<Attribute>,
    vis: Visibility,
) -> syn::Result<PolicyDecl> {
    let name: Ident = input.parse()?;

    let mut db = None;
    let mut attrs_kept = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("db") {
            attrs_kept.push(attr);
            continue;
        }
        if !matches!(attr.meta, syn::Meta::Path(_)) {
            return Err(syn::Error::new(
                attr.span(),
                "`#[db]` takes no arguments\nhelp: write `#[db]`",
            ));
        }
        if db.is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "`#[db]` appears twice\nhelp: write it once",
            ));
        }
        db = Some(attr.span());
    }
    let attrs = attrs_kept;

    let params;
    parenthesized!(params in input);
    let subject_pat = if params.peek(Token![_]) {
        params.parse::<Token![_]>()?.into_token_stream()
    } else {
        params.parse::<Ident>()?.into_token_stream()
    };
    params.parse::<Token![:]>()?;
    let ty: Type = params.parse()?;
    let subject_ty = match ty {
        Type::Reference(reference) if reference.mutability.is_none() => *reference.elem,
        other => {
            return Err(syn::Error::new(
                other.span(),
                "the subject parameter must be a shared reference\nhelp: write `&Subject`; a rule reads the subject and does not own it",
            ))
        }
    };
    if !params.is_empty() {
        return Err(params.error(
            "a policy takes one parameter, the subject\nhelp: remove the other parameters",
        ));
    }

    input.parse::<Token![for]>()?;
    let entity: Ident = input.parse()?;

    let content;
    braced!(content in input);
    let mut rules: Vec<(Ident, Rule<PredExpr>)> = Vec::new();
    let mut masks: Vec<(Ident, Rule<MaskExpr>)> = Vec::new();
    let mut eval = None;
    while !content.is_empty() {
        let attrs = content.call(Attribute::parse_outer)?;
        let (attrs, app_only) = take_app_only(attrs)?;
        let rule: Ident = content.call(Ident::parse_any)?;
        let label = rule.to_string();
        if let Some(reason) = &app_only {
            let help = match label.as_str() {
                "list" | "view" | "create" | "update" | "delete" => None,
                "update_check" => Some("help: `update_check` is read by the database only; write it in the `policy!` syntax"),
                "writable" | "writable_on_create" => Some("help: field masks always stay in the application (docs/policy-rls-bridge.md, decision 9); remove the attribute"),
                _ => Some("help: put `#[app_only(\"reason\")]` on a rule: list, view, create, update or delete"),
            };
            if let Some(help) = help {
                return Err(syn::Error::new(
                    reason.span(),
                    format!("`#[app_only]` does not apply to `{label}`\n{help}"),
                ));
            }
        }
        let seen = rules.iter().any(|(r, _)| *r == rule)
            || masks.iter().any(|(r, _)| *r == rule)
            || (label == "eval" && eval.is_some());
        if seen {
            return Err(syn::Error::new(
                rule.span(),
                format!("rule `{label}` appears twice in policy `{name}`\nhelp: write each rule once; use `||` to join two predicates"),
            ));
        }
        match label.as_str() {
            "list" | "view" | "create" | "update" | "update_check" | "delete" => {
                let mut body = parse_rule(&content, &rule, attrs, parse_pred_tokens)?;
                body.app_only = app_only;
                rules.push((rule, body));
            }
            "writable" | "writable_on_create" => {
                let body = parse_rule(&content, &rule, attrs, parse_mask_tokens)?;
                masks.push((rule, body));
            }
            "eval" => {
                if let Some(attr) = attrs.first() {
                    return Err(syn::Error::new(
                        attr.span(),
                        "`eval` takes no attributes\nhelp: put the comment on the policy",
                    ));
                }
                eval = Some(parse_eval(&content, &rule)?);
            }
            _ => {
                let help = if label == "read" {
                    "help: the rule for one row is `view`".to_owned()
                } else {
                    format!("help: use one of: {RULES}")
                };
                return Err(syn::Error::new(
                    rule.span(),
                    format!("`{label}` is not a rule\n{help}"),
                ));
            }
        }
    }

    Ok(PolicyDecl {
        attrs,
        db,
        vis,
        name,
        subject_pat,
        subject_ty,
        entity,
        rules,
        masks,
        eval,
    })
}

/// The rule's attributes without `#[app_only("reason")]`, and the reason.
fn take_app_only(attrs: Vec<Attribute>) -> syn::Result<(Vec<Attribute>, Option<LitStr>)> {
    let mut kept = Vec::new();
    let mut reason: Option<LitStr> = None;
    for attr in attrs {
        if !attr.path().is_ident("app_only") {
            kept.push(attr);
            continue;
        }
        const HELP: &str = "help: write `#[app_only(\"why the database cannot check this rule\")]`";
        let lit = attr.parse_args::<LitStr>().map_err(|_| {
            syn::Error::new(attr.span(), format!("`#[app_only]` needs a reason\n{HELP}"))
        })?;
        if lit.value().trim().is_empty() {
            return Err(syn::Error::new(
                lit.span(),
                format!("the reason of `#[app_only]` is empty\n{HELP}"),
            ));
        }
        if reason.is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "`#[app_only]` appears twice\nhelp: write it once",
            ));
        }
        reason = Some(lit);
    }
    Ok((kept, reason))
}

/// `name: expr,` or `name { cond => expr, …, expr }`.
fn parse_rule<T>(
    input: ParseStream,
    rule: &Ident,
    attrs: Vec<Attribute>,
    item: fn(TokenStream2, Span) -> syn::Result<T>,
) -> syn::Result<Rule<T>> {
    if input.peek(Token![:]) {
        input.parse::<Token![:]>()?;
        let tokens = until_comma(input)?;
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
        }
        if tokens.is_empty() {
            return Err(syn::Error::new(
                rule.span(),
                format!("rule `{rule}` is empty\nhelp: write a predicate after `{rule}:`"),
            ));
        }
        return Ok(Rule {
            attrs,
            app_only: None,
            arms: Vec::new(),
            fallback: Some(item(tokens, rule.span())?),
        });
    }
    if !input.peek(syn::token::Brace) {
        return Err(input.error(format!(
            "expected `:` or `{{` after rule `{rule}`\nhelp: write `{rule}: <predicate>,` or `{rule} {{ <guard> => <predicate>, <predicate> }}`"
        )));
    }
    let content;
    braced!(content in input);
    let mut arms = Vec::new();
    let mut fallback: Option<T> = None;
    while !content.is_empty() {
        let tokens: Vec<TokenTree> = until_comma(&content)?.into_iter().collect();
        if !content.is_empty() {
            content.parse::<Token![,]>()?;
        }
        if tokens.is_empty() {
            continue;
        }
        let first_span = tokens[0].span();
        if fallback.is_some() {
            return Err(syn::Error::new(
                first_span,
                format!("this line comes after the fallback of rule `{rule}`\nhelp: put the line without `=>` last"),
            ));
        }
        match fat_arrow(&tokens) {
            Some(index) => {
                let cond: TokenStream2 = tokens[..index].iter().cloned().collect();
                let then: TokenStream2 = tokens[index + 2..].iter().cloned().collect();
                if cond.is_empty() {
                    return Err(syn::Error::new(
                        tokens[index].span(),
                        "a guard has no condition before `=>`\nhelp: write `<condition> => <result>`",
                    ));
                }
                let arrow = tokens[index + 1].span();
                if then.is_empty() {
                    return Err(syn::Error::new(
                        arrow,
                        "a guard has no result after `=>`\nhelp: write `<condition> => <result>`",
                    ));
                }
                let cond: Expr = syn::parse2(cond)?;
                arms.push((cond, item(then, arrow)?));
            }
            None => {
                let tokens: TokenStream2 = tokens.into_iter().collect();
                fallback = Some(item(tokens, first_span)?);
            }
        }
    }
    Ok(Rule {
        attrs,
        app_only: None,
        arms,
        fallback,
    })
}

/// The tokens up to the next comma at this level, which is not consumed.
fn until_comma(input: ParseStream) -> syn::Result<TokenStream2> {
    let mut out = TokenStream2::new();
    while !input.is_empty() && !input.peek(Token![,]) {
        out.extend([input.parse::<TokenTree>()?]);
    }
    Ok(out)
}

/// The index of the `=` of the first top-level `=>`.
fn fat_arrow(tokens: &[TokenTree]) -> Option<usize> {
    tokens.windows(2).position(|pair| {
        matches!(
            pair,
            [TokenTree::Punct(a), TokenTree::Punct(b)]
                if a.as_char() == '=' && a.spacing() == Spacing::Joint && b.as_char() == '>'
        )
    })
}

fn parse_eval(input: ParseStream, rule: &Ident) -> syn::Result<EvalClause> {
    let content;
    braced!(content in input);
    let mut subjects = None;
    let mut database = None;
    let mut dialect = None;
    let mut cases = None;
    while !content.is_empty() {
        let field: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        let tokens = until_comma(&content)?;
        if !content.is_empty() {
            content.parse::<Token![,]>()?;
        }
        match field.to_string().as_str() {
            "subjects" => subjects = Some(syn::parse2::<Expr>(tokens)?),
            "database" => database = Some(syn::parse2::<Expr>(tokens)?),
            "dialect" => {
                let ident: Ident = syn::parse2(tokens)?;
                if ident != "Postgres" {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("`{ident}` is not a dialect\nhelp: use `Postgres`, the only dialect brocanth supports"),
                    ));
                }
                dialect = Some(ident);
            }
            "cases" => cases = Some(syn::parse2::<LitInt>(tokens)?),
            other => {
                return Err(syn::Error::new(
                    field.span(),
                    format!("`{other}` is not an `eval` field\nhelp: use `subjects`, `database`, and optionally `dialect` and `cases`"),
                ))
            }
        }
    }
    let missing = |what: &str, help: &str| {
        syn::Error::new(rule.span(), format!("`eval` has no `{what}`\nhelp: {help}"))
    };
    Ok(EvalClause {
        span: rule.span(),
        subjects: subjects.ok_or_else(|| {
            missing("subjects", "list the subjects to test, for example `subjects: [Visitor::anonymous(), Visitor::admin()]`")
        })?,
        database: database.ok_or_else(|| {
            missing("database", "name a function `fn(&[MapRow<Column>]) -> impl Oracle<Entity>` that returns a database with exactly these rows")
        })?,
        dialect: dialect.unwrap_or_else(|| Ident::new("Postgres", rule.span())),
        cases,
    })
}

// --- predicates -------------------------------------------------------------

fn parse_pred_tokens(tokens: TokenStream2, _span: Span) -> syn::Result<PredExpr> {
    let parser = |input: ParseStream| {
        let pred = parse_or(input)?;
        if !input.is_empty() {
            return Err(input.error(
                "unexpected tokens after the predicate\nhelp: join predicates with `&&` or `||`",
            ));
        }
        Ok(pred)
    };
    parser.parse2(tokens)
}

fn parse_or(input: ParseStream) -> syn::Result<PredExpr> {
    let mut parts = vec![parse_and(input)?];
    while input.peek(Token![||]) {
        input.parse::<Token![||]>()?;
        parts.push(parse_and(input)?);
    }
    Ok(if parts.len() == 1 {
        parts.remove(0)
    } else {
        PredExpr::Any(parts)
    })
}

fn parse_and(input: ParseStream) -> syn::Result<PredExpr> {
    let mut parts = vec![parse_unary(input)?];
    while input.peek(Token![&&]) {
        input.parse::<Token![&&]>()?;
        parts.push(parse_unary(input)?);
    }
    Ok(if parts.len() == 1 {
        parts.remove(0)
    } else {
        PredExpr::All(parts)
    })
}

fn parse_unary(input: ParseStream) -> syn::Result<PredExpr> {
    if input.peek(Token![!]) {
        let bang = input.parse::<Token![!]>()?;
        let inner = parse_unary(input)?;
        // `!exists` is `NOT EXISTS`, one term: an absent `else deny` value
        // still makes it `never` (docs/policy.md, "Membership").
        if let PredExpr::Exists(mut exists) = inner {
            exists.negated = !exists.negated;
            return Ok(PredExpr::Exists(exists));
        }
        if let Some((absent, in_exists)) = decided_absence(&inner) {
            let help = if in_exists {
                format!("help: put `!` directly before `exists`: `!exists` keeps `else {absent}` as written")
            } else {
                format!("help: write the inverse operator instead of `!`, for example `author_id != who.user_id else {absent}`")
            };
            return Err(syn::Error::new(
                bang.span,
                format!(
                    "`!` over a comparison with `else {absent}` turns the decision for an absent value around\n{help}"
                ),
            ));
        }
        return Ok(PredExpr::Not(Box::new(inner)));
    }
    if input.peek(syn::token::Paren) {
        let content;
        parenthesized!(content in input);
        let inner = parse_or(&content)?;
        if !content.is_empty() {
            return Err(content.error("unexpected tokens in the parentheses"));
        }
        return Ok(inner);
    }
    if input.peek(syn::token::Brace) {
        return Ok(PredExpr::Block(input.parse()?));
    }
    let word = input.call(Ident::parse_any).map_err(|e| {
        syn::Error::new(
            e.span(),
            "expected a predicate\nhelp: write `always`, `never`, `<column> <op> <value>`, `exists Link(…)`, `!`, `( … )` or a `{ … }` block that returns a `Pred`",
        )
    })?;
    if let Some(op) = comparison(input) {
        return parse_comparison(input, word, op);
    }
    match word.to_string().as_str() {
        "always" => Ok(PredExpr::Always),
        "never" => Ok(PredExpr::Never),
        "exists" => parse_exists(input),
        _ => Err(syn::Error::new(
            word.span(),
            format!("expected a comparison after column `{word}`\nhelp: use `==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `is not`, `in` or `not in`"),
        )),
    }
}

/// The `else deny` or `else allow` inside a predicate, when it has one.
///
/// `else deny` compiles to `never` for an absent value, and `!never` is
/// `always`: negated, the word grants access to the visitor it was written
/// to refuse. `else null` stays unknown under `!`, so it is not reported.
/// A `{ … }` block is Rust code, and the macro does not look inside it.
///
/// The flag is `true` when the word is in the filter of an `exists`.
fn decided_absence(pred: &PredExpr) -> Option<(String, bool)> {
    match pred {
        PredExpr::Cmp {
            value: ValueExpr {
                absent: Some(word), ..
            },
            ..
        } if word != "null" => Some((word.to_string(), false)),
        PredExpr::All(parts) | PredExpr::Any(parts) => parts.iter().find_map(decided_absence),
        PredExpr::Not(inner) => decided_absence(inner),
        PredExpr::Exists(exists) => exists
            .filter
            .iter()
            .find_map(decided_absence)
            .map(|(word, _)| (word, true)),
        _ => None,
    }
}

/// `exists Link(foreign == local, filter, …)`, after the word `exists`.
///
/// The left side of each comparison is a column of `Link`. The right side
/// is a column of the outer entity (a bare name), a literal, or a value
/// such as `who.user_id else deny`. Exactly one comparison links the two
/// tables, with `==` and a bare name.
fn parse_exists(input: ParseStream) -> syn::Result<PredExpr> {
    const SHAPE: &str = "help: write `exists Link(<column of Link> == <column of this entity>, <column of Link> == <value>, …)`";
    let entity: Ident = input.parse().map_err(|e| {
        syn::Error::new(
            e.span(),
            format!("expected the link entity after `exists`\n{SHAPE}"),
        )
    })?;
    if !input.peek(syn::token::Paren) {
        return Err(input.error(format!("expected `(` after `exists {entity}`\n{SHAPE}")));
    }
    let content;
    let parens = parenthesized!(content in input);
    let mut link: Option<(Ident, Ident)> = None;
    let mut filter = Vec::new();
    while !content.is_empty() {
        let tokens = until_comma(&content)?;
        if !content.is_empty() {
            content.parse::<Token![,]>()?;
        }
        if tokens.is_empty() {
            continue;
        }
        let item = (|input: ParseStream| {
            let column = input.call(Ident::parse_any)?;
            let Some(op) = comparison(input) else {
                return Err(syn::Error::new(
                    column.span(),
                    format!("expected a comparison after column `{column}`\n{SHAPE}"),
                ));
            };
            let pred = parse_comparison(input, column, op)?;
            if !input.is_empty() {
                return Err(input.error(format!(
                    "unexpected tokens in `exists`; separate the comparisons with `,`\n{SHAPE}"
                )));
            }
            Ok(pred)
        })
        .parse2(tokens)?;
        match item {
            PredExpr::In { column, .. } => {
                return Err(syn::Error::new(
                    column.span(),
                    "`in` is not allowed inside `exists`\nhelp: compare each column of the link table with one value",
                ))
            }
            PredExpr::Cmp { column, op, value } => match bare_name(&value.expr) {
                Some(local) if op == "eq" && value.absent.is_none() => {
                    if link.is_some() {
                        return Err(syn::Error::new(
                            column.span(),
                            format!("`exists {entity}` links to this row twice\nhelp: keep one `<column of {entity}> == <column>`; compare the other columns with a literal or a `who.<field>` value"),
                        ));
                    }
                    link = Some((column, local.clone()));
                }
                Some(local) => {
                    return Err(syn::Error::new(
                        local.span(),
                        format!("a bare name in `exists` is a column of this entity, and links the tables with `==` and no `else`\nhelp: write `{column} == {local}`, or write a constant with its path, for example `self::{local}`"),
                    ))
                }
                None => filter.push(PredExpr::Cmp { column, op, value }),
            },
            other => filter.push(other),
        }
    }
    let Some((foreign, local)) = link else {
        return Err(syn::Error::new(
            parens.span.join(),
            format!("`exists {entity}` does not say which row of `{entity}` belongs to this row\nhelp: add `<column of {entity}> == <column of this entity>`, for example `post_id == id`"),
        ));
    };
    Ok(PredExpr::Exists(ExistsExpr {
        entity,
        local,
        foreign,
        filter,
        negated: false,
    }))
}

/// The name, when the expression is one bare name: `id`, not `who.id`,
/// `x::Y` or `"id"`.
fn bare_name(expr: &Expr) -> Option<&Ident> {
    match expr {
        Expr::Path(path) if path.qself.is_none() && path.attrs.is_empty() => path.path.get_ident(),
        Expr::Paren(inner) => bare_name(&inner.expr),
        Expr::Group(inner) => bare_name(&inner.expr),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Comparison {
    Op(&'static str),
    Is,
    In,
    NotIn,
}

/// Which comparison follows, without consuming it.
fn comparison(input: ParseStream) -> Option<Comparison> {
    if input.peek(Token![==]) {
        Some(Comparison::Op("eq"))
    } else if input.peek(Token![!=]) {
        Some(Comparison::Op("ne"))
    } else if input.peek(Token![<=]) {
        Some(Comparison::Op("le"))
    } else if input.peek(Token![>=]) {
        Some(Comparison::Op("ge"))
    } else if input.peek(Token![<]) {
        Some(Comparison::Op("lt"))
    } else if input.peek(Token![>]) {
        Some(Comparison::Op("gt"))
    } else if input.peek(Token![in]) {
        Some(Comparison::In)
    } else if peek_word(input, "is") {
        Some(Comparison::Is)
    } else if peek_word(input, "not") && input.peek2(Token![in]) {
        Some(Comparison::NotIn)
    } else {
        None
    }
}

fn peek_word(input: ParseStream, word: &str) -> bool {
    input
        .cursor()
        .ident()
        .is_some_and(|(ident, _)| ident == word)
}

fn parse_comparison(input: ParseStream, column: Ident, op: Comparison) -> syn::Result<PredExpr> {
    match op {
        Comparison::Op(name) => {
            let op_span = input.span();
            match name {
                "eq" => drop(input.parse::<Token![==]>()?),
                "ne" => drop(input.parse::<Token![!=]>()?),
                "le" => drop(input.parse::<Token![<=]>()?),
                "ge" => drop(input.parse::<Token![>=]>()?),
                "lt" => drop(input.parse::<Token![<]>()?),
                _ => drop(input.parse::<Token![>]>()?),
            }
            let value = parse_value(input, op_span)?;
            Ok(PredExpr::Cmp {
                column,
                op: name,
                value,
            })
        }
        Comparison::Is => {
            let is_span = input.call(Ident::parse_any)?.span();
            let negated = if peek_word(input, "not") {
                input.call(Ident::parse_any)?;
                true
            } else {
                false
            };
            if peek_word(input, "null") {
                input.call(Ident::parse_any)?;
                return Ok(PredExpr::Null { column, negated });
            }
            let value = parse_value(input, is_span)?;
            Ok(PredExpr::Cmp {
                column,
                op: if negated { "is_not" } else { "is" },
                value,
            })
        }
        Comparison::In | Comparison::NotIn => {
            let negated = matches!(op, Comparison::NotIn);
            if negated {
                input.call(Ident::parse_any)?;
            }
            let in_span = input.parse::<Token![in]>()?.span;
            let tokens = value_tokens(input);
            if tokens.is_empty() {
                return Err(syn::Error::new(
                    in_span,
                    "expected a set of values after `in`\nhelp: write `<column> in [a, b]` or `<column> in <iterator>`",
                ));
            }
            let set: Expr = syn::parse2(tokens)?;
            Ok(PredExpr::In {
                column,
                negated,
                set,
            })
        }
    }
}

/// The tokens of a value: everything up to `&&`, `||` or `else` at this level.
fn value_tokens(input: ParseStream) -> TokenStream2 {
    let mut out = TokenStream2::new();
    while !input.is_empty()
        && !input.peek(Token![&&])
        && !input.peek(Token![||])
        && !input.peek(Token![else])
    {
        match input.parse::<TokenTree>() {
            Ok(tt) => out.extend([tt]),
            Err(_) => break,
        }
    }
    out
}

fn parse_value(input: ParseStream, op_span: Span) -> syn::Result<ValueExpr> {
    let tokens = value_tokens(input);
    if tokens.is_empty() {
        return Err(syn::Error::new(
            op_span,
            "expected a value after the comparison\nhelp: write a Rust expression, for example `who.id`",
        ));
    }
    let expr: Expr = syn::parse2(tokens)?;
    let absent = if input.peek(Token![else]) {
        let else_span = input.parse::<Token![else]>()?.span;
        let word = input.call(Ident::parse_any).map_err(|_| {
            syn::Error::new(else_span, "expected `deny`, `allow` or `null` after `else`")
        })?;
        if word != "deny" && word != "allow" && word != "null" {
            return Err(syn::Error::new(
                word.span(),
                format!("`{word}` is not a meaning of absence\nhelp: write `else deny`, `else allow` or `else null`"),
            ));
        }
        Some(word)
    } else {
        None
    };
    Ok(ValueExpr { expr, absent })
}

// --- masks ------------------------------------------------------------------

fn parse_mask_tokens(tokens: TokenStream2, _span: Span) -> syn::Result<MaskExpr> {
    let parser = |input: ParseStream| {
        if input.peek(syn::token::Brace) {
            let block = input.parse()?;
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after the block"));
            }
            return Ok(MaskExpr::Block(block));
        }
        let word = input
            .call(Ident::parse_any)
            .map_err(|e| syn::Error::new(e.span(), MASK_HELP))?;
        let mask = match word.to_string().as_str() {
            "all" => MaskExpr::All,
            "none" => MaskExpr::None,
            "only" | "except" => {
                let content;
                bracketed!(content in input);
                let columns = content
                    .parse_terminated(Ident::parse_any, Token![,])?
                    .into_iter()
                    .collect();
                if word == "only" {
                    MaskExpr::Only(columns)
                } else {
                    MaskExpr::Except(columns)
                }
            }
            other => {
                return Err(syn::Error::new(
                    word.span(),
                    format!("`{other}` is not a field mask\n{MASK_HELP}"),
                ))
            }
        };
        if !input.is_empty() {
            return Err(input.error(format!(
                "unexpected tokens after the field mask\n{MASK_HELP}"
            )));
        }
        Ok(mask)
    };
    parser.parse2(tokens)
}

const MASK_HELP: &str = "help: write `all`, `none`, `only [col, …]`, `except [col, …]` or a `{ … }` block that returns a `FieldMask`";

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

fn expand(input: Input) -> syn::Result<TokenStream2> {
    let mut errors: Option<syn::Error> = None;
    let mut out = TokenStream2::new();

    let mut entities: BTreeMap<String, &EntityDecl> = BTreeMap::new();
    for entity in &input.entities {
        if entities.insert(entity.name.to_string(), entity).is_some() {
            push_error(
                &mut errors,
                syn::Error::new(
                    entity.name.span(),
                    format!(
                        "entity `{}` appears twice\nhelp: declare each entity once",
                        entity.name
                    ),
                ),
            );
        }
        out.extend(expand_entity(entity));
    }

    for policy in &input.policies {
        let Some(entity) = entities.get(&policy.entity.to_string()) else {
            push_error(
                &mut errors,
                syn::Error::new(
                    policy.entity.span(),
                    format!(
                        "entity `{0}` is not declared in this `policy!` block\nhelp: declare `entity {0} in \"table\" {{ … }}` in the same block",
                        policy.entity
                    ),
                ),
            );
            continue;
        };
        match expand_policy(policy, entity, &entities) {
            Ok(tokens) => out.extend(tokens),
            Err(e) => push_error(&mut errors, e),
        }
    }

    match errors {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

fn push_error(errors: &mut Option<syn::Error>, error: syn::Error) {
    match errors {
        Some(existing) => existing.combine(error),
        None => *errors = Some(error),
    }
}

fn has_doc(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("doc"))
}

fn column_enum(entity: &EntityDecl) -> Ident {
    format_ident!("{}Column", entity.name)
}

fn expand_entity(entity: &EntityDecl) -> TokenStream2 {
    let EntityDecl {
        attrs,
        vis,
        name,
        table,
        model,
        columns,
    } = entity;
    let column = column_enum(entity);

    let entity_doc = (!has_doc(attrs)).then(|| {
        let doc = format!("The `{}` table.", table.value());
        quote!(#[doc = #doc])
    });
    let enum_doc = format!("The columns of [`{name}`].");

    let variants = columns.iter().map(|c| {
        let attrs = &c.attrs;
        let variant = c.variant();
        let doc = (!has_doc(attrs)).then(|| {
            let doc = format!("`{}`.", c.sql_name());
            quote!(#[doc = #doc])
        });
        quote!(#doc #(#attrs)* #variant)
    });
    let all = columns.iter().map(|c| {
        let variant = c.variant();
        quote!(Self::#variant)
    });
    let names = columns.iter().map(|c| {
        let variant = c.variant();
        let sql = c.sql_name();
        quote!(Self::#variant => #sql)
    });
    let samples = columns.iter().map(|c| {
        let variant = c.variant();
        let ty = &c.ty;
        quote_spanned!(ty.span()=> Self::#variant => <#ty as ::brocanth_policy::typed::ColumnType>::samples())
    });
    let keys = columns.iter().map(|c| {
        let variant = c.variant();
        let ty = &c.ty;
        quote_spanned!(ty.span()=> Self::#variant => <#ty as ::brocanth_policy::typed::ColumnType>::key(n))
    });
    let sorts = columns.iter().map(|c| {
        let variant = c.variant();
        let ty = &c.ty;
        quote_spanned!(ty.span()=> Self::#variant => <<#ty as ::brocanth_policy::typed::ColumnType>::Kind as ::brocanth_policy::typed::Kind>::SORT)
    });
    let key = columns
        .iter()
        .find(|c| c.key)
        .map(ColumnDecl::variant)
        .expect("the parser requires one key column");

    let model_check = model.as_ref().map(|model| {
        let fields = columns.iter().map(|c| {
            let field = &c.name;
            let ty = &c.ty;
            quote_spanned!(field.span()=> let _: &#ty = &model.#field;)
        });
        quote! {
            // Each column is a field of the model, with the same type. A
            // misspelt column or a changed type is a compile error here.
            const _: () = {
                #[allow(dead_code)]
                fn __brocanth_policy_model(model: &#model) {
                    #(#fields)*
                }
            };
        }
    });

    quote! {
        #entity_doc
        #(#attrs)*
        #vis struct #name;

        #[doc = #enum_doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #vis enum #column {
            #(#variants,)*
        }

        impl ::brocanth_policy::Column for #column {
            const ALL: &'static [Self] = &[#(#all),*];

            fn name(&self) -> &'static str {
                match self {
                    #(#names,)*
                }
            }
        }

        impl ::brocanth_policy::Entity for #name {
            type Column = #column;
            const TABLE: ::brocanth_policy::Ident = ::brocanth_policy::Ident::must(#table);
            const PRIMARY_KEY: Self::Column = #column::#key;
        }

        impl ::brocanth_policy::typed::TypedColumn for #column {
            fn samples(&self) -> ::std::vec::Vec<::brocanth_policy::Value> {
                match self {
                    #(#samples,)*
                }
            }

            fn key(&self, n: i64) -> ::brocanth_policy::Value {
                match self {
                    #(#keys,)*
                }
            }
        }

        impl ::brocanth_policy::Sortable for #column {
            fn sort_kind(&self) -> ::std::option::Option<::brocanth_policy::SortKind> {
                match self {
                    #(#sorts,)*
                }
            }
        }

        #model_check
    }
}

/// Resolves column names against one entity.
struct Columns<'a> {
    entity: &'a EntityDecl,
    enum_name: Ident,
    /// Every entity of the block, for `exists`.
    entities: &'a BTreeMap<String, &'a EntityDecl>,
}

impl<'a> Columns<'a> {
    /// The columns of the link entity of an `exists`.
    fn link(&self, name: &Ident) -> syn::Result<Columns<'a>> {
        match self.entities.get(&name.to_string()) {
            Some(entity) => Ok(Columns {
                entity,
                enum_name: column_enum(entity),
                entities: self.entities,
            }),
            None => Err(syn::Error::new(
                name.span(),
                format!("entity `{name}` is not declared in this `policy!` block\nhelp: declare the link table as `entity {name} in \"table\" {{ … }}` in the same block"),
            )),
        }
    }

    fn find(&self, name: &Ident) -> syn::Result<&ColumnDecl> {
        let wanted = name.unraw().to_string();
        if let Some(column) = self.entity.columns.iter().find(|c| c.sql_name() == wanted) {
            return Ok(column);
        }
        let known: Vec<String> = self
            .entity
            .columns
            .iter()
            .map(ColumnDecl::sql_name)
            .collect();
        let nearest = known
            .iter()
            .map(|k| (distance(&wanted, k), k))
            .filter(|(d, _)| *d <= 2.max(wanted.len() / 3))
            .min();
        let help = match nearest {
            Some((_, k)) => format!("help: did you mean `{k}`?"),
            None => format!("help: use one of: {}", known.join(", ")),
        };
        Err(syn::Error::new(
            name.span(),
            format!(
                "`{wanted}` is not a column of entity `{}`\n{help}",
                self.entity.name
            ),
        ))
    }

    /// `PostColumn::AuthorId`, on the span of the author's token.
    fn path(&self, name: &Ident) -> syn::Result<TokenStream2> {
        let column = self.find(name)?;
        let enum_name = &self.enum_name;
        let variant = Ident::new(&column.variant().to_string(), name.span());
        Ok(quote_spanned!(name.span()=> #enum_name::#variant))
    }

    /// `<Option<i64> as ColumnType>::Kind`.
    fn kind(&self, name: &Ident) -> syn::Result<TokenStream2> {
        let ty = &self.find(name)?.ty;
        Ok(quote!(<#ty as ::brocanth_policy::typed::ColumnType>::Kind))
    }
}

/// Levenshtein distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = if ca == *cb {
                previous
            } else {
                1 + previous.min(row[j]).min(row[j + 1])
            };
            previous = current;
        }
    }
    row[b.len()]
}

fn expand_policy(
    policy: &PolicyDecl,
    entity: &EntityDecl,
    entities: &BTreeMap<String, &EntityDecl>,
) -> syn::Result<TokenStream2> {
    let PolicyDecl {
        attrs,
        db,
        vis,
        name,
        subject_pat,
        subject_ty,
        rules,
        masks,
        eval,
        ..
    } = policy;
    let entity_name = &entity.name;
    let columns = Columns {
        entity,
        enum_name: column_enum(entity),
        entities,
    };
    let column_enum = &columns.enum_name;

    let mut errors: Option<syn::Error> = None;
    if db.is_none() {
        for (action, rule) in rules {
            if action == "update_check" {
                push_error(
                    &mut errors,
                    syn::Error::new(
                        action.span(),
                        "`update_check` needs `#[db]`\nhelp: only the database reads `update_check`; put `#[db]` on the policy, or remove the rule (the application checks the new row with `writable`)",
                    ),
                );
            }
            if let Some(reason) = &rule.app_only {
                push_error(
                    &mut errors,
                    syn::Error::new(
                        reason.span(),
                        "`#[app_only]` needs `#[db]`\nhelp: without `#[db]`, every rule runs in the application only; remove the attribute",
                    ),
                );
            }
        }
    }

    let mut methods = Vec::new();
    for (action, rule) in rules {
        if action == "update_check" {
            // Not a `Policy` method: only `DbPolicy::rules` holds it.
            continue;
        }
        let rule_attrs = &rule.attrs;
        if db.is_some() && rule.app_only.is_none() {
            let field = if action == "view" {
                quote!(view())
            } else {
                action.to_token_stream()
            };
            methods.push(quote! {
                #(#rule_attrs)*
                fn #action(&self, subject: &#subject_ty) -> ::brocanth_policy::EntityPred<#entity_name> {
                    <Self as ::brocanth_policy::DbPolicy>::rules()
                        .#field
                        .resolve(&::brocanth_policy::Subject::attributes(subject))
                }
            });
            continue;
        }
        match expand_rule(rule, |p| expand_pred(p, &columns), quote!(::brocanth_policy::Pred::Never)) {
            Ok(body) => methods.push(quote! {
                #(#rule_attrs)*
                #[allow(unused_variables, unused_braces)]
                fn #action(&self, #subject_pat: &#subject_ty) -> ::brocanth_policy::EntityPred<#entity_name> {
                    #body
                }
            }),
            Err(e) => push_error(&mut errors, e),
        }
    }
    for (action, rule) in masks {
        let rule_attrs = &rule.attrs;
        match expand_rule(
            rule,
            |m| expand_mask(m, &columns),
            quote!(::brocanth_policy::FieldMask::deny_all()),
        ) {
            Ok(body) => methods.push(quote! {
                #(#rule_attrs)*
                #[allow(unused_variables, unused_braces)]
                fn #action(&self, #subject_pat: &#subject_ty) -> ::brocanth_policy::FieldMask<#column_enum> {
                    #body
                }
            }),
            Err(e) => push_error(&mut errors, e),
        }
    }
    if let Some(e) = errors {
        return Err(e);
    }

    let doc = (!has_doc(attrs)).then(|| {
        let doc = format!("The rules for [`{entity_name}`].");
        quote!(#[doc = #doc])
    });

    let db_impl = match db {
        Some(_) => {
            let mut cx = Db {
                who: syn::parse2::<Ident>(subject_pat.clone()).ok(),
                subject: subject_ty,
                checks: Vec::new(),
            };
            let mut fields = Vec::new();
            for (action, rule) in rules {
                match cx.rule(rule, &columns) {
                    Ok(rule) => fields.push(match action.to_string().as_str() {
                        "view" | "update_check" => {
                            quote!(rules.#action = ::std::option::Option::Some(#rule);)
                        }
                        _ => quote!(rules.#action = #rule;),
                    }),
                    Err(e) => push_error(&mut errors, e),
                }
            }
            let checks = &cx.checks;
            Some(quote! {
                impl ::brocanth_policy::DbPolicy for #name {
                    #[allow(clippy::clone_on_copy, clippy::redundant_clone, unused_variables)]
                    fn rules() -> ::brocanth_policy::Rules<#entity_name> {
                        // The types the application expansion checks: each
                        // `who.<field>` against its column, each guard is a
                        // `bool`. Never called.
                        let _ = |#subject_pat: &#subject_ty| {
                            #(#checks)*
                        };
                        let mut rules = ::brocanth_policy::Rules::<#entity_name>::deny_all();
                        #(#fields)*
                        rules
                    }
                }
            })
        }
        None => None,
    };
    if let Some(e) = errors {
        return Err(e);
    }

    let test = eval.as_ref().map(|eval| expand_eval(eval, name));

    Ok(quote! {
        #doc
        #(#attrs)*
        #vis struct #name;

        impl ::brocanth_policy::Policy for #name {
            type Entity = #entity_name;
            type Subject = #subject_ty;

            #(#methods)*
        }

        #db_impl

        #test
    })
}

fn expand_rule<T>(
    rule: &Rule<T>,
    item: impl Fn(&T) -> syn::Result<TokenStream2>,
    default: TokenStream2,
) -> syn::Result<TokenStream2> {
    let mut out = TokenStream2::new();
    for (cond, then) in &rule.arms {
        let then = item(then)?;
        out.extend(quote! {
            if #cond {
                return #then;
            }
        });
    }
    match &rule.fallback {
        Some(fallback) => out.extend(item(fallback)?),
        None => out.extend(default),
    }
    Ok(out)
}

fn expand_pred(pred: &PredExpr, columns: &Columns) -> syn::Result<TokenStream2> {
    Ok(match pred {
        PredExpr::Always => quote!(::brocanth_policy::Pred::Always),
        PredExpr::Never => quote!(::brocanth_policy::Pred::Never),
        PredExpr::Cmp { column, op, value } => {
            let path = columns.path(column)?;
            let kind = columns.kind(column)?;
            let op = Ident::new(op, column.span());
            let bound = bound(&kind, &value.expr.to_token_stream(), value);
            quote!(::brocanth_policy::Pred::#op(#path, #bound))
        }
        PredExpr::Null { column, negated } => {
            let path = columns.path(column)?;
            if *negated {
                quote!(::brocanth_policy::Pred::is_not_null(#path))
            } else {
                quote!(::brocanth_policy::Pred::is_null(#path))
            }
        }
        PredExpr::In {
            column,
            negated,
            set,
        } => {
            let path = columns.path(column)?;
            let kind = columns.kind(column)?;
            let span = set.span();
            let values = quote_spanned!(span=> ::brocanth_policy::typed::set::<#kind, _>(#set));
            if *negated {
                quote!(::brocanth_policy::Pred::none_of(#path, #values))
            } else {
                quote!(::brocanth_policy::Pred::one_of(#path, #values))
            }
        }
        PredExpr::All(parts) => {
            let parts = parts
                .iter()
                .map(|p| expand_pred(p, columns))
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(::brocanth_policy::Pred::all([#(#parts),*]))
        }
        PredExpr::Any(parts) => {
            let parts = parts
                .iter()
                .map(|p| expand_pred(p, columns))
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(::brocanth_policy::Pred::any([#(#parts),*]))
        }
        PredExpr::Not(inner) => {
            let inner = expand_pred(inner, columns)?;
            quote!(::brocanth_policy::Pred::negate(#inner))
        }
        PredExpr::Exists(exists) => expand_exists(exists, columns)?,
        PredExpr::Block(block) => block.to_token_stream(),
    })
}

/// The `Bound` of a value, with its type checked against the column kind:
/// `typed::bind`, or `typed::absent` with the word after `else`. `expr` is
/// the value as written, or a stand-in on the same span.
fn bound(kind: &TokenStream2, expr: &TokenStream2, value: &ValueExpr) -> TokenStream2 {
    let span = value.expr.span();
    match &value.absent {
        None => quote_spanned!(span=> ::brocanth_policy::typed::bind::<#kind, _>(#expr)),
        Some(word) => {
            let missing = Ident::new(&word.to_string().to_upper_camel_case(), word.span());
            quote_spanned!(span=> ::brocanth_policy::typed::absent::<#kind, _>(#expr, ::brocanth_policy::Missing::#missing))
        }
    }
}

/// `Op::Eq` for `"eq"`, `Op::IsNot` for `"is_not"`.
fn op_variant(op: &str, span: Span) -> Ident {
    Ident::new(&op.to_upper_camel_case(), span)
}

/// The link entity, its columns, and the two linked columns of an `exists`,
/// with a check that both hold the same kind of value.
struct Link<'a> {
    columns: Columns<'a>,
    local: TokenStream2,
    foreign: String,
    same_kind: TokenStream2,
}

fn link<'a>(exists: &ExistsExpr, columns: &Columns<'a>) -> syn::Result<Link<'a>> {
    let link = columns.link(&exists.entity)?;
    let local = columns.path(&exists.local)?;
    let local_kind = columns.kind(&exists.local)?;
    let foreign = link.find(&exists.foreign)?.sql_name();
    let foreign_kind = link.kind(&exists.foreign)?;
    let span = exists.local.span();
    let same_kind = quote_spanned! {span=>
        let _: ::std::marker::PhantomData<#local_kind> = ::std::marker::PhantomData::<#foreign_kind>;
    };
    Ok(Link {
        columns: link,
        local,
        foreign,
        same_kind,
    })
}

/// The `Membership` the hand-written form builds: `Membership::new`, then
/// `filter_by` for each filter, `negate` for `!exists`, and `Pred::Never`
/// when a `filter_by` refuses an absent `else deny` value.
fn expand_exists(exists: &ExistsExpr, columns: &Columns) -> syn::Result<TokenStream2> {
    let Link {
        columns: link,
        local,
        foreign,
        same_kind,
    } = link(exists, columns)?;
    let table = &exists.entity;
    let mut chain = quote! {
        ::std::option::Option::Some(::brocanth_policy::Membership::new(
            #local,
            <#table as ::brocanth_policy::Entity>::TABLE,
            ::brocanth_policy::Ident::must(#foreign),
        ))
    };
    for filter in &exists.filter {
        let (column, op, bound) = match filter {
            PredExpr::Cmp { column, op, value } => {
                let kind = link.kind(column)?;
                (
                    column,
                    op_variant(op, column.span()),
                    bound(&kind, &value.expr.to_token_stream(), value),
                )
            }
            PredExpr::Null { column, negated } => (
                column,
                op_variant(if *negated { "is_not" } else { "is" }, column.span()),
                quote!(::brocanth_policy::Value::Null),
            ),
            _ => unreachable!("the parser keeps comparisons only"),
        };
        let sql = link.find(column)?.sql_name();
        chain = quote! {
            #chain.and_then(|membership| membership.filter_by(
                ::brocanth_policy::Ident::must(#sql),
                ::brocanth_policy::Op::#op,
                #bound,
            ))
        };
    }
    if exists.negated {
        chain = quote!(#chain.map(::brocanth_policy::Membership::negate));
    }
    Ok(quote! {
        {
            #same_kind
            #chain.map_or(::brocanth_policy::Pred::Never, ::brocanth_policy::Membership::into_pred)
        }
    })
}

// --- #[db]: the rules as data -----------------------------------------------

const HELP_BLOCK: &str =
    "help: write the rule in the policy! syntax, or mark it #[app_only(\"reason\")]";
const HELP_GUARD: &str = "help: add the value to the subject as a #[attr] field and test that";
const HELP_WHO: &str =
    "help: compute the value when you build the subject, and store it in a field";
const HELP_IS: &str =
    "help: use == with else deny; IS NOT DISTINCT FROM matches NULL to a missing setting";
const HELP_IN: &str = "help: a setting holds one value; use a membership table and exists";
const HELP_ALLOW: &str = "help: put the fact in a bool #[attr] field and guard on it";
const HELP_LITERAL: &str = "help: the database policy holds the value written at `rls make`; use a literal or a constant, or store the value in a #[attr] field of the subject";

/// Builds `DbPolicy::rules` for a `#[db]` policy, and refuses the shapes of
/// docs/policy-rls-bridge.md, decision 3.
struct Db<'a> {
    /// The subject parameter, unless it is `_`.
    who: Option<Ident>,
    subject: &'a Type,
    /// Statements that check types the way the application expansion does.
    /// They go in a closure that never runs.
    checks: Vec<TokenStream2>,
}

impl Db<'_> {
    /// A `Rule<Column>` expression.
    fn rule(&mut self, rule: &Rule<PredExpr>, columns: &Columns) -> syn::Result<TokenStream2> {
        if let Some(reason) = &rule.app_only {
            return Ok(quote!(::brocanth_policy::Rule::app_only(#reason)));
        }
        let mut arms = Vec::new();
        for (guard, then) in &rule.arms {
            let guard = self.guard(guard)?;
            let then = self.term(then, columns)?;
            arms.push(quote!(.arm(#guard, #then)));
        }
        let fallback = match &rule.fallback {
            Some(fallback) => self.term(fallback, columns)?,
            None => quote!(::brocanth_policy::Term::Never),
        };
        Ok(quote!(::brocanth_policy::Rule::new(#fallback) #(#arms)*))
    }

    /// The field, when `expr` is exactly `who.<field>`.
    fn who_field<'e>(&self, expr: &'e Expr) -> Option<&'e Ident> {
        let who = self.who.as_ref()?;
        match expr {
            Expr::Paren(inner) => self.who_field(&inner.expr),
            Expr::Group(inner) => self.who_field(&inner.expr),
            Expr::Field(field) if field.attrs.is_empty() => match (&*field.base, &field.member) {
                (Expr::Path(base), syn::Member::Named(name))
                    if base.qself.is_none() && base.path.is_ident(who) =>
                {
                    Some(name)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// `true` when the tokens name the subject anywhere.
    fn uses_who(&self, tokens: TokenStream2) -> bool {
        let Some(who) = &self.who else {
            return false;
        };
        tokens.into_iter().any(|tt| match tt {
            TokenTree::Ident(ident) => ident == *who,
            TokenTree::Group(group) => self.uses_who(group.stream()),
            _ => false,
        })
    }

    /// The attribute's key, from the derive:
    /// `<Visitor>::BROCANTH_ATTRIBUTES.f`. A field without `#[attr]` has
    /// none, and does not compile.
    fn key(&self, field: &Ident) -> TokenStream2 {
        let subject = self.subject;
        let keys = quote_spanned!(field.span()=> <#subject>::BROCANTH_ATTRIBUTES);
        quote_spanned!(field.span()=> #keys.#field)
    }

    /// `who.<field>`, on the field's span.
    fn read(&self, field: &Ident) -> TokenStream2 {
        let who = self.who.as_ref().expect("who_field found the subject");
        quote_spanned!(field.span()=> #who.#field)
    }

    fn guard(&mut self, expr: &Expr) -> syn::Result<TokenStream2> {
        let refuse = |help: &str| {
            Err(syn::Error::new(
                expr.span(),
                format!("this guard cannot become a database policy\n{help}"),
            ))
        };
        match expr {
            Expr::Paren(inner) => self.guard(&inner.expr),
            Expr::Group(inner) => self.guard(&inner.expr),
            Expr::Block(_) | Expr::Unsafe(_) => refuse(HELP_BLOCK),
            Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Not(_)) => {
                let inner = self.guard(&unary.expr)?;
                Ok(quote!(::brocanth_policy::Guard::Not(::std::boxed::Box::new(#inner))))
            }
            Expr::Binary(binary) if matches!(binary.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) => {
                let and = matches!(binary.op, syn::BinOp::And(_));
                let mut parts = Vec::new();
                self.flatten(expr, and, &mut parts)?;
                Ok(if and {
                    quote!(::brocanth_policy::Guard::All(::std::vec![#(#parts),*]))
                } else {
                    quote!(::brocanth_policy::Guard::Any(::std::vec![#(#parts),*]))
                })
            }
            Expr::Binary(binary) if matches!(binary.op, syn::BinOp::Eq(_) | syn::BinOp::Ne(_)) => {
                let Some(field) = self.who_field(&binary.left) else {
                    return refuse(HELP_GUARD);
                };
                let right = &binary.right;
                if self.uses_who(right.to_token_stream()) || !is_literal(right) {
                    return refuse(HELP_GUARD);
                }
                let read = self.read(field);
                self.checks
                    .push(quote_spanned!(expr.span()=> let _: bool = #read == #right;));
                let key = self.key(field);
                let eq = quote!(::brocanth_policy::Guard::Eq(#key, ::brocanth_policy::Value::from(#right)));
                Ok(if matches!(binary.op, syn::BinOp::Ne(_)) {
                    quote!(::brocanth_policy::Guard::Not(::std::boxed::Box::new(#eq)))
                } else {
                    eq
                })
            }
            _ => match self.who_field(expr) {
                Some(field) => {
                    let read = self.read(field);
                    self.checks
                        .push(quote_spanned!(expr.span()=> let _: bool = #read;));
                    let key = self.key(field);
                    Ok(quote!(::brocanth_policy::Guard::Attr(#key)))
                }
                None => refuse(HELP_GUARD),
            },
        }
    }

    /// The parts of `a && b && c` (or `||`), each a guard.
    fn flatten(&mut self, expr: &Expr, and: bool, out: &mut Vec<TokenStream2>) -> syn::Result<()> {
        match expr {
            Expr::Binary(binary)
                if (and && matches!(binary.op, syn::BinOp::And(_)))
                    || (!and && matches!(binary.op, syn::BinOp::Or(_))) =>
            {
                self.flatten(&binary.left, and, out)?;
                self.flatten(&binary.right, and, out)
            }
            other => {
                out.push(self.guard(other)?);
                Ok(())
            }
        }
    }

    /// A `Term<Column>` expression.
    fn term(&mut self, pred: &PredExpr, columns: &Columns) -> syn::Result<TokenStream2> {
        Ok(match pred {
            PredExpr::Always => quote!(::brocanth_policy::Term::Always),
            PredExpr::Never => quote!(::brocanth_policy::Term::Never),
            PredExpr::Cmp { column, op, value } => {
                let path = columns.path(column)?;
                let kind = columns.kind(column)?;
                let operand = self.operand(op, value, &kind)?;
                let op = op_variant(op, column.span());
                quote! {
                    ::brocanth_policy::Term::Cmp {
                        column: #path,
                        op: ::brocanth_policy::Op::#op,
                        operand: #operand,
                    }
                }
            }
            PredExpr::Null { column, negated } => {
                let path = columns.path(column)?;
                let op = op_variant(if *negated { "is_not" } else { "is" }, column.span());
                quote! {
                    ::brocanth_policy::Term::Cmp {
                        column: #path,
                        op: ::brocanth_policy::Op::#op,
                        operand: ::brocanth_policy::Operand::Value(::brocanth_policy::Value::Null),
                    }
                }
            }
            PredExpr::In {
                column,
                negated,
                set,
            } => {
                let path = columns.path(column)?;
                let kind = columns.kind(column)?;
                if self.uses_who(set.to_token_stream()) {
                    return Err(syn::Error::new(
                        set.span(),
                        format!(
                            "a set of subject values cannot become a database policy\n{HELP_IN}"
                        ),
                    ));
                }
                if !is_literal_set(set) {
                    return Err(syn::Error::new(
                        set.span(),
                        format!("this set is not a list of literals\n{HELP_LITERAL}"),
                    ));
                }
                let span = set.span();
                quote_spanned! {span=>
                    ::brocanth_policy::Term::In {
                        column: #path,
                        values: ::brocanth_policy::typed::set::<#kind, _>(#set),
                        negated: #negated,
                    }
                }
            }
            PredExpr::All(parts) | PredExpr::Any(parts) => {
                let parts = parts
                    .iter()
                    .map(|p| self.term(p, columns))
                    .collect::<syn::Result<Vec<_>>>()?;
                if matches!(pred, PredExpr::All(_)) {
                    quote!(::brocanth_policy::Term::All(::std::vec![#(#parts),*]))
                } else {
                    quote!(::brocanth_policy::Term::Any(::std::vec![#(#parts),*]))
                }
            }
            PredExpr::Not(inner) => {
                let inner = self.term(inner, columns)?;
                quote!(::brocanth_policy::Term::Not(::std::boxed::Box::new(#inner)))
            }
            PredExpr::Exists(exists) => self.exists(exists, columns)?,
            PredExpr::Block(block) => {
                return Err(syn::Error::new(
                    block.span(),
                    format!("a `{{ … }}` block cannot become a database policy\n{HELP_BLOCK}"),
                ))
            }
        })
    }

    /// An `Operand` expression: `who.<field>` with its absence mode, or a
    /// literal checked against the column kind.
    fn operand(
        &mut self,
        op: &str,
        value: &ValueExpr,
        kind: &TokenStream2,
    ) -> syn::Result<TokenStream2> {
        let expr = &value.expr;
        if let Some(field) = self.who_field(expr) {
            if op == "is" || op == "is_not" {
                return Err(syn::Error::new(
                    expr.span(),
                    format!("`is` with a subject value cannot become a database policy\n{HELP_IS}"),
                ));
            }
            let missing = match &value.absent {
                None => Ident::new("Deny", expr.span()),
                Some(word) if word == "allow" => {
                    return Err(syn::Error::new(
                        word.span(),
                        format!("`else allow` cannot become a database policy: a missing setting would open the table\n{HELP_ALLOW}"),
                    ))
                }
                Some(word) => Ident::new(&word.to_string().to_upper_camel_case(), word.span()),
            };
            // The type check of the application expansion, on a copy.
            let read = self.read(field);
            let copy = quote_spanned!(expr.span()=> ::std::clone::Clone::clone(&#read));
            let checked = bound(kind, &copy, value);
            self.checks.push(quote!(let _ = #checked;));
            let key = self.key(field);
            return Ok(quote! {
                ::brocanth_policy::Operand::Attr {
                    key: #key,
                    missing: ::brocanth_policy::Missing::#missing,
                }
            });
        }
        if self.uses_who(expr.to_token_stream()) {
            return Err(syn::Error::new(
                expr.span(),
                format!(
                    "a value computed from the subject cannot become a database policy\n{HELP_WHO}"
                ),
            ));
        }
        if let Some(word) = &value.absent {
            return Err(syn::Error::new(
                word.span(),
                format!("`else {word}` on a value that is never absent\nhelp: `else` says what a missing subject value means; remove it, or compare with `who.<field>`"),
            ));
        }
        if !is_literal(expr) {
            return Err(syn::Error::new(
                expr.span(),
                format!("this value is not a literal\n{HELP_LITERAL}"),
            ));
        }
        let span = expr.span();
        Ok(quote_spanned! {span=>
            ::brocanth_policy::Operand::Value(
                <_ as ::brocanth_policy::typed::Operand<#kind>>::into_value(#expr),
            )
        })
    }

    fn exists(&mut self, exists: &ExistsExpr, columns: &Columns) -> syn::Result<TokenStream2> {
        let Link {
            columns: link,
            local,
            foreign,
            same_kind,
        } = link(exists, columns)?;
        self.checks.push(same_kind);
        let mut filter = Vec::new();
        for item in &exists.filter {
            let (column, op, operand) = match item {
                PredExpr::Cmp { column, op, value } => {
                    let kind = link.kind(column)?;
                    let operand = self.operand(op, value, &kind)?;
                    (column, op_variant(op, column.span()), operand)
                }
                PredExpr::Null { column, negated } => (
                    column,
                    op_variant(if *negated { "is_not" } else { "is" }, column.span()),
                    quote!(::brocanth_policy::Operand::Value(
                        ::brocanth_policy::Value::Null
                    )),
                ),
                _ => unreachable!("the parser keeps comparisons only"),
            };
            let sql = link.find(column)?.sql_name();
            filter.push(quote! {
                (::brocanth_policy::Ident::must(#sql), ::brocanth_policy::Op::#op, #operand)
            });
        }
        let table = &exists.entity;
        let negated = exists.negated;
        Ok(quote! {
            ::brocanth_policy::Term::Exists(::std::boxed::Box::new(::brocanth_policy::Exists {
                local: #local,
                table: <#table as ::brocanth_policy::Entity>::TABLE,
                foreign: ::brocanth_policy::Ident::must(#foreign),
                filter: ::std::vec![#(#filter),*],
                negated: #negated,
            }))
        })
    }
}

/// A value the database policy can hold as written: a literal, a negative
/// literal, or a constant's path. Not a call, a field or a block.
fn is_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) => true,
        Expr::Unary(unary) => matches!(unary.op, syn::UnOp::Neg(_)) && is_literal(&unary.expr),
        Expr::Path(path) => path.qself.is_none(),
        Expr::Paren(inner) => is_literal(&inner.expr),
        Expr::Group(inner) => is_literal(&inner.expr),
        _ => false,
    }
}

/// `[a, b]`, `[] as [T; 0]` or `&[a, b]`, each element a literal.
fn is_literal_set(expr: &Expr) -> bool {
    match expr {
        Expr::Array(array) => array.elems.iter().all(is_literal),
        Expr::Cast(cast) => is_literal_set(&cast.expr),
        Expr::Reference(reference) => is_literal_set(&reference.expr),
        Expr::Paren(inner) => is_literal_set(&inner.expr),
        Expr::Group(inner) => is_literal_set(&inner.expr),
        _ => false,
    }
}

fn expand_mask(mask: &MaskExpr, columns: &Columns) -> syn::Result<TokenStream2> {
    let list = |names: &[Ident]| {
        names
            .iter()
            .map(|n| columns.path(n))
            .collect::<syn::Result<Vec<_>>>()
    };
    Ok(match mask {
        MaskExpr::All => quote!(::brocanth_policy::FieldMask::all()),
        MaskExpr::None => quote!(::brocanth_policy::FieldMask::deny_all()),
        MaskExpr::Only(names) => {
            let names = list(names)?;
            quote!(::brocanth_policy::FieldMask::only([#(#names),*]))
        }
        MaskExpr::Except(names) => {
            let names = list(names)?;
            quote!(::brocanth_policy::FieldMask::except([#(#names),*]))
        }
        MaskExpr::Block(block) => block.to_token_stream(),
    })
}

fn expand_eval(eval: &EvalClause, policy: &Ident) -> TokenStream2 {
    let EvalClause {
        span,
        subjects,
        database,
        dialect,
        cases,
    } = eval;
    let module = format_ident!(
        "__brocanth_policy_eval_{}",
        policy.to_string().to_snake_case()
    );
    let cases = match cases {
        Some(lit) => lit.to_token_stream(),
        None => quote!(64),
    };
    let test = Ident::new("eval_agrees_with_the_query", *span);
    quote! {
        #[cfg(test)]
        mod #module {
            #[allow(unused_imports)]
            use super::*;

            /// PLAN.md §9 obligation 1: `eval(row) == (row ∈ query(condition))`
            /// for every rule of this policy.
            #[test]
            fn #test() {
                let subjects = #subjects;
                if let ::std::result::Result::Err(failure) =
                    ::brocanth_policy::agreement::policy_agrees(
                        &super::#policy,
                        ::std::convert::AsRef::<[_]>::as_ref(&subjects),
                        ::brocanth_policy::Dialect::#dialect,
                        #cases,
                        #database,
                    )
                {
                    ::std::panic!("eval() and condition() disagree for `{}`:\n{}", ::std::stringify!(#policy), failure);
                }
            }
        }
    }
}
