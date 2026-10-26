//! The grammar of `routes!`.
//!
//! Every rule reads as a line a person would write on a whiteboard: a verb, a
//! path, an arrow, a handler. The parser's only job is to turn that into an
//! [`Item`] tree with spans attached, so that every message the expansion
//! produces points at the word that caused it rather than at the macro.

use brocanth::routing::Verb;
use proc_macro2::Span;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, bracketed, Expr, Ident, LitBool, LitStr, Path, Token};

/// The words the grammar reserves. They are keywords only here, so a project
/// may still have a module called `scope` or a handler called `get`.
pub mod kw {
    syn::custom_keyword!(root);
    syn::custom_keyword!(get);
    syn::custom_keyword!(head);
    syn::custom_keyword!(post);
    syn::custom_keyword!(put);
    syn::custom_keyword!(patch);
    syn::custom_keyword!(delete);
    syn::custom_keyword!(options);
    syn::custom_keyword!(scope);
    syn::custom_keyword!(namespace);
    syn::custom_keyword!(resources);
    syn::custom_keyword!(resource);
    syn::custom_keyword!(member);
    syn::custom_keyword!(collection);
    syn::custom_keyword!(only);
    syn::custom_keyword!(except);
    syn::custom_keyword!(param);
    syn::custom_keyword!(shallow);
}

/// The whole macro body.
pub struct Routes {
    /// The items, in the order they were written. Registration order is what
    /// `Router::routes()` lists, so it is kept.
    pub items: Vec<Item>,
}

/// One line, or one block, of the table.
pub enum Item {
    /// `root => h;` or `get "/about" => h;`
    Route(Route),
    /// `scope "/admin" { .. }` or `namespace admin { .. }`
    Group(Group),
    /// `resources "/posts" => ctrl;` or `resource "/profile" => ctrl;`
    Resource(Resource),
    /// `member { .. }` or `collection { .. }`, inside a resource block.
    Block(Block),
}

/// One route and the function that answers it.
pub struct Route {
    /// The verb this line names. `root` means GET.
    pub verb: Verb,
    /// The path, relative to the enclosing prefix.
    pub path: String,
    /// Where the path was written.
    pub path_span: Span,
    /// What goes in the table.
    pub handler: Expr,
    /// `as: name`, the helper module this route gets.
    pub name: Option<Ident>,
}

/// How a group spells its prefix.
///
/// The two produce the same `Group`: `namespace admin` is `scope "/admin"`
/// written as one word. They differ only in what they can say -- a scope
/// takes a path, so it can nest several segments at once and can carry a
/// parameter; a namespace takes an identifier, so its prefix and its helper
/// module cannot drift apart.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// `scope "/admin"`.
    Scope,
    /// `namespace admin`.
    Namespace,
}

/// `scope` or `namespace`, and what they hold.
pub struct Group {
    /// The prefix this group adds, always starting with `/`.
    pub path: String,
    /// Where the prefix was written.
    pub path_span: Span,
    /// `as: name`, overriding the helper module the group contributes.
    pub name: Option<Ident>,
    /// The items inside the braces.
    pub items: Vec<Item>,
}

/// A REST resource.
pub struct Resource {
    /// `resource` rather than `resources`: one of a thing, so no `index` and
    /// no `{id}`.
    pub singular: bool,
    /// The path, relative to the enclosing prefix.
    pub path: String,
    /// Where the path was written.
    pub path_span: Span,
    /// The module whose seven functions answer the seven routes.
    pub controller: Path,
    /// Where the controller was written, for the errors about its functions.
    pub controller_span: Span,
    /// `only: [..]`.
    pub only: Option<(Vec<Action>, Span)>,
    /// `except: [..]`.
    pub except: Option<(Vec<Action>, Span)>,
    /// `param: slug`, the member parameter name. `id` by default.
    pub param: Option<Ident>,
    /// `as: name`, the helper module this resource gets.
    pub name: Option<Ident>,
    /// `shallow: true`: resources nested in this one hang their member routes
    /// off the parent's parent.
    pub shallow: bool,
    /// The items inside the braces, if any.
    pub items: Vec<Item>,
    /// Where the resource was declared.
    pub span: Span,
}

/// Whether a block hangs off one of the collection or one of its members.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// `member { .. }`: under `/posts/{id}`.
    Member,
    /// `collection { .. }`: under `/posts`.
    Collection,
}

/// `member { .. }` or `collection { .. }`.
pub struct Block {
    /// Which of the two was written.
    pub kind: BlockKind,
    /// Where it was written, for the error when it is not inside a resource.
    pub span: Span,
    /// The items inside the braces.
    pub items: Vec<Item>,
}

/// One of the seven things a resource does.
///
/// A closed list, so `only: [lsit]` is a compile error naming the seven
/// rather than a resource that quietly loses a route.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// `GET /posts`
    Index,
    /// `GET /posts/new`
    New,
    /// `POST /posts`
    Create,
    /// `GET /posts/{id}`
    Show,
    /// `GET /posts/{id}/edit`
    Edit,
    /// `PATCH` and `PUT /posts/{id}`
    Update,
    /// `DELETE /posts/{id}`
    Destroy,
}

