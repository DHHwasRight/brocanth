//! The `#[task]` attribute.
//!
//! Not depended on directly: `brocanth-task` re-exports it, so a project adds
//! one crate and gets the macro, the trait, the registry and the runner.
//!
//! # What it emits, and why that shape
//!
//! A task has two halves with two different authors. The *name* is derived
//! from the struct and belongs to the macro; the *body* is the point of the
//! task and belongs to the person writing it. A single trait cannot hold both,
//! because an `impl` block has one author -- so there are two traits:
//!
//! ```ignore
//! #[brocanth_task::task]
//! #[derive(clap::Args)]
//! struct SeedDemo { /* ... */ }
//!
//! // Written here, by hand:
//! impl Task for SeedDemo {
//!     fn run(self, ctx: &mut Context) -> anyhow::Result<()> { /* ... */ }
//! }
//!
//! // Written by the macro, never by hand:
//! // impl Registered for SeedDemo { const NAME: &'static str = "seed_demo"; }
//! ```
//!
//! Every path the macro emits is absolute (`::brocanth_task::...`), so a local
//! item called `Task`, `Registered` or `entry` cannot capture it. The slice
//! element carries `#[linkme(crate = ::brocanth_task::linkme)]`, which is what
//! lets the registration work through a re-export: a project never names
//! `linkme` in its own manifest.

use heck::{ToShoutySnakeCase, ToSnakeCase};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Item, ItemStruct, LitStr};

/// Registers a `clap::Args` struct as a task.
///
/// The command name defaults to the struct name in `snake_case`
/// (`SeedDemo` becomes `seed_demo`) and `name = "..."` overrides it, which is
/// how a name no Rust identifier can spell -- `cache:clear` -- is reached.
#[proc_macro_attribute]
pub fn task(args: TokenStream, input: TokenStream) -> TokenStream {
    // Parsed as `Item` rather than `ItemStruct` so that an enum gets an error
    // naming what it is, pointing at itself, instead of clap's parser failing
    // somewhere inside a struct it never found.
    let item = parse_macro_input!(input as Item);
    let strukt = match item {
        Item::Struct(strukt) => strukt,
        other => return not_a_struct(&other).into(),
    };

    let name = match parse_name(args) {
        Ok(name) => name,
        Err(e) => return e.to_compile_error().into(),
    };

    expand(&strukt, &name).into()
}

/// The `name = "..."` argument, or `None` when there were no arguments.
///
/// Anything else is an error on the span of the offending key, which is what
/// `syn::meta::parser` gives for free.
fn parse_name(args: TokenStream) -> syn::Result<Option<String>> {
    if args.is_empty() {
        return Ok(None);
    }
    let mut name = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let literal: LitStr = meta.value()?.parse()?;
            if literal.value().is_empty() {
                return Err(syn::Error::new(
                    literal.span(),
                    "a task name cannot be empty",
                ));
            }
            name = Some(literal.value());
            return Ok(());
        }
        Err(meta.error("unknown argument; `#[task]` accepts only `name = \"...\"`"))
    });
    syn::parse::Parser::parse(parser, args)?;
    Ok(name)
}

/// The error for `#[task]` on anything that is not a struct.
fn not_a_struct(item: &Item) -> proc_macro2::TokenStream {
    let what = match item {
        Item::Enum(_) => "an enum",
        Item::Fn(_) => "a function",
        Item::Impl(_) => "an impl block",
        Item::Mod(_) => "a module",
        Item::Trait(_) => "a trait",
        Item::Union(_) => "a union",
        _ => "this item",
    };
    syn::Error::new_spanned(
        item,
        format!(
            "`#[task]` works on a struct, not on {what}: a task's fields are \
             its command-line arguments, which is what `#[derive(clap::Args)]` \
             reads"
        ),
    )
    .to_compile_error()
}

fn expand(strukt: &ItemStruct, override_name: &Option<String>) -> proc_macro2::TokenStream {
    let ident = &strukt.ident;
    let (impl_generics, ty_generics, where_clause) = strukt.generics.split_for_impl();

    let name = match override_name {
        Some(name) => name.clone(),
        None => ident.to_string().to_snake_case(),
    };

    // Prefixed and shouty so it cannot collide with anything the author wrote,
    // and suffixed with the derived name rather than the struct's, so two
    // tasks that differ only by `name = "..."` still get distinct statics.
    let slot = format_ident!(
        "__BROCANTH_TASK_{}",
        name.to_shouty_snake_case(),
        span = ident.span()
    );

    quote! {
        #strukt

        #[automatically_derived]
        impl #impl_generics ::brocanth_task::Registered for #ident #ty_generics #where_clause {
            const NAME: &'static str = #name;
        }

        // The registration. `#[linkme(crate = ...)]` is what makes this work
        // through the re-export: nothing here names `linkme` as the project
        // would have to spell it.
        #[::brocanth_task::linkme::distributed_slice(::brocanth_task::TASKS)]
        #[linkme(crate = ::brocanth_task::linkme)]
        #[doc(hidden)]
        static #slot: ::brocanth_task::TaskEntry =
            ::brocanth_task::entry::<#ident #ty_generics>();
    }
}
