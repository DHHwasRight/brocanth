//! `#[derive(Subject)]`: docs/policy-rls-bridge.md, decision 5.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, LitStr};

pub(crate) fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new(
                    name.span(),
                    format!("`Subject` needs a struct with named fields\nhelp: name each field of `{name}`; the field name is the attribute's key"),
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                name.span(),
                "`Subject` needs a struct\nhelp: derive it on the struct that describes who is asking",
            ))
        }
    };

    let mut attrs = Vec::new();
    let mut values = Vec::new();
    for field in fields {
        let Some(marker) = field.attrs.iter().find(|a| a.path().is_ident("attr")) else {
            continue;
        };
        if !matches!(marker.meta, syn::Meta::Path(_)) {
            return Err(syn::Error::new(
                marker.meta.path().span(),
                "`#[attr]` takes no arguments\nhelp: write `#[attr]`; the key is the field name",
            ));
        }
        if field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("attr"))
            .count()
            > 1
        {
            return Err(syn::Error::new(
                marker.span(),
                "`#[attr]` appears twice on one field\nhelp: write it once",
            ));
        }
        let ident = field.ident.as_ref().expect("named field");
        let key = LitStr::new(&ident.unraw().to_string(), ident.span());
        let ty = &field.ty;
        let attr_type = quote_spanned!(ty.span()=> <#ty as ::brocanth_policy::AttrType>);
        attrs.push(quote! {
            ::brocanth_policy::Attr {
                key: #key,
                kind: #attr_type::KIND,
                optional: #attr_type::OPTIONAL,
            }
        });
        values.push(quote! {
            .with(#key, #attr_type::attr_value(&self.#ident))
        });
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::brocanth_policy::Subject for #name #ty_generics #where_clause {
            const ATTRS: &'static [::brocanth_policy::Attr] = &[#(#attrs),*];

            fn attributes(&self) -> ::brocanth_policy::Attributes {
                ::brocanth_policy::Attributes::new()
                    #(#values)*
            }
        }
    })
}
