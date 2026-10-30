//! The `#[validates(..)]` attribute macro behind `brocanth_model`'s declarative
//! validations.
//!
//! Do not depend on this crate directly. Use `brocanth_model::validates`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parenthesized, parse_macro_input, Error, Ident, ItemStruct, Lit, Result, Token};

/// Declare validations for the `ActiveModel` that SeaORM generates from a
/// model struct. See `brocanth_model::validates` for the documented grammar.
#[proc_macro_attribute]
pub fn validates(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ValidatesArgs);
    let mut item = parse_macro_input!(item as ItemStruct);
    if let Err(e) = strip_ids(&mut item) {
        let err = e.to_compile_error();
        return quote! { #item #err }.into();
    }
    match expand(&args, &item) {
        Ok(ts) => ts.into(),
        Err(e) => {
            let err = e.to_compile_error();
            quote! { #item #err }.into()
        }
    }
}

/// Remove the `#[brocanth(id = "..")]` attributes from a model and its fields,
/// after a check of their shape. See `brocanth_model::ids`.
#[proc_macro_attribute]
pub fn ids(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemStruct);
    if !attr.is_empty() {
        let err = Error::new(
            Span::call_site(),
            "`#[brocanth_model::ids]` takes no arguments",
        )
        .to_compile_error();
        return quote! { #item #err }.into();
    }
    match strip_ids(&mut item) {
        Ok(()) => quote! { #item }.into(),
        Err(e) => {
            let err = e.to_compile_error();
            quote! { #item #err }.into()
        }
    }
}

/// Take every `#[brocanth(..)]` off the struct and its fields. The ids are for
/// `brocanth migrate`, which reads them from the source text; the compiler
/// must not see them, because no derive declares `brocanth` as a helper.
fn strip_ids(item: &mut ItemStruct) -> Result<()> {
    let mut errors: Option<Error> = None;
    let mut take = |attrs: &mut Vec<syn::Attribute>| {
        attrs.retain(|a| {
            if !a.path().is_ident("brocanth") {
                return true;
            }
            if let Err(e) = check_id(a) {
                match &mut errors {
                    Some(all) => all.combine(e),
                    None => errors = Some(e),
                }
            }
            false
        });
    };
    take(&mut item.attrs);
    for field in item.fields.iter_mut() {
        take(&mut field.attrs);
    }
    errors.map_or(Ok(()), Err)
}

/// `#[brocanth(id = "<26 Crockford base-32 characters>")]`, and nothing else.
fn check_id(attr: &syn::Attribute) -> Result<()> {
    attr.parse_nested_meta(|meta| {
        if !meta.path.is_ident("id") {
            return Err(meta.error("the only key in `#[brocanth(..)]` is `id`"));
        }
        let value: syn::LitStr = meta.value()?.parse()?;
        if is_ulid(&value.value()) {
            Ok(())
        } else {
            Err(Error::new(
                value.span(),
                "the id is not a ULID: 26 characters from `0123456789ABCDEFGHJKMNPQRSTVWXYZ`. \
                 Run `brocanth migrate ids` to write one",
            ))
        }
    })
}

fn is_ulid(text: &str) -> bool {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    text.len() == 26 && text.bytes().all(|b| ALPHABET.contains(&b)) && text.as_bytes()[0] <= b'7'
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

struct ValidatesArgs {
    no_hook: bool,
    fields: Vec<FieldRules>,
}

struct FieldRules {
    field: Ident,
    rules: Vec<Rule>,
}

enum Rule {
    Presence(Common),
    Length {
        min: Option<usize>,
        max: Option<usize>,
        is: Option<usize>,
        common: Common,
    },
    Format {
        pattern: String,
        common: Common,
    },
    Numericality {
        greater_than: Option<f64>,
        greater_than_or_equal_to: Option<f64>,
        less_than: Option<f64>,
        less_than_or_equal_to: Option<f64>,
        equal_to: Option<f64>,
        only_integer: bool,
        common: Common,
    },
    Inclusion {
        allowed: Vec<String>,
        common: Common,
    },
    Uniqueness {
        column: Option<Ident>,
        common: Common,
    },
}

#[derive(Default)]
struct Common {
    message: Option<String>,
}

enum Arg {
    Named(Ident, Lit),
    Flag(Ident),
    Positional(Lit),
}

impl Parse for ValidatesArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut out = ValidatesArgs {
            no_hook: false,
            fields: Vec::new(),
        };
        let items = Punctuated::<FieldRulesOrFlag, Token![,]>::parse_terminated(input)?;
        for item in items {
            match item {
                FieldRulesOrFlag::NoHook => out.no_hook = true,
                FieldRulesOrFlag::Field(f) => out.fields.push(f),
            }
        }
        Ok(out)
    }
}

enum FieldRulesOrFlag {
    NoHook,
    Field(FieldRules),
}

impl Parse for FieldRulesOrFlag {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name: Ident = input.parse()?;
        if !input.peek(syn::token::Paren) {
            return if name == "no_hook" {
                Ok(FieldRulesOrFlag::NoHook)
            } else {
                Err(Error::new(
                    name.span(),
                    "expected `no_hook` or `field(rule, ...)`",
                ))
            };
        }
        let content;
        parenthesized!(content in input);
        let rules = Punctuated::<Rule, Token![,]>::parse_terminated(&content)?;
        Ok(FieldRulesOrFlag::Field(FieldRules {
            field: name,
            rules: rules.into_iter().collect(),
        }))
    }
}

impl Parse for Rule {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name: Ident = input.parse()?;
        let mut args: Vec<Arg> = Vec::new();
        if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            for a in Punctuated::<Arg, Token![,]>::parse_terminated(&content)? {
                args.push(a);
            }
        }
        build_rule(&name, &args)
    }
}

