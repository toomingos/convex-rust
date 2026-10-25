//! `#[derive(ConvexArgs)]` / `#[derive(ConvexValue)]`: a static Convex
//! validator (`convex_fn::ConvexType`) that matches what serde (de)serialises.
//! The derive reads the `#[serde(...)]` attributes that change the wire shape
//! (`rename`, `rename_all`, `default`, `skip*`, `tag`, `content`, `untagged`).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DataEnum, DeriveInput, Error, Fields, GenericArgument, Lit, PathArguments,
    Result, Type,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `ConvexArgs`: the type must be a struct with named fields.
    Args,
    /// `ConvexValue`: structs and enums.
    Value,
}

pub fn expand(input: DeriveInput, mode: Mode) -> Result<TokenStream> {
    let name = &input.ident;
    let container = ContainerAttrs::parse(&input.attrs)?;
    let validator = match &input.data {
        Data::Struct(s) => {
            if mode == Mode::Args && !matches!(s.fields, Fields::Named(_)) {
                return Err(Error::new_spanned(
                    &input.ident,
                    "`ConvexArgs` needs a struct with named fields (action arguments are an \
                     object); use `ConvexValue` for other shapes",
                ));
            }
            struct_validator(&s.fields, &container)?
        }
        Data::Enum(e) => {
            if mode == Mode::Args {
                return Err(Error::new_spanned(
                    &input.ident,
                    "`ConvexArgs` needs a struct (action arguments are an object); use \
                     `ConvexValue` for enums",
                ));
            }
            enum_validator(e, &container)?
        }
        Data::Union(_) => return Err(Error::new_spanned(&input.ident, "unions are not supported")),
    };

    let mut generics = input.generics.clone();
    for p in generics.type_params_mut() {
        p.bounds.push(syn::parse_quote!(::convex_fn::ConvexType));
    }
    let (impl_g, ty_g, where_g) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_g ::convex_fn::ConvexType for #name #ty_g #where_g {
            const VALIDATOR: ::convex_fn::Validator = #validator;
        }
    })
}

// ---------------------------------------------------------------------------
// attributes

#[derive(Default)]
struct ContainerAttrs {
    rename_all: Option<RenameRule>,
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
    transparent: bool,
}

#[derive(Default)]
struct FieldAttrs {
    rename: Option<String>,
    default: bool,
    skip: bool,
    skip_serializing: bool,
    skip_deserializing: bool,
    skip_serializing_if: bool,
    id: Option<String>,
    any: bool,
}

#[derive(Default)]
struct VariantAttrs {
    rename: Option<String>,
    rename_all: Option<RenameRule>,
    skip: bool,
}

fn lit_str(meta: &syn::meta::ParseNestedMeta) -> Result<String> {
    let lit: Lit = meta.value()?.parse()?;
    match lit {
        Lit::Str(s) => Ok(s.value()),
        other => Err(Error::new(other.span(), "expected a string literal")),
    }
}

/// `rename = "x"` or `rename(serialize = "x", deserialize = "y")` (uses the
/// deserialize name: the validator is mostly for arguments).
fn rename_value(meta: &syn::meta::ParseNestedMeta) -> Result<String> {
    if meta.input.peek(syn::Token![=]) {
        return lit_str(meta);
    }
    let mut ser = None;
    let mut de = None;
    meta.parse_nested_meta(|m| {
        if m.path.is_ident("serialize") {
            ser = Some(lit_str(&m)?);
        } else if m.path.is_ident("deserialize") {
            de = Some(lit_str(&m)?);
        }
        Ok(())
    })?;
    de.or(ser)
        .ok_or_else(|| meta.error("expected `rename = \"..\"`"))
}

/// Skips the value of an attribute we don't care about.
fn skip_value(meta: &syn::meta::ParseNestedMeta) -> Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let _: syn::Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|m| skip_value(&m))?;
    }
    Ok(())
}

impl ContainerAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut out = ContainerAttrs::default();
        for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    let s = if meta.input.peek(syn::Token![=]) {
                        lit_str(&meta)?
                    } else {
                        rename_value(&meta)?
                    };
                    out.rename_all =
                        Some(RenameRule::parse(&s).ok_or_else(|| {
                            meta.error(format!("unknown `rename_all` rule `{s}`"))
                        })?);
                } else if meta.path.is_ident("tag") {
                    out.tag = Some(lit_str(&meta)?);
                } else if meta.path.is_ident("content") {
                    out.content = Some(lit_str(&meta)?);
                } else if meta.path.is_ident("untagged") {
                    out.untagged = true;
                } else if meta.path.is_ident("transparent") {
                    out.transparent = true;
                } else if ["from", "into", "try_from", "remote"]
                    .iter()
                    .any(|k| meta.path.is_ident(k))
                {
                    return Err(meta.error(
                        "`ConvexArgs`/`ConvexValue` cannot describe a type converted through \
                         another type; implement `convex_fn::ConvexType` by hand",
                    ));
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            })?;
        }
        Ok(out)
    }
}