impl Action {
    /// The seven, in the order a resource declares them.
    pub const ALL: [Self; 7] = [
        Self::Index,
        Self::New,
        Self::Create,
        Self::Show,
        Self::Edit,
        Self::Update,
        Self::Destroy,
    ];

    /// The function this action calls on the controller.
    pub const fn function(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::New => "new",
            Self::Create => "create",
            Self::Show => "show",
            Self::Edit => "edit",
            Self::Update => "update",
            Self::Destroy => "destroy",
        }
    }

    fn from_ident(ident: &Ident) -> syn::Result<Self> {
        let found = Self::ALL
            .into_iter()
            .find(|action| ident == action.function());
        found.ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|a| a.function()).collect();
            syn::Error::new(
                ident.span(),
                format!(
                    "`{ident}` is not an action of a resource. The seven are {}",
                    names.join(", ")
                ),
            )
        })
    }
}

impl Parse for Routes {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut items = Vec::new();
        while !input.is_empty() {
            items.push(input.parse()?);
        }
        Ok(Self { items })
    }
}

impl Parse for Item {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let lookahead = input.lookahead1();
        if lookahead.peek(kw::root) {
            let token: kw::root = input.parse()?;
            return Ok(Self::Route(route(input, Verb::Get, "/", token.span)?));
        }
        for (peeked, verb) in [
            (lookahead.peek(kw::get), Verb::Get),
            (lookahead.peek(kw::head), Verb::Head),
            (lookahead.peek(kw::post), Verb::Post),
            (lookahead.peek(kw::put), Verb::Put),
            (lookahead.peek(kw::patch), Verb::Patch),
            (lookahead.peek(kw::delete), Verb::Delete),
            (lookahead.peek(kw::options), Verb::Options),
        ] {
            if peeked {
                // The keyword and the path are two tokens; the path carries
                // the span worth reporting, so the keyword is dropped.
                input.parse::<Ident>()?;
                let literal: LitStr = input.parse()?;
                return Ok(Self::Route(route(
                    input,
                    verb,
                    &literal.value(),
                    literal.span(),
                )?));
            }
        }
        if lookahead.peek(kw::scope) {
            return Ok(Self::Group(group(input, GroupKind::Scope)?));
        }
        if lookahead.peek(kw::namespace) {
            return Ok(Self::Group(group(input, GroupKind::Namespace)?));
        }
        if lookahead.peek(kw::resources) {
            return Ok(Self::Resource(resource(input, false)?));
        }
        if lookahead.peek(kw::resource) {
            return Ok(Self::Resource(resource(input, true)?));
        }
        if lookahead.peek(kw::member) {
            let token: kw::member = input.parse()?;
            return Ok(Self::Block(block(input, BlockKind::Member, token.span)?));
        }
        if lookahead.peek(kw::collection) {
            let token: kw::collection = input.parse()?;
            return Ok(Self::Block(block(
                input,
                BlockKind::Collection,
                token.span,
            )?));
        }
        Err(lookahead.error())
    }
}

/// `=> handler [, as: name];`
fn route(input: ParseStream<'_>, verb: Verb, path: &str, path_span: Span) -> syn::Result<Route> {
    input.parse::<Token![=>]>()?;
    let handler: Expr = input.parse()?;
    let options = options(input)?;
    input.parse::<Token![;]>()?;
    options.reject_resource_only("a single route")?;
    Ok(Route {
        verb,
        path: path.to_owned(),
        path_span,
        handler,
        name: options.name,
    })
}

fn group(input: ParseStream<'_>, kind: GroupKind) -> syn::Result<Group> {
    input.parse::<Ident>()?;
    // `scope` takes a path because a prefix is a path; `namespace` takes an
    // identifier because its whole point is that the prefix and the helper
    // module are the same word.
    let (path, path_span, default_name) = match kind {
        GroupKind::Scope => {
            let literal: LitStr = input.parse()?;
            (literal.value(), literal.span(), None)
        }
        GroupKind::Namespace => {
            let ident: Ident = input.parse()?;
            (format!("/{ident}"), ident.span(), Some(ident))
        }
    };
    let options = options(input)?;
    options.reject_resource_only("a group")?;
    let items = braces(input)?;
    Ok(Group {
        path,
        path_span,
        name: options.name.or(default_name),
        items,
    })
}

fn resource(input: ParseStream<'_>, singular: bool) -> syn::Result<Resource> {
    let keyword: Ident = input.parse()?;
    let span = keyword.span();
    let literal: LitStr = input.parse()?;
    input.parse::<Token![=>]>()?;
    let controller: Path = input.parse()?;
    let controller_span = controller
        .segments
        .last()
        .map_or_else(Span::call_site, |segment| segment.ident.span());
    let options = options(input)?;
    let items = if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
        Vec::new()
    } else {
        braces(input)?
    };
    Ok(Resource {
        singular,
        path: literal.value(),
        path_span: literal.span(),
        controller,
        controller_span,
        only: options.only,
        except: options.except,
        param: options.param,
        name: options.name,
        shallow: options.shallow.is_some_and(|(value, _)| value),
        items,
        span,
    })
}