impl Parse for Arg {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.peek(Ident) {
            let name: Ident = input.parse()?;
            if input.peek(Token![=]) {
                let _: Token![=] = input.parse()?;
                let lit: Lit = input.parse()?;
                Ok(Arg::Named(name, lit))
            } else {
                Ok(Arg::Flag(name))
            }
        } else {
            Ok(Arg::Positional(input.parse()?))
        }
    }
}

// ---------------------------------------------------------------------------
// Argument readers
// ---------------------------------------------------------------------------

fn lit_usize(lit: &Lit) -> Result<usize> {
    match lit {
        Lit::Int(i) => i.base10_parse(),
        other => Err(Error::new_spanned(other, "expected an integer")),
    }
}

fn lit_f64(lit: &Lit) -> Result<f64> {
    match lit {
        Lit::Int(i) => Ok(i.base10_parse::<i64>()? as f64),
        Lit::Float(f) => f.base10_parse(),
        other => Err(Error::new_spanned(other, "expected a number")),
    }
}

fn lit_string(lit: &Lit) -> Result<String> {
    match lit {
        Lit::Str(s) => Ok(s.value()),
        Lit::Int(i) => Ok(i.base10_digits().to_owned()),
        other => Err(Error::new_spanned(other, "expected a string")),
    }
}