impl FieldAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut out = FieldAttrs::default();
        for attr in attrs {
            if attr.path().is_ident("serde") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("rename") {
                        out.rename = Some(rename_value(&meta)?);
                    } else if meta.path.is_ident("default") {
                        skip_value(&meta)?;
                        out.default = true;
                    } else if meta.path.is_ident("skip") {
                        out.skip = true;
                    } else if meta.path.is_ident("skip_serializing") {
                        out.skip_serializing = true;
                    } else if meta.path.is_ident("skip_deserializing") {
                        out.skip_deserializing = true;
                    } else if meta.path.is_ident("skip_serializing_if") {
                        skip_value(&meta)?;
                        out.skip_serializing_if = true;
                    } else if meta.path.is_ident("flatten") {
                        return Err(meta.error(
                            "`#[serde(flatten)]` is not supported by `ConvexArgs`/`ConvexValue`; \
                             implement `convex_fn::ConvexType` by hand",
                        ));
                    } else if ["with", "serialize_with", "deserialize_with"]
                        .iter()
                        .any(|k| meta.path.is_ident(k))
                    {
                        return Err(meta.error(
                            "custom (de)serializers change the wire format; describe the field \
                             with `#[convex(any)]` or `#[convex(id = \"table\")]`",
                        ));
                    } else {
                        skip_value(&meta)?;
                    }
                    Ok(())
                })?;
            } else if attr.path().is_ident("convex") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("id") {
                        out.id = Some(lit_str(&meta)?);
                    } else if meta.path.is_ident("any") {
                        out.any = true;
                    } else {
                        return Err(meta.error("expected `id = \"table\"` or `any`"));
                    }
                    Ok(())
                })?;
            }
        }
        Ok(out)
    }
}

impl VariantAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut out = VariantAttrs::default();
        for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    out.rename = Some(rename_value(&meta)?);
                } else if meta.path.is_ident("rename_all") {
                    let s = rename_value(&meta)?;
                    out.rename_all = RenameRule::parse(&s);
                } else if meta.path.is_ident("skip") {
                    out.skip = true;
                } else if meta.path.is_ident("untagged") || meta.path.is_ident("other") {
                    return Err(meta.error("unsupported variant attribute"));
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            })?;
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// rename rules

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            _ => return None,
        })
    }

    /// Applies to a field name (snake_case in Rust).
    pub fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::Lower | Self::Snake => field.to_string(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => {
                let mut out = String::new();
                let mut cap = true;
                for c in field.chars() {
                    if c == '_' {
                        cap = true;
                    } else if cap {
                        out.push(c.to_ascii_uppercase());
                        cap = false;
                    } else {
                        out.push(c);
                    }
                }
                out
            }
            Self::Camel => {
                let p = Self::Pascal.apply_to_field(field);
                p[..1].to_ascii_lowercase() + &p[1..]
            }
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake.apply_to_field(field).replace('_', "-"),
        }
    }

    /// Applies to a variant name (PascalCase in Rust).
    pub fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::Pascal => variant.to_string(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => variant[..1].to_ascii_lowercase() + &variant[1..],
            Self::Snake => {
                let mut s = String::new();
                for (i, c) in variant.char_indices() {
                    if i > 0 && c.is_uppercase() {
                        s.push('_');
                    }
                    s.push(c.to_ascii_lowercase());
                }
                s
            }
            Self::ScreamingSnake => Self::Snake.apply_to_variant(variant).to_ascii_uppercase(),
            Self::Kebab => Self::Snake.apply_to_variant(variant).replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake
                .apply_to_variant(variant)
                .replace('_', "-"),
        }
    }
}

// ---------------------------------------------------------------------------
// validators

fn v_path() -> TokenStream {
    quote!(::convex_fn::Validator)
}

