//! `#[derive(Validate)]`.
//!
//! The derive writes two things from one declaration: the description
//! `brocanth-openapi` and `brocanth-admin` read, and the code that checks a
//! value. They cannot drift, because there is one source.
//!
//! # What this macro refuses to compile
//!
//! * A constraint that cannot mean anything for the field's type. The macro
//!   does not inspect the type — it cannot, since `type Name = String` is just
//!   an ident to it — so it emits a call to `AsText`, `AsInt`, `AsFloat`,
//!   `AsBool` or `AsSequence` and lets the trait solver answer. The error
//!   points at the field.
//! * Two type rules on one field, an unknown rule, or an argument an attribute
//!   does not take.
//! * A `date`, `time` or `datetime` bound that is not one. `2026-02-30` is
//!   rejected here, not on the first request.
//! * A `pattern` that is not a valid regular expression.
//! * A field whose type has no `Describe`, because the description it is
//!   missing from is what an OpenAPI emitter reads.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    parse_macro_input, Attribute, Data, DeriveInput, Expr, ExprLit, Fields, GenericArgument, Lit,
    LitStr, Meta, Path as SynPath, PathArguments, Token, Type,
};

/// Attach the brocanth validation vocabulary to a struct.
///
/// See the crate documentation of `brocanth-schema` for the grammar.
#[proc_macro_derive(Validate, attributes(validate))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

// ---------------------------------------------------------------------------
// Parsed form
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Accessor {
    Text,
    Int,
    Float,
    Bool,
    Nested,
}

#[derive(Clone)]
struct Kind {
    accessor: Accessor,
    /// The concrete schema type, for the `OnceLock` the derive emits.
    schema_type: Option<TokenStream2>,
    /// The builder expression.
    builder: Option<TokenStream2>,
    /// What the rule was called, for the "two type rules" error.
    name: String,
}

impl Kind {
    fn nested() -> Self {
        Self {
            accessor: Accessor::Nested,
            schema_type: None,
            builder: None,
            name: "nested".to_string(),
        }
    }
}

#[derive(Default)]
struct ArrayRules {
    min: Option<usize>,
    max: Option<usize>,
    exact: Option<usize>,
    unique: bool,
}

struct Refine {
    path: SynPath,
    code: String,
    message: String,
}