fn build_rule(name: &Ident, args: &[Arg]) -> Result<Rule> {
    let mut common = Common::default();
    let mut rest: Vec<&Arg> = Vec::new();
    for a in args {
        match a {
            Arg::Named(k, v) if k == "message" => common.message = Some(lit_string(v)?),
            other => rest.push(other),
        }
    }

    let unexpected = |a: &Arg| -> Error {
        let span = match a {
            Arg::Named(k, _) | Arg::Flag(k) => k.span(),
            Arg::Positional(_) => Span::call_site(),
        };
        Error::new(span, format!("unexpected argument for `{name}`"))
    };

    match name.to_string().as_str() {
        "presence" => {
            if let Some(a) = rest.first() {
                return Err(unexpected(a));
            }
            Ok(Rule::Presence(common))
        }
        "length" => {
            let (mut min, mut max, mut is) = (None, None, None);
            for a in rest {
                match a {
                    Arg::Named(k, v) if k == "min" => min = Some(lit_usize(v)?),
                    Arg::Named(k, v) if k == "max" => max = Some(lit_usize(v)?),
                    Arg::Named(k, v) if k == "is" => is = Some(lit_usize(v)?),
                    other => return Err(unexpected(other)),
                }
            }
            Ok(Rule::Length {
                min,
                max,
                is,
                common,
            })
        }
        "format" => {
            let mut pattern = None;
            for a in rest {
                match a {
                    Arg::Positional(v) => pattern = Some(lit_string(v)?),
                    Arg::Named(k, v) if k == "pattern" || k == "with" => {
                        pattern = Some(lit_string(v)?)
                    }
                    other => return Err(unexpected(other)),
                }
            }
            let pattern = pattern.ok_or_else(|| {
                Error::new(name.span(), "`format` needs a pattern: format(r\"^\\w+$\")")
            })?;
            if let Err(e) = regex_is_parseable(&pattern) {
                return Err(Error::new(name.span(), e));
            }
            Ok(Rule::Format { pattern, common })
        }
        "numericality" => {
            let mut r = Rule::Numericality {
                greater_than: None,
                greater_than_or_equal_to: None,
                less_than: None,
                less_than_or_equal_to: None,
                equal_to: None,
                only_integer: false,
                common,
            };
            let Rule::Numericality {
                greater_than,
                greater_than_or_equal_to,
                less_than,
                less_than_or_equal_to,
                equal_to,
                only_integer,
                ..
            } = &mut r
            else {
                unreachable!()
            };
            for a in rest {
                match a {
                    Arg::Named(k, v) if k == "greater_than" => *greater_than = Some(lit_f64(v)?),
                    Arg::Named(k, v) if k == "greater_than_or_equal_to" || k == "min" => {
                        *greater_than_or_equal_to = Some(lit_f64(v)?)
                    }
                    Arg::Named(k, v) if k == "less_than" => *less_than = Some(lit_f64(v)?),
                    Arg::Named(k, v) if k == "less_than_or_equal_to" || k == "max" => {
                        *less_than_or_equal_to = Some(lit_f64(v)?)
                    }
                    Arg::Named(k, v) if k == "equal_to" => *equal_to = Some(lit_f64(v)?),
                    Arg::Flag(k) if k == "only_integer" => *only_integer = true,
                    other => return Err(unexpected(other)),
                }
            }
            Ok(r)
        }
        "inclusion" => {
            let mut allowed = Vec::new();
            for a in rest {
                match a {
                    Arg::Positional(v) => allowed.push(lit_string(v)?),
                    Arg::Named(k, v) if k == "in" => allowed.push(lit_string(v)?),
                    other => return Err(unexpected(other)),
                }
            }
            if allowed.is_empty() {
                return Err(Error::new(
                    name.span(),
                    "`inclusion` needs at least one allowed value",
                ));
            }
            Ok(Rule::Inclusion { allowed, common })
        }
        "uniqueness" => {
            let mut column = None;
            for a in rest {
                match a {
                    Arg::Named(k, v) if k == "column" => {
                        column = Some(Ident::new(&lit_string(v)?, k.span()))
                    }
                    other => return Err(unexpected(other)),
                }
            }
            Ok(Rule::Uniqueness { column, common })
        }
        other => Err(Error::new(
            name.span(),
            format!(
                "unknown validation `{other}`; expected one of presence, length, format, \
                 numericality, inclusion, uniqueness"
            ),
        )),
    }
}

