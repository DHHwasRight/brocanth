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
//!   `impl Column for PostColumn`, `impl Entity for Post`, and
//!   `impl typed::TypedColumn for PostColumn`.
//! * `policy PostPolicy(who: &Visitor) for Post { … }` emits
//!   `struct PostPolicy` and `impl Policy for PostPolicy`. Each rule is the
//!   method of the same name. A guard `cond => pred,` is
//!   `if cond { return pred; }`. A comparison is the `Pred` constructor of
//!   the same name, with its value passed through `typed::bind`, which
//!   checks the type and returns the same `Bound` as `value.into()`.
//! * `eval { … }` emits one `#[cfg(test)]` test that calls
//!   `agreement::policy_agrees` (PLAN.md §9 obligation 1).
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
///         update {
///             who.admin => always,
///             author_id == who.user_id else deny,
///         }
///         writable {
///             who.admin => except [id],
///             only [title],
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
    vis: Visibility,
    name: Ident,
    subject_pat: TokenStream2,
    subject_ty: Type,
    entity: Ident,
    rules: Vec<(Ident, Rule<PredExpr>)>,
    writable: Option<(Ident, Rule<MaskExpr>)>,
    eval: Option<EvalClause>,
}

/// Guards, then a fallback. `cond => then,` is `if cond { return then; }`.
struct Rule<T> {
    /// Doc comments and other attributes, put on the method.
    attrs: Vec<Attribute>,
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
    Block(syn::Block),
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

const RULES: &str = "list, view, create, update, delete, writable, eval";

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
    let mut writable = None;
    let mut eval = None;
    while !content.is_empty() {
        let attrs = content.call(Attribute::parse_outer)?;
        let rule: Ident = content.call(Ident::parse_any)?;
        let label = rule.to_string();
        let seen = rules.iter().any(|(r, _)| *r == rule)
            || (label == "writable" && writable.is_some())
            || (label == "eval" && eval.is_some());
        if seen {
            return Err(syn::Error::new(
                rule.span(),
                format!("rule `{label}` appears twice in policy `{name}`\nhelp: write each rule once; use `||` to join two predicates"),
            ));
        }
        match label.as_str() {
            "list" | "view" | "create" | "update" | "delete" => {
                let body = parse_rule(&content, &rule, attrs, parse_pred_tokens)?;
                rules.push((rule, body));
            }
            "writable" => {
                let body = parse_rule(&content, &rule, attrs, parse_mask_tokens)?;
                writable = Some((rule, body));
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
        vis,
        name,
        subject_pat,
        subject_ty,
        entity,
        rules,
        writable,
        eval,
    })
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
                if ident != "Postgres" && ident != "Sqlite" {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("`{ident}` is not a dialect\nhelp: use `Postgres` or `Sqlite`"),
                    ));
                }
                dialect = Some(ident);
            }
            "cases" => cases = Some(syn::parse2::<LitInt>(tokens)?),
            other => {
                return Err(syn::Error::new(
                    field.span(),
                    format!("`{other}` is not an `eval` field\nhelp: use `subjects`, `database`, `dialect` and optionally `cases`"),
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
        dialect: dialect.ok_or_else(|| missing("dialect", "write `dialect: Postgres` or `dialect: Sqlite`"))?,
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
        input.parse::<Token![!]>()?;
        return Ok(PredExpr::Not(Box::new(parse_unary(input)?)));
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
            "expected a predicate\nhelp: write `always`, `never`, `<column> <op> <value>`, `!`, `( … )` or a `{ … }` block that returns a `Pred`",
        )
    })?;
    if let Some(op) = comparison(input) {
        return parse_comparison(input, word, op);
    }
    match word.to_string().as_str() {
        "always" => Ok(PredExpr::Always),
        "never" => Ok(PredExpr::Never),
        _ => Err(syn::Error::new(
            word.span(),
            format!("expected a comparison after column `{word}`\nhelp: use `==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `is not`, `in` or `not in`"),
        )),
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
        match expand_policy(policy, entity) {
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

        #model_check
    }
}

/// Resolves column names against one entity.
struct Columns<'a> {
    entity: &'a EntityDecl,
    enum_name: Ident,
}

impl Columns<'_> {
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

fn expand_policy(policy: &PolicyDecl, entity: &EntityDecl) -> syn::Result<TokenStream2> {
    let PolicyDecl {
        attrs,
        vis,
        name,
        subject_pat,
        subject_ty,
        rules,
        writable,
        eval,
        ..
    } = policy;
    let entity_name = &entity.name;
    let columns = Columns {
        entity,
        enum_name: column_enum(entity),
    };
    let column_enum = &columns.enum_name;

    let mut errors: Option<syn::Error> = None;
    let mut methods = Vec::new();
    for (action, rule) in rules {
        let rule_attrs = &rule.attrs;
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
    if let Some((action, rule)) = writable {
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
            let expr = &value.expr;
            let span = expr.span();
            let bound = match &value.absent {
                None => quote_spanned!(span=> ::brocanth_policy::typed::bind::<#kind, _>(#expr)),
                Some(word) => {
                    let missing = Ident::new(&word.to_string().to_upper_camel_case(), word.span());
                    quote_spanned!(span=> ::brocanth_policy::typed::absent::<#kind, _>(#expr, ::brocanth_policy::Missing::#missing))
                }
            };
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
        PredExpr::Block(block) => block.to_token_stream(),
    })
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