#[derive(Default)]
struct Rules {
    skip: bool,
    rename: Option<String>,
    description: Option<String>,
    required: bool,
    kind: Option<Kind>,
    array: Option<ArrayRules>,
    each: Option<Kind>,
    refines: Vec<Refine>,
}

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let unknown_keys = container_unknown_keys(&input.attrs)?;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "`Validate` can only be derived for a struct: an enum has no fields to attach \
             constraints to, and a union has no safe way to read one",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new(
            data.fields.span(),
            "`Validate` needs named fields, because a field name is the path an issue reports",
        ));
    };

    let mut field_nodes = Vec::new();
    let mut field_checks = Vec::new();

    for field in &named.named {
        let rules = parse_rules(&field.attrs)?;
        if rules.skip {
            continue;
        }
        let member = field.ident.as_ref().expect("named field");
        let name = rules.rename.clone().unwrap_or_else(|| member.to_string());
        let optional_ty = option_inner(&field.ty);
        let is_option = optional_ty.is_some();
        let base_ty = optional_ty.unwrap_or(&field.ty);
        let optional_field = is_option && !rules.required;

        // ----- the description
        let mut node = field_node(&rules, base_ty);
        for refine in rules.refines.iter().rev() {
            let code = &refine.code;
            let message = &refine.message;
            node = quote! {
                ::brocanth_schema::SchemaNode::Refined {
                    inner: ::std::boxed::Box::new(#node),
                    code: ::std::borrow::Cow::Borrowed(#code),
                    description: ::core::option::Option::Some(::std::borrow::Cow::Borrowed(#message)),
                }
            };
        }
        if optional_field {
            node = quote! {
                ::brocanth_schema::SchemaNode::Optional(::std::boxed::Box::new(#node))
            };
        }
        let description = match &rules.description {
            Some(text) => {
                quote!(::core::option::Option::Some(::std::borrow::Cow::Borrowed(#text)))
            }
            None => quote!(::core::option::Option::None),
        };
        let required = !optional_field;
        field_nodes.push(quote! {
            ::brocanth_schema::FieldNode {
                name: ::std::borrow::Cow::Borrowed(#name),
                required: #required,
                schema: #node,
                description: #description,
            }
        });

        // ----- the checks
        let value = if is_option {
            quote!(__value)
        } else {
            quote!(&self.#member)
        };
        let core = build_checks(&rules, &value);
        if core.is_empty() {
            continue;
        }
        let body = if is_option {
            if rules.required {
                quote! {
                    match &self.#member {
                        ::core::option::Option::Some(__value) => { #core }
                        ::core::option::Option::None => {
                            issues.report(at, ::brocanth_schema::IssueCode::Missing, |i| {
                                i.expected("a value").received("nothing").message("is required")
                            });
                        }
                    }
                }
            } else {
                quote! {
                    if let ::core::option::Option::Some(__value) = &self.#member { #core }
                }
            }
        } else {
            core
        };
        field_checks.push(quote! {
            at.with_field(#name, |at| { #body });
        });
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::brocanth_schema::Validate for #ident #ty_generics #where_clause {
            fn schema() -> ::brocanth_schema::SchemaNode
            where
                Self: ::core::marker::Sized,
            {
                ::brocanth_schema::SchemaNode::Object(::std::boxed::Box::new(
                    ::brocanth_schema::ObjectNode {
                        fields: ::std::vec![ #(#field_nodes),* ],
                        unknown_keys: #unknown_keys,
                    },
                ))
            }

            fn validate_at(
                &self,
                at: &mut ::brocanth_schema::Path,
                issues: &mut ::brocanth_schema::Issues,
            ) {
                #(#field_checks)*
            }
        }

        #[automatically_derived]
        impl #impl_generics ::brocanth_schema::Describe for #ident #ty_generics #where_clause {
            fn describe() -> ::brocanth_schema::SchemaNode {
                <Self as ::brocanth_schema::Validate>::schema()
            }
        }
    })
}

fn field_node(rules: &Rules, base_ty: &Type) -> TokenStream2 {
    if rules.array.is_some() || rules.each.is_some() {
        let array = rules.array.as_ref().cloned_or_default();
        let min = opt_usize(array.min);
        let max = opt_usize(array.max);
        let exact = opt_usize(array.exact);
        let unique = array.unique;
        let items = match &rules.each {
            Some(kind) if kind.accessor != Accessor::Nested => {
                let builder = kind.builder.as_ref().expect("a builder for a typed rule");
                quote!(::brocanth_schema::Schema::describe(&#builder))
            }
            Some(_) => quote! {
                <<#base_ty as ::brocanth_schema::AsSequence>::Item
                    as ::brocanth_schema::Validate>::schema()
            },
            None => quote! {
                <<#base_ty as ::brocanth_schema::AsSequence>::Item
                    as ::brocanth_schema::Describe>::describe()
            },
        };
        return quote! {
            ::brocanth_schema::SchemaNode::Array(::std::boxed::Box::new(
                ::brocanth_schema::ArrayNode {
                    items: #items,
                    min_items: #min,
                    max_items: #max,
                    exact_items: #exact,
                    unique: #unique,
                },
            ))
        };
    }

    match &rules.kind {
        Some(kind) if kind.accessor != Accessor::Nested => {
            let builder = kind.builder.as_ref().expect("a builder for a typed rule");
            quote!(::brocanth_schema::Schema::describe(&#builder))
        }
        // `nested` says the field is itself a `Validate`, so its own schema is
        // the description — no `Describe` impl needed next to a hand-written
        // one.
        Some(_) => quote!(<#base_ty as ::brocanth_schema::Validate>::schema()),
        None => quote!(<#base_ty as ::brocanth_schema::Describe>::describe()),
    }
}

fn build_checks(rules: &Rules, value: &TokenStream2) -> TokenStream2 {
    let mut out = TokenStream2::new();

    if rules.array.is_some() || rules.each.is_some() {
        let array = rules.array.as_ref().cloned_or_default();
        let min = opt_usize(array.min);
        let max = opt_usize(array.max);
        let exact = opt_usize(array.exact);
        out.extend(quote! {
            let __items = ::brocanth_schema::AsSequence::items(#value);
            ::brocanth_schema::array::check_length(
                __items.len(), #min, #max, #exact, at, issues,
            );
        });
        if let Some(each) = &rules.each {
            let item = scalar_check(each, &quote!(__item));
            out.extend(quote! {
                for (__index, __item) in __items.iter().enumerate() {
                    at.with_index(__index, |at| { #item });
                }
            });
        }
        if array.unique {
            out.extend(quote! {
                ::brocanth_schema::array::check_unique(__items, at, issues);
            });
        }
    } else if let Some(kind) = &rules.kind {
        out.extend(scalar_check(kind, value));
    }

    for refine in &rules.refines {
        let path = &refine.path;
        let code = &refine.code;
        let message = &refine.message;
        out.extend(quote! {
            if !#path(#value) {
                issues.report(
                    at,
                    ::brocanth_schema::IssueCode::Refused(::std::borrow::Cow::Borrowed(#code)),
                    |i| {
                        i.expected(#message)
                            .received("a value the check refused")
                            .message(#message)
                    },
                );
            }
        });
    }

    out
}

fn scalar_check(kind: &Kind, value: &TokenStream2) -> TokenStream2 {
    if kind.accessor == Accessor::Nested {
        return quote! { ::brocanth_schema::Validate::validate_at(#value, at, issues); };
    }
    let schema_type = kind.schema_type.as_ref().expect("a type for a typed rule");
    let builder = kind.builder.as_ref().expect("a builder for a typed rule");
    let input = match kind.accessor {
        Accessor::Text => quote!(::brocanth_schema::AsText::as_text(#value)),
        Accessor::Int => quote!(&::brocanth_schema::AsInt::as_int(#value)),
        Accessor::Float => quote!(&::brocanth_schema::AsFloat::as_float(#value)),
        Accessor::Bool => quote!(&::brocanth_schema::AsBool::as_bool(#value)),
        Accessor::Nested => unreachable!("handled above"),
    };
    quote! {
        {
            static __SCHEMA: ::std::sync::OnceLock<#schema_type> = ::std::sync::OnceLock::new();
            let __schema = __SCHEMA.get_or_init(|| #builder);
            ::brocanth_schema::Schema::check(__schema, #input, at, issues);
        }
    }
}

// ---------------------------------------------------------------------------
// Attribute parsing
// ---------------------------------------------------------------------------

fn container_unknown_keys(attrs: &[Attribute]) -> syn::Result<TokenStream2> {
    let mut policy = quote!(::brocanth_schema::UnknownKeys::Reject);
    for attr in attrs.iter().filter(|a| a.path().is_ident("validate")) {
        let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in metas {
            match &meta {
                Meta::NameValue(nv) if nv.path.is_ident("unknown_keys") => {
                    let value = lit_str(&nv.value)?;
                    policy = match value.value().as_str() {
                        "reject" => quote!(::brocanth_schema::UnknownKeys::Reject),
                        "ignore" => quote!(::brocanth_schema::UnknownKeys::Ignore),
                        "passthrough" => quote!(::brocanth_schema::UnknownKeys::Passthrough),
                        other => {
                            return Err(syn::Error::new(
                                value.span(),
                                format!(
                                    "unknown key policy `{other}`: expected \"reject\", \
                                     \"ignore\" or \"passthrough\""
                                ),
                            ))
                        }
                    };
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "the only `#[validate(..)]` option on a struct is \
                         `unknown_keys = \"reject\" | \"ignore\" | \"passthrough\"`",
                    ))
                }
            }
        }
    }
    Ok(policy)
}

fn parse_rules(attrs: &[Attribute]) -> syn::Result<Rules> {
    let mut rules = Rules::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("validate")) {
        let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in metas {
            apply_rule(&mut rules, meta)?;
        }
    }
    Ok(rules)
}

fn apply_rule(rules: &mut Rules, meta: Meta) -> syn::Result<()> {
    let span = meta.span();
    match &meta {
        Meta::Path(path) if path.is_ident("skip") => rules.skip = true,
        Meta::Path(path) if path.is_ident("required") => rules.required = true,
        Meta::NameValue(nv) if nv.path.is_ident("rename") => {
            rules.rename = Some(lit_str(&nv.value)?.value());
        }
        Meta::NameValue(nv) if nv.path.is_ident("describe") => {
            rules.description = Some(lit_str(&nv.value)?.value());
        }
        Meta::List(list) if list.path.is_ident("array") => {
            rules.array = Some(parse_array(list)?);
        }
        Meta::List(list) if list.path.is_ident("each") => {
            let inner = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            if inner.len() != 1 {
                return Err(syn::Error::new(
                    span,
                    "`each(..)` takes exactly one rule: `each(string(..))`, `each(int(..))`, \
                     `each(nested)`",
                ));
            }
            let kind = parse_kind(inner.into_iter().next().expect("one rule"))?;
            rules.each = Some(kind);
        }
        Meta::List(list) if list.path.is_ident("refine") => {
            rules.refines.push(parse_refine(list)?);
        }
        other => {
            let kind = parse_kind(other.clone())?;
            if let Some(existing) = &rules.kind {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "this field already has the `{}` rule: a field has one type, so it has \
                         one type rule",
                        existing.name
                    ),
                ));
            }
            rules.kind = Some(kind);
        }
    }
    Ok(())
}

fn parse_kind(meta: Meta) -> syn::Result<Kind> {
    let span = meta.span();
    match meta {
        Meta::Path(path) if path.is_ident("nested") => Ok(Kind::nested()),
        Meta::Path(path) if path.is_ident("bool") => Ok(Kind {
            accessor: Accessor::Bool,
            schema_type: Some(quote!(::brocanth_schema::BoolSchema)),
            builder: Some(quote!(::brocanth_schema::boolean())),
            name: "bool".to_string(),
        }),
        Meta::List(list) if list.path.is_ident("string") => parse_string(&list),
        Meta::List(list) if list.path.is_ident("int") => parse_int(&list),
        Meta::List(list) if list.path.is_ident("float") => parse_float(&list),
        Meta::List(list) if list.path.is_ident("one_of") => parse_one_of(&list),
        Meta::List(list) if list.path.is_ident("date") => {
            parse_temporal(&list, "date", quote!(DateSchema), quote!(Date))
        }
        Meta::List(list) if list.path.is_ident("time") => {
            parse_temporal(&list, "time", quote!(TimeSchema), quote!(Time))
        }
        Meta::List(list) if list.path.is_ident("datetime") => {
            parse_temporal(&list, "datetime", quote!(DateTimeSchema), quote!(DateTime))
        }
        _ => Err(syn::Error::new(
            span,
            "unknown rule: expected one of `string(..)`, `int(..)`, `float(..)`, `bool`, \
             `date(..)`, `time(..)`, `datetime(..)`, `one_of(..)`, `array(..)`, `each(..)`, \
             `nested`, `refine(..)`, `required`, `rename = \"..\"`, `describe = \"..\"`, `skip`",
        )),
    }
}

fn parse_string(list: &syn::MetaList) -> syn::Result<Kind> {
    let mut builder = quote!(::brocanth_schema::string());
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        match &meta {
            Meta::Path(p) if p.is_ident("email") => builder = quote!(#builder.email()),
            Meta::Path(p) if p.is_ident("url") => builder = quote!(#builder.url()),
            Meta::Path(p) if p.is_ident("uuid") => builder = quote!(#builder.uuid()),
            Meta::Path(p) if p.is_ident("non_empty") => builder = quote!(#builder.non_empty()),
            Meta::NameValue(nv) if nv.path.is_ident("min_length") => {
                let n = lit_usize(&nv.value)?;
                builder = quote!(#builder.min_length(#n));
            }
            Meta::NameValue(nv) if nv.path.is_ident("max_length") => {
                let n = lit_usize(&nv.value)?;
                builder = quote!(#builder.max_length(#n));
            }
            Meta::NameValue(nv) if nv.path.is_ident("length") => {
                let n = lit_usize(&nv.value)?;
                builder = quote!(#builder.length(#n));
            }
            Meta::NameValue(nv) if nv.path.is_ident("pattern") => {
                let value = lit_str(&nv.value)?;
                check_pattern(&value)?;
                builder = quote!(#builder.pattern(#value));
            }
            Meta::NameValue(nv) if nv.path.is_ident("starts_with") => {
                let value = lit_str(&nv.value)?;
                builder = quote!(#builder.starts_with(#value));
            }
            Meta::NameValue(nv) if nv.path.is_ident("ends_with") => {
                let value = lit_str(&nv.value)?;
                builder = quote!(#builder.ends_with(#value));
            }
            Meta::NameValue(nv) if nv.path.is_ident("contains") => {
                let value = lit_str(&nv.value)?;
                builder = quote!(#builder.contains(#value));
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "unknown string constraint: expected `min_length`, `max_length`, `length`, \
                     `non_empty`, `pattern`, `email`, `url`, `uuid`, `starts_with`, `ends_with` \
                     or `contains`",
                ))
            }
        }
    }
    Ok(Kind {
        accessor: Accessor::Text,
        schema_type: Some(quote!(::brocanth_schema::StringSchema)),
        builder: Some(builder),
        name: "string".to_string(),
    })
}

fn parse_int(list: &syn::MetaList) -> syn::Result<Kind> {
    let mut builder = quote!(::brocanth_schema::int());
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        match &meta {
            Meta::Path(p) if p.is_ident("positive") => builder = quote!(#builder.positive()),
            Meta::Path(p) if p.is_ident("non_negative") => {
                builder = quote!(#builder.non_negative());
            }
            Meta::NameValue(nv) => {
                let n = lit_i64(&nv.value)?;
                builder = match () {
                    () if nv.path.is_ident("min") => quote!(#builder.min(#n)),
                    () if nv.path.is_ident("max") => quote!(#builder.max(#n)),
                    () if nv.path.is_ident("exclusive_min") => quote!(#builder.exclusive_min(#n)),
                    () if nv.path.is_ident("exclusive_max") => quote!(#builder.exclusive_max(#n)),
                    () if nv.path.is_ident("multiple_of") => {
                        if n == 0 {
                            return Err(syn::Error::new(
                                nv.value.span(),
                                "`multiple_of = 0` can never be satisfied",
                            ));
                        }
                        quote!(#builder.multiple_of(#n))
                    }
                    () => {
                        return Err(syn::Error::new(
                            nv.path.span(),
                            "unknown integer constraint: expected `min`, `max`, `exclusive_min`, \
                             `exclusive_max`, `multiple_of`, `positive` or `non_negative`",
                        ))
                    }
                };
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "unknown integer constraint: expected `min`, `max`, `exclusive_min`, \
                     `exclusive_max`, `multiple_of`, `positive` or `non_negative`",
                ))
            }
        }
    }
    Ok(Kind {
        accessor: Accessor::Int,
        schema_type: Some(quote!(::brocanth_schema::IntSchema)),
        builder: Some(builder),
        name: "int".to_string(),
    })
}

fn parse_float(list: &syn::MetaList) -> syn::Result<Kind> {
    let mut builder = quote!(::brocanth_schema::float());
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        match &meta {
            Meta::Path(p) if p.is_ident("integer") => builder = quote!(#builder.integer()),
            Meta::Path(p) if p.is_ident("finite") => builder = quote!(#builder.finite()),
            Meta::NameValue(nv) => {
                let n = lit_f64(&nv.value)?;
                builder = match () {
                    () if nv.path.is_ident("min") => quote!(#builder.min(#n)),
                    () if nv.path.is_ident("max") => quote!(#builder.max(#n)),
                    () if nv.path.is_ident("exclusive_min") => quote!(#builder.exclusive_min(#n)),
                    () if nv.path.is_ident("exclusive_max") => quote!(#builder.exclusive_max(#n)),
                    () if nv.path.is_ident("multiple_of") => {
                        if n == 0.0 {
                            return Err(syn::Error::new(
                                nv.value.span(),
                                "`multiple_of = 0` can never be satisfied",
                            ));
                        }
                        quote!(#builder.multiple_of(#n))
                    }
                    () => {
                        return Err(syn::Error::new(
                            nv.path.span(),
                            "unknown float constraint: expected `min`, `max`, `exclusive_min`, \
                             `exclusive_max`, `multiple_of`, `integer` or `finite`",
                        ))
                    }
                };
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "unknown float constraint: expected `min`, `max`, `exclusive_min`, \
                     `exclusive_max`, `multiple_of`, `integer` or `finite`",
                ))
            }
        }
    }
    Ok(Kind {
        accessor: Accessor::Float,
        schema_type: Some(quote!(::brocanth_schema::FloatSchema)),
        builder: Some(builder),
        name: "float".to_string(),
    })
}

fn parse_one_of(list: &syn::MetaList) -> syn::Result<Kind> {
    let variants = list.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
    if variants.is_empty() {
        return Err(syn::Error::new(
            list.span(),
            "`one_of(..)` needs at least one variant; an empty set accepts nothing",
        ));
    }
    let values: Vec<&LitStr> = variants.iter().collect();
    Ok(Kind {
        accessor: Accessor::Text,
        schema_type: Some(quote!(::brocanth_schema::EnumSchema)),
        builder: Some(quote!(::brocanth_schema::one_of([#(#values),*]))),
        name: "one_of".to_string(),
    })
}

fn parse_temporal(
    list: &syn::MetaList,
    name: &str,
    schema_type: TokenStream2,
    value_type: TokenStream2,
) -> syn::Result<Kind> {
    let builder_fn = syn::Ident::new(name, list.span());
    let mut builder = quote!(::brocanth_schema::#builder_fn());
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        let Meta::NameValue(nv) = &meta else {
            return Err(syn::Error::new(
                meta.span(),
                format!("`{name}(..)` takes `min = \"..\"` and `max = \"..\"`"),
            ));
        };
        let value = lit_str(&nv.value)?;
        check_temporal(name, &value)?;
        let parsed = quote! {
            #value.parse::<::brocanth_schema::#value_type>()
                .expect("checked by #[derive(Validate)]")
        };
        builder = match () {
            () if nv.path.is_ident("min") => quote!(#builder.min(#parsed)),
            () if nv.path.is_ident("max") => quote!(#builder.max(#parsed)),
            () => {
                return Err(syn::Error::new(
                    nv.path.span(),
                    format!("`{name}(..)` takes `min = \"..\"` and `max = \"..\"`"),
                ))
            }
        };
    }
    Ok(Kind {
        accessor: Accessor::Text,
        schema_type: Some(quote!(::brocanth_schema::#schema_type)),
        builder: Some(builder),
        name: name.to_string(),
    })
}

fn parse_array(list: &syn::MetaList) -> syn::Result<ArrayRules> {
    let mut rules = ArrayRules::default();
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        match &meta {
            Meta::Path(p) if p.is_ident("unique") => rules.unique = true,
            Meta::Path(p) if p.is_ident("non_empty") => rules.min = Some(1),
            Meta::NameValue(nv) if nv.path.is_ident("min_items") => {
                rules.min = Some(lit_usize(&nv.value)?);
            }
            Meta::NameValue(nv) if nv.path.is_ident("max_items") => {
                rules.max = Some(lit_usize(&nv.value)?);
            }
            Meta::NameValue(nv) if nv.path.is_ident("length") => {
                rules.exact = Some(lit_usize(&nv.value)?);
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "unknown array constraint: expected `min_items`, `max_items`, `length`, \
                     `non_empty` or `unique`",
                ))
            }
        }
    }
    Ok(rules)
}

fn parse_refine(list: &syn::MetaList) -> syn::Result<Refine> {
    let metas = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    let mut path = None;
    let mut code = None;
    let mut message = None;
    for meta in &metas {
        match meta {
            Meta::Path(p) if path.is_none() => path = Some(p.clone()),
            Meta::NameValue(nv) if nv.path.is_ident("code") => {
                code = Some(lit_str(&nv.value)?.value());
            }
            Meta::NameValue(nv) if nv.path.is_ident("message") => {
                message = Some(lit_str(&nv.value)?.value());
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "`refine(..)` takes a function path, then `code = \"..\"` and \
                     `message = \"..\"`",
                ))
            }
        }
    }
    let path = path.ok_or_else(|| {
        syn::Error::new(
            list.span(),
            "`refine(..)` needs the path of a `fn(&FieldType) -> bool`",
        )
    })?;
    let default_code = path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| "refused".to_string());
    Ok(Refine {
        code: code.unwrap_or(default_code),
        message: message.unwrap_or_else(|| "is not valid".to_string()),
        path,
    })
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

fn lit_str(expr: &Expr) -> syn::Result<LitStr> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(syn::Error::new(other.span(), "expected a string literal")),
    }
}

fn lit_usize(expr: &Expr) -> syn::Result<usize> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse(),
        other => Err(syn::Error::new(
            other.span(),
            "expected a non-negative whole number",
        )),
    }
}

fn lit_i64(expr: &Expr) -> syn::Result<i64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse(),
        Expr::Unary(unary) => {
            let inner = lit_i64(&unary.expr)?;
            Ok(-inner)
        }
        other => Err(syn::Error::new(other.span(), "expected a whole number")),
    }
}

fn lit_f64(expr: &Expr) -> syn::Result<f64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Float(f), ..
        }) => f.base10_parse(),
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => Ok(i.base10_parse::<i64>()? as f64),
        Expr::Unary(unary) => Ok(-lit_f64(&unary.expr)?),
        other => Err(syn::Error::new(other.span(), "expected a number")),
    }
}

fn opt_usize(value: Option<usize>) -> TokenStream2 {
    match value {
        Some(n) => quote!(::core::option::Option::Some(#n)),
        None => quote!(::core::option::Option::None),
    }
}

/// `Option<T>` written in any of the three usual ways.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() {
        return None;
    }
    let last = path.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Literals this macro refuses
// ---------------------------------------------------------------------------

fn check_pattern(value: &LitStr) -> syn::Result<()> {
    let source = value.value();
    let body = source.strip_prefix('^').unwrap_or(&source);
    let body = body.strip_suffix('$').unwrap_or(body);
    match regex::Regex::new(&format!("^(?:{body})$")) {
        Ok(_) => Ok(()),
        Err(error) => Err(syn::Error::new(
            value.span(),
            format!("not a valid regular expression: {error}"),
        )),
    }
}

fn check_temporal(name: &str, value: &LitStr) -> syn::Result<()> {
    let text = value.value();
    let ok = match name {
        "date" => is_date(&text),
        "time" => is_time(&text),
        _ => is_datetime(&text),
    };
    if ok {
        return Ok(());
    }
    let shape = match name {
        "date" => "YYYY-MM-DD",
        "time" => "HH:MM[:SS[.fff]]",
        _ => "YYYY-MM-DDTHH:MM[:SS][Z|±HH:MM]",
    };
    Err(syn::Error::new(
        value.span(),
        format!("`{text}` is not a {name} in {shape} form"),
    ))
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (
        number(&text[0..4]),
        number(&text[5..7]),
        number(&text[8..10]),
    ) else {
        return false;
    };
    (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year as i32, month)
}

fn is_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 5 || bytes[2] != b':' {
        return false;
    }
    let (Some(hour), Some(minute)) = (number(&text[0..2]), number(&text[3..5])) else {
        return false;
    };
    if hour > 23 || minute > 59 {
        return false;
    }
    if bytes.len() == 5 {
        return true;
    }
    if bytes[5] != b':' || bytes.len() < 8 {
        return false;
    }
    let Some(second) = number(&text[6..8]) else {
        return false;
    };
    if second > 59 {
        return false;
    }
    if bytes.len() == 8 {
        return true;
    }
    bytes[8] == b'.' && text.len() <= 18 && number(&text[9..]).is_some()
}

fn is_datetime(text: &str) -> bool {
    if text.len() < 11 || !is_date(&text[0..10]) {
        return false;
    }
    let separator = text.as_bytes()[10];
    if !matches!(separator, b'T' | b't' | b' ') {
        return false;
    }
    let rest = &text[11..];
    if let Some(stripped) = rest.strip_suffix(['Z', 'z']) {
        return is_time(stripped);
    }
    if let Some(position) = rest.rfind(['+', '-']) {
        let (clock, offset) = rest.split_at(position);
        let body = &offset[1..];
        let valid_offset = match body.len() {
            5 if body.as_bytes()[2] == b':' => {
                matches!((number(&body[0..2]), number(&body[3..5])), (Some(h), Some(m)) if h <= 23 && m <= 59)
            }
            4 => {
                matches!((number(&body[0..2]), number(&body[2..4])), (Some(h), Some(m)) if h <= 23 && m <= 59)
            }
            2 => matches!(number(body), Some(h) if h <= 23),
            _ => false,
        };
        return valid_offset && is_time(clock);
    }
    is_time(rest)
}

// ---------------------------------------------------------------------------
// Small conveniences
// ---------------------------------------------------------------------------

trait ClonedOrDefault {
    fn cloned_or_default(self) -> ArrayRules;
}

impl ClonedOrDefault for Option<&ArrayRules> {
    fn cloned_or_default(self) -> ArrayRules {
        match self {
            Some(rules) => ArrayRules {
                min: rules.min,
                max: rules.max,
                exact: rules.exact,
                unique: rules.unique,
            },
            None => ArrayRules::default(),
        }
    }
}