/// Reject an unparseable pattern at compile time instead of at the first
/// request. The macro crate cannot depend on `regex`, so this only checks the
/// cheap structural mistakes; `Pattern` reports the rest on first use.
fn regex_is_parseable(pattern: &str) -> std::result::Result<(), String> {
    let mut depth = 0i32;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err("unbalanced `)` in pattern".to_owned());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unbalanced `(` in pattern".to_owned());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

fn pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = true;
    for c in s.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn opt_str(v: &Option<String>) -> TokenStream2 {
    match v {
        Some(s) => quote!(::core::option::Option::Some(#s)),
        None => quote!(::core::option::Option::None),
    }
}

fn opt_usize(v: &Option<usize>) -> TokenStream2 {
    match v {
        Some(n) => quote!(::core::option::Option::Some(#n)),
        None => quote!(::core::option::Option::None),
    }
}

fn opt_f64(v: &Option<f64>) -> TokenStream2 {
    match v {
        Some(n) => quote!(::core::option::Option::Some(#n)),
        None => quote!(::core::option::Option::None),
    }
}

fn expand(args: &ValidatesArgs, item: &ItemStruct) -> Result<TokenStream2> {
    let known: Vec<String> = item
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
        .collect();

    let mut sync_body = TokenStream2::new();
    let mut db_body = TokenStream2::new();
    let mut statics = TokenStream2::new();

    for fr in &args.fields {
        let name = fr.field.to_string();
        if !known.contains(&name) {
            return Err(Error::new(
                fr.field.span(),
                format!("`{name}` is not a field of `{}`", item.ident),
            ));
        }
        let field = &fr.field;

        for (i, rule) in fr.rules.iter().enumerate() {
            match rule {
                Rule::Presence(c) => {
                    let msg = opt_str(&c.message);
                    sync_body.extend(quote! {
                        ::brocanth_model::validators::presence(
                            __errors, #name,
                            ::brocanth_model::validators::scalar(&self.#field),
                            #msg,
                        );
                    });
                }
                Rule::Length {
                    min,
                    max,
                    is,
                    common,
                } => {
                    let (min, max, is) = (opt_usize(min), opt_usize(max), opt_usize(is));
                    let msg = opt_str(&common.message);
                    sync_body.extend(quote! {
                        ::brocanth_model::validators::length(
                            __errors, #name,
                            ::brocanth_model::validators::scalar(&self.#field),
                            ::brocanth_model::validators::Length {
                                min: #min, max: #max, is: #is,
                            },
                            #msg,
                        );
                    });
                }
                Rule::Format { pattern, common } => {
                    let konst =
                        format_ident!("__BROCANTH_FMT_{}_{}", name.to_uppercase(), i as u32);
                    statics.extend(quote! {
                        static #konst: ::brocanth_model::validators::Pattern =
                            ::brocanth_model::validators::Pattern::new(#pattern);
                    });
                    let msg = opt_str(&common.message);
                    sync_body.extend(quote! {
                        ::brocanth_model::validators::format(
                            __errors, #name,
                            ::brocanth_model::validators::scalar(&self.#field),
                            &#konst,
                            #msg,
                        );
                    });
                }
                Rule::Numericality {
                    greater_than,
                    greater_than_or_equal_to,
                    less_than,
                    less_than_or_equal_to,
                    equal_to,
                    only_integer,
                    common,
                } => {
                    let gt = opt_f64(greater_than);
                    let gte = opt_f64(greater_than_or_equal_to);
                    let lt = opt_f64(less_than);
                    let lte = opt_f64(less_than_or_equal_to);
                    let eq = opt_f64(equal_to);
                    let msg = opt_str(&common.message);
                    sync_body.extend(quote! {
                        ::brocanth_model::validators::numericality(
                            __errors, #name,
                            ::brocanth_model::validators::scalar(&self.#field),
                            ::brocanth_model::validators::Numericality {
                                greater_than: #gt,
                                greater_than_or_equal_to: #gte,
                                less_than: #lt,
                                less_than_or_equal_to: #lte,
                                equal_to: #eq,
                                only_integer: #only_integer,
                            },
                            #msg,
                        );
                    });
                }
                Rule::Inclusion { allowed, common } => {
                    let allowed = allowed.iter().map(|s| quote!(#s));
                    let msg = opt_str(&common.message);
                    sync_body.extend(quote! {
                        ::brocanth_model::validators::inclusion(
                            __errors, #name,
                            ::brocanth_model::validators::scalar(&self.#field),
                            &[#(#allowed),*],
                            #msg,
                        );
                    });
                }
                Rule::Uniqueness { column, common } => {
                    let col = column
                        .clone()
                        .unwrap_or_else(|| Ident::new(&pascal_case(&name), fr.field.span()));
                    let msg = opt_str(&common.message);
                    db_body.extend(quote! {
                        if let ::sea_orm::ActiveValue::Set(ref __v) = self.#field {
                            ::brocanth_model::validators::uniqueness::<Entity, _, _>(
                                __errors, #name, Column::#col, __v.clone(),
                                <Self as ::sea_orm::ActiveModelTrait>::get_primary_key_value(self),
                                __db, #msg,
                            ).await?;
                        }
                    });
                }
            }
        }
    }

    let hook = if args.no_hook {
        TokenStream2::new()
    } else {
        quote! {
            #[::brocanth_model::async_trait]
            impl ::sea_orm::ActiveModelBehavior for ActiveModel {
                async fn before_save<C>(self, db: &C, _insert: bool)
                    -> ::core::result::Result<Self, ::sea_orm::DbErr>
                where
                    C: ::sea_orm::ConnectionTrait,
                {
                    ::brocanth_model::enforce(self, db).await
                }
            }
        }
    };

    Ok(quote! {
        #item

        #[automatically_derived]
        #[allow(unused_variables, clippy::needless_question_mark)]
        #[::brocanth_model::async_trait]
        impl ::brocanth_model::Validate for ActiveModel {
            fn validate(&self, __errors: &mut ::brocanth_model::Errors) {
                #statics
                #sync_body
            }

            async fn validate_db<C>(
                &self,
                __db: &C,
                __errors: &mut ::brocanth_model::Errors,
            ) -> ::core::result::Result<(), ::sea_orm::DbErr>
            where
                C: ::sea_orm::ConnectionTrait,
            {
                #db_body
                ::core::result::Result::Ok(())
            }
        }

        #hook
    })
}