fn leaf(ty: &Type) -> TokenStream {
    quote!(<#ty as ::convex_fn::ConvexType>::VALIDATOR)
}

/// `Some(inner)` if `ty` is `Option<inner>` (any path ending in `Option`).
fn option_inner(ty: &Type) -> Option<&Type> {
    generic_inner(ty, "Option")
}

fn generic_inner<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != name {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}

/// Validator expression for a field type, honouring `#[convex(id/any)]`.
fn type_validator(ty: &Type, attrs: &FieldAttrs) -> Result<TokenStream> {
    let v = v_path();
    if attrs.any {
        return Ok(quote!(#v::Any));
    }
    let Some(table) = &attrs.id else {
        return Ok(leaf(ty));
    };
    fn walk(ty: &Type, table: &str) -> Result<TokenStream> {
        let v = v_path();
        if let Some(inner) = option_inner(ty) {
            let i = walk(inner, table)?;
            return Ok(quote!(#v::Union(&[#i, #v::Null])));
        }
        if let Some(inner) = generic_inner(ty, "Vec") {
            let i = walk(inner, table)?;
            return Ok(quote!(#v::Array(&#i)));
        }
        if let Type::Path(p) = ty {
            if p.path.is_ident("String") {
                return Ok(quote!(#v::Id(#table)));
            }
        }
        Err(Error::new_spanned(
            ty,
            "`#[convex(id = \"..\")]` goes on a `String` (or an `Option`/`Vec` of one)",
        ))
    }
    walk(ty, table)
}

struct NamedField {
    wire_name: String,
    validator: TokenStream,
    optional: bool,
}

fn named_fields(
    fields: &syn::FieldsNamed,
    rename_all: Option<RenameRule>,
) -> Result<Vec<NamedField>> {
    let mut out = Vec::new();
    for f in &fields.named {
        let attrs = FieldAttrs::parse(&f.attrs)?;
        if attrs.skip {
            continue;
        }
        let ident = f.ident.as_ref().unwrap();
        let raw = ident.to_string();
        let raw = raw.strip_prefix("r#").unwrap_or(&raw);
        let wire_name = match (&attrs.rename, rename_all) {
            (Some(r), _) => r.clone(),
            (None, Some(rule)) => rule.apply_to_field(raw),
            (None, None) => raw.to_string(),
        };
        let v = v_path();
        let (validator, optional) = if let Some(inner) = option_inner(&f.ty) {
            let inner_v = type_validator(inner, &attrs)?;
            if attrs.skip_serializing_if {
                (inner_v, true)
            } else {
                // `None` serializes as `null`, a missing field deserializes as `None`.
                (quote!(#v::Union(&[#inner_v, #v::Null])), true)
            }
        } else {
            let optional = attrs.default || attrs.skip_serializing || attrs.skip_deserializing;
            (type_validator(&f.ty, &attrs)?, optional)
        };
        out.push(NamedField {
            wire_name,
            validator,
            optional,
        });
    }
    Ok(out)
}

fn field_tokens(fields: &[NamedField]) -> Vec<TokenStream> {
    fields
        .iter()
        .map(|f| {
            let name = &f.wire_name;
            let v = &f.validator;
            let opt = f.optional;
            quote!(::convex_fn::Field::new(#name, &#v, #opt))
        })
        .collect()
}

fn object_of(fields: &[NamedField]) -> TokenStream {
    let v = v_path();
    let toks = field_tokens(fields);
    quote!(#v::Object(&[#(#toks),*]))
}

fn struct_validator(fields: &Fields, c: &ContainerAttrs) -> Result<TokenStream> {
    let v = v_path();
    match fields {
        Fields::Named(n) => {
            if c.transparent {
                let live: Vec<_> = n.named.iter().collect();
                if live.len() != 1 {
                    return Err(Error::new_spanned(n, "transparent needs exactly one field"));
                }
                return Ok(leaf(&live[0].ty));
            }
            Ok(object_of(&named_fields(n, c.rename_all)?))
        }
        Fields::Unnamed(u) if u.unnamed.len() == 1 => {
            let f = u.unnamed.first().unwrap();
            type_validator(&f.ty, &FieldAttrs::parse(&f.attrs)?)
        }
        Fields::Unnamed(u) => Err(Error::new_spanned(
            u,
            "tuple structs serialize as arrays; use a struct with named fields or implement \
             `convex_fn::ConvexType` by hand",
        )),
        Fields::Unit => Ok(quote!(#v::Null)),
    }
}

fn enum_validator(e: &DataEnum, c: &ContainerAttrs) -> Result<TokenStream> {
    let v = v_path();
    let mut members = Vec::new();
    for var in &e.variants {
        let va = VariantAttrs::parse(&var.attrs)?;
        if va.skip {
            continue;
        }
        let raw = var.ident.to_string();
        let name = match (&va.rename, c.rename_all) {
            (Some(r), _) => r.clone(),
            (None, Some(rule)) => rule.apply_to_variant(&raw),
            (None, None) => raw.clone(),
        };
        // The payload (if any) as a validator, plus its named fields for the
        // internally tagged case.
        enum Payload {
            Unit,
            Newtype(TokenStream),
            Named(Vec<NamedField>),
        }
        let payload = match &var.fields {
            Fields::Unit => Payload::Unit,
            Fields::Unnamed(u) if u.unnamed.len() == 1 => {
                let f = u.unnamed.first().unwrap();
                Payload::Newtype(type_validator(&f.ty, &FieldAttrs::parse(&f.attrs)?)?)
            }
            Fields::Unnamed(u) => {
                return Err(Error::new_spanned(
                    u,
                    "tuple variants serialize as arrays; use a struct variant",
                ))
            }
            Fields::Named(n) => Payload::Named(named_fields(n, va.rename_all)?),
        };
        let tag_field =
            |tag: &str| quote!(::convex_fn::Field::new(#tag, &#v::LiteralStr(#name), false));
        let member =
            if c.untagged {
                match payload {
                    Payload::Unit => quote!(#v::Null),
                    Payload::Newtype(t) => t,
                    Payload::Named(f) => object_of(&f),
                }
            } else if let (Some(tag), Some(content)) = (&c.tag, &c.content) {
                let tf = tag_field(tag);
                match payload {
                    Payload::Unit => quote!(#v::Object(&[#tf])),
                    Payload::Newtype(t) => {
                        quote!(#v::Object(&[#tf, ::convex_fn::Field::new(#content, &#t, false)]))
                    }
                    Payload::Named(f) => {
                        let o = object_of(&f);
                        quote!(#v::Object(&[#tf, ::convex_fn::Field::new(#content, &#o, false)]))
                    }
                }
            } else if let Some(tag) = &c.tag {
                let tf = tag_field(tag);
                match payload {
                    Payload::Unit => quote!(#v::Object(&[#tf])),
                    Payload::Named(f) => {
                        let rest = field_tokens(&f);
                        quote!(#v::Object(&[#tf, #(#rest),*]))
                    }
                    Payload::Newtype(_) => return Err(Error::new_spanned(
                        var,
                        "internally tagged enums cannot hold newtype variants here; use a struct \
                         variant",
                    )),
                }
            } else {
                // Externally tagged.
                match payload {
                    Payload::Unit => quote!(#v::LiteralStr(#name)),
                    Payload::Newtype(t) => {
                        quote!(#v::Object(&[::convex_fn::Field::new(#name, &#t, false)]))
                    }
                    Payload::Named(f) => {
                        let o = object_of(&f);
                        quote!(#v::Object(&[::convex_fn::Field::new(#name, &#o, false)]))
                    }
                }
            };
        members.push(member);
    }
    Ok(quote!(#v::Union(&[#(#members),*])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_rules() {
        assert_eq!(RenameRule::Camel.apply_to_field("user_id"), "userId");
        assert_eq!(RenameRule::Pascal.apply_to_field("user_id"), "UserId");
        assert_eq!(
            RenameRule::ScreamingSnake.apply_to_field("user_id"),
            "USER_ID"
        );
        assert_eq!(RenameRule::Kebab.apply_to_field("user_id"), "user-id");
        assert_eq!(
            RenameRule::Snake.apply_to_variant("HttpError"),
            "http_error"
        );
        assert_eq!(RenameRule::Camel.apply_to_variant("HttpError"), "httpError");
        assert_eq!(
            RenameRule::ScreamingKebab.apply_to_variant("HttpError"),
            "HTTP-ERROR"
        );
    }

    fn expand_src(src: &str, mode: Mode) -> std::result::Result<String, String> {
        let input: DeriveInput = syn::parse_str(src).unwrap();
        expand(input, mode)
            .map(|t| t.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn args_rejects_enums_and_tuples() {
        let e = expand_src("enum E { A }", Mode::Args).unwrap_err();
        assert!(e.contains("ConvexValue"), "{e}");
        let e = expand_src("struct T(u8, u8);", Mode::Args).unwrap_err();
        assert!(e.contains("named fields"), "{e}");
    }

    #[test]
    fn rejects_flatten_and_custom_serializers() {
        let e = expand_src("struct S { #[serde(flatten)] a: A }", Mode::Args).unwrap_err();
        assert!(e.contains("flatten"), "{e}");
        let e = expand_src("struct S { #[serde(with = \"x\")] a: A }", Mode::Args).unwrap_err();
        assert!(e.contains("wire format"), "{e}");
    }

    #[test]
    fn id_attribute_needs_string() {
        let e = expand_src("struct S { #[convex(id = \"t\")] a: u32 }", Mode::Args).unwrap_err();
        assert!(e.contains("String"), "{e}");
        assert!(expand_src(
            "struct S { #[convex(id = \"t\")] a: Option<String> }",
            Mode::Args
        )
        .is_ok());
    }
}