fn block(input: ParseStream<'_>, kind: BlockKind, span: Span) -> syn::Result<Block> {
    Ok(Block {
        kind,
        span,
        items: braces(input)?,
    })
}

fn braces(input: ParseStream<'_>) -> syn::Result<Vec<Item>> {
    let content;
    braced!(content in input);
    let mut items = Vec::new();
    while !content.is_empty() {
        items.push(content.parse()?);
    }
    Ok(items)
}

/// Everything after the arrow and before the `;` or the `{`.
#[derive(Default)]
struct Options {
    only: Option<(Vec<Action>, Span)>,
    except: Option<(Vec<Action>, Span)>,
    param: Option<Ident>,
    name: Option<Ident>,
    shallow: Option<(bool, Span)>,
}

impl Options {
    /// Four of the five words only mean something on a resource. Accepting
    /// them elsewhere and ignoring them would be the one failure a route
    /// table cannot afford: a line that reads as a rule and is not one.
    fn reject_resource_only(&self, subject: &str) -> syn::Result<()> {
        let misplaced = [
            self.only.as_ref().map(|(_, span)| ("only", *span)),
            self.except.as_ref().map(|(_, span)| ("except", *span)),
            self.param.as_ref().map(|i| ("param", i.span())),
            self.shallow.as_ref().map(|(_, span)| ("shallow", *span)),
        ];
        match misplaced.into_iter().flatten().next() {
            Some((word, span)) => Err(syn::Error::new(
                span,
                format!("`{word}:` is for `resources` and `resource`, not for {subject}"),
            )),
            None => Ok(()),
        }
    }
}

fn options(input: ParseStream<'_>) -> syn::Result<Options> {
    let mut out = Options::default();
    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        let lookahead = input.lookahead1();
        if lookahead.peek(kw::only) {
            let token: kw::only = input.parse()?;
            input.parse::<Token![:]>()?;
            out.only = Some((actions(input)?, token.span));
        } else if lookahead.peek(kw::except) {
            let token: kw::except = input.parse()?;
            input.parse::<Token![:]>()?;
            out.except = Some((actions(input)?, token.span));
        } else if lookahead.peek(kw::param) {
            input.parse::<kw::param>()?;
            input.parse::<Token![:]>()?;
            out.param = Some(input.parse()?);
        } else if lookahead.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            input.parse::<Token![:]>()?;
            out.name = Some(input.parse()?);
        } else if lookahead.peek(kw::shallow) {
            let token: kw::shallow = input.parse()?;
            input.parse::<Token![:]>()?;
            let value: LitBool = input.parse()?;
            out.shallow = Some((value.value, token.span));
        } else {
            return Err(lookahead.error());
        }
    }
    Ok(out)
}

fn actions(input: ParseStream<'_>) -> syn::Result<Vec<Action>> {
    let content;
    bracketed!(content in input);
    let names: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
    names.iter().map(Action::from_ident).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> syn::Result<Routes> {
        syn::parse_str::<Routes>(source)
    }

    /// What the compiler would print, which is the only part of a parse
    /// failure a person ever sees.
    fn error(source: &str) -> String {
        match parse(source) {
            Ok(_) => panic!("`{source}` parsed, and it should not have"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn root_is_a_get_on_the_slash() {
        let routes = parse("root => pages::index::handle;").unwrap();
        let Item::Route(route) = &routes.items[0] else {
            panic!("expected a route")
        };
        assert_eq!(route.path, "/");
        assert_eq!(route.verb, Verb::Get);
    }

    #[test]
    fn a_word_that_is_not_a_rule_names_the_ones_that_are() {
        let message = error("fetch \"/x\" => h;");
        assert!(message.contains("get"), "{message}");
        assert!(message.contains("resources"), "{message}");
    }

    #[test]
    fn an_action_that_is_not_one_of_the_seven_names_the_seven() {
        let message = error("resources \"/p\" => c, only: [lsit];");
        assert!(message.contains("destroy"), "{message}");
    }

    #[test]
    fn only_on_a_plain_route_is_rejected_rather_than_ignored() {
        let message = error("get \"/x\" => h, only: [index];");
        assert!(message.contains("`only:` is for"), "{message}");
    }

    #[test]
    fn a_resource_takes_a_block_or_a_semicolon() {
        assert!(parse("resources \"/p\" => c;").is_ok());
        assert!(parse("resources \"/p\" => c { member { get \"/x\" => h; } }").is_ok());
    }

    #[test]
    fn a_namespace_is_an_identifier_and_a_scope_is_a_path() {
        let routes = parse("namespace admin { get \"/\" => h; }").unwrap();
        let Item::Group(group) = &routes.items[0] else {
            panic!("expected a group")
        };
        assert_eq!(group.path, "/admin");
        assert_eq!(group.name.as_ref().unwrap(), "admin");

        let routes = parse("scope \"/v1\" { get \"/\" => h; }").unwrap();
        let Item::Group(group) = &routes.items[0] else {
            panic!("expected a group")
        };
        assert_eq!(group.path, "/v1");
        assert!(group.name.is_none());
    }
}
