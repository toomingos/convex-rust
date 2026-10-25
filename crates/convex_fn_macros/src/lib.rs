//! Proc macros for `convex_fn`. See `contracts/ABI.md` and `contracts/MANIFEST.md`.

mod derive;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, DeriveInput, FnArg, GenericArgument, ItemFn, Meta, PathArguments,
    ReturnType, Token, Type,
};

/// Marks an `async fn(&Ctx, Args) -> convex_fn::Result<R>` as a Convex action.
///
/// `#[action]` makes it public (`action`), `#[action(internal)]` makes it an
/// `internalAction`. `Args` must implement `serde::DeserializeOwned` and `R`
/// `serde::Serialize`.
///
/// On every target this emits the `convex_run:<module_path>:<name>` export. The
/// manifest entry goes into a `convex:manifest` custom section on `wasm32` and
/// into an exported `convex_mf:<module_path>:<name>` static elsewhere. The
/// function itself is left as-is so `cargo test` can call it directly through
/// `convex_fn_test`.
#[proc_macro_attribute]
pub fn action(attr: TokenStream, item: TokenStream) -> TokenStream {
    let metas = parse_macro_input!(
        attr with syn::punctuated::Punctuated::<Meta, Token![,]>::parse_terminated
    );
    let mut visibility = "public";
    for meta in &metas {
        match meta {
            Meta::Path(p) if p.is_ident("internal") => visibility = "internal",
            other => {
                return syn::Error::new_spanned(other, "expected `internal` or nothing")
                    .to_compile_error()
                    .into()
            }
        }
    }

    let func = parse_macro_input!(item as ItemFn);
    if let Err(e) = check_signature(&func) {
        return e.to_compile_error().into();
    }
    expand_action(&func, visibility, false).into()
}

/// Marks an `async fn(&Ctx, HttpRequest) -> convex_fn::Result<HttpResponse>` as a Convex HTTP action.
///
/// It becomes an `httpAction` that is routed in `convex/http.ts` (`http.route({ path, method, handler:
/// rust.<module>.<name> })`). The request body is read as a stream (`HttpRequest::chunk`) or whole
/// (`bytes`, `text`, `json`); the response is buffered or streamed (`HttpResponse::streaming`). The
/// manifest entry has `"kind":"http"` and no validators. V8 host (cloud) only for now.
#[proc_macro_attribute]
pub fn http_action(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(proc_macro2::Span::call_site(), "`#[http_action]` takes no arguments")
            .to_compile_error()
            .into();
    }
    let func = parse_macro_input!(item as ItemFn);
    if let Err(e) = check_signature(&func) {
        return e.to_compile_error().into();
    }
    expand_action(&func, "public", true).into()
}

/// The expansion of `#[action]` for a validated function.
fn expand_action(func: &ItemFn, visibility: &str, http: bool) -> proc_macro2::TokenStream {
    let name = &func.sig.ident;
    let name_str = name.to_string();
    let export_fn = format_ident!("__convex_run_{}", name);
    let kind = if http { "http" } else { "" };
    let (args_ty, returns_ty): (Type, Type) = if http {
        (syn::parse_quote!(::convex_fn::__rt::NoReturnType), syn::parse_quote!(::convex_fn::__rt::NoReturnType))
    } else {
        (args_type(func), returns_type(func))
    };
    let run = if http {
        quote! { ::convex_fn::__rt::run_http_export(args_ptr, args_len, |ctx, req| async move { #name(&ctx, req).await }) }
    } else {
        quote! { ::convex_fn::__rt::run_export(args_ptr, args_len, |ctx, args| async move { #name(&ctx, args).await }) }
    };

    // Split builds (`convex-rust build`, zero-config per-module wasm): the CLI compiles the crate once per
    // Convex module with `--cfg convex_split --cfg 'convex_keep="<fn>"'` for that module's functions. A
    // function without the cfg loses its exports (the linker's GC roots), so wasm-ld drops its code and
    // everything only it uses. Without `convex_split` (cargo test, plain builds) nothing is gated.
    quote! {
        #[allow(dead_code)]
        #func

        #[allow(unexpected_cfgs)]
        const _: () = {
            #[cfg(any(not(convex_split), convex_keep = #name_str))]
            #[doc(hidden)]
            #[export_name = concat!("convex_run:", module_path!(), ":", #name_str)]
            pub extern "C" fn #export_fn(args_ptr: *mut u8, args_len: usize) -> i64 {
                #run
            }

            // `Probe::<T>::VALIDATOR` is `Some(..)` when `T` implements
            // `ConvexType` (derive `ConvexArgs`/`ConvexValue`), else `None`.
            use ::convex_fn::__rt::NoValidator as _;
            const __ARGS: ::core::option::Option<&'static ::convex_fn::Validator> =
                ::convex_fn::__rt::Probe::<#args_ty>::VALIDATOR;
            const __RETURNS: ::core::option::Option<&'static ::convex_fn::Validator> =
                ::convex_fn::__rt::Probe::<#returns_ty>::VALIDATOR;
            const __LEN: usize = ::convex_fn::__rt::manifest_len(
                module_path!(), #name_str, #visibility, #kind, __ARGS, __RETURNS,
            );
            #[cfg(all(target_arch = "wasm32", any(not(convex_split), convex_keep = #name_str)))]
            #[used]
            #[link_section = "convex:manifest"]
            static __CONVEX_MANIFEST: [u8; __LEN] = ::convex_fn::__rt::manifest_bytes::<__LEN>(
                module_path!(), #name_str, #visibility, #kind, __ARGS, __RETURNS,
            );
            // Native: an exported symbol, found by name in the dynamic symbol table.
            #[cfg(all(not(target_arch = "wasm32"), any(not(convex_split), convex_keep = #name_str)))]
            #[used]
            #[doc(hidden)]
            #[export_name = concat!("convex_mf:", module_path!(), ":", #name_str)]
            pub static __CONVEX_MANIFEST: [u8; __LEN] = ::convex_fn::__rt::manifest_bytes::<__LEN>(
                module_path!(), #name_str, #visibility, #kind, __ARGS, __RETURNS,
            );
        };
    }
}

/// The type of the second parameter.
fn args_type(func: &ItemFn) -> Type {
    match func.sig.inputs.iter().nth(1) {
        Some(FnArg::Typed(pt)) => (*pt.ty).clone(),
        _ => syn::parse_quote!(()),
    }
}

/// `R` of `Result<R>` / `convex_fn::Result<R>` / `Result<R, E>`; `()` otherwise
/// (no validator).
fn returns_type(func: &ItemFn) -> Type {
    if let ReturnType::Type(_, ty) = &func.sig.output {
        if let Type::Path(p) = &**ty {
            if let Some(seg) = p.path.segments.last() {
                if seg.ident == "Result" {
                    if let PathArguments::AngleBracketed(a) = &seg.arguments {
                        if let Some(GenericArgument::Type(t)) = a.args.first() {
                            return t.clone();
                        }
                    }
                }
            }
        }
    }
    syn::parse_quote!(::convex_fn::__rt::NoReturnType)
}

/// Derives `convex_fn::ConvexType` for action arguments: a static Convex
/// `v.object({...})` validator that matches what serde (de)serialises.
/// `#[action]` embeds it in the `convex:manifest`, so the backend validates
/// arguments like it does for TypeScript functions and the dashboard shows the
/// argument form.
///
/// ```ignore
/// #[derive(Deserialize, ConvexArgs)]
/// struct Args {
///     body: String,
///     #[convex(id = "users")] author: String,
///     limit: Option<f64>,
/// }
/// ```
///
/// Field types: `String`, `bool`, floats and integers (`v.float64()`),
/// `convex_fn::Int64` (`v.int64()`), `convex_fn::Bytes` (`v.bytes()`),
/// `Vec<T>`, `Option<T>` (optional field), `HashMap<String, T>`,
/// `serde_json::Value` (`v.any()`) and other `ConvexArgs`/`ConvexValue` types.
/// `#[convex(id = "table")]` marks a `String` as `v.id("table")`,
/// `#[convex(any)]` as `v.any()`. It follows the serde attributes `rename`,
/// `rename_all`, `default` and `skip*`.
#[proc_macro_derive(ConvexArgs, attributes(convex, serde))]
pub fn derive_convex_args(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    derive::expand(input, derive::Mode::Args)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Like [`ConvexArgs`](derive@ConvexArgs) for any serde-shaped value: structs
/// and enums (externally, internally or adjacently tagged, or untagged).
/// Use it for return types and nested values.
#[proc_macro_derive(ConvexValue, attributes(convex, serde))]
pub fn derive_convex_value(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    derive::expand(input, derive::Mode::Value)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn check_signature(func: &ItemFn) -> syn::Result<()> {
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            func.sig.fn_token,
            "actions must be `async fn`",
        ));
    }
    let inputs: Vec<&FnArg> = func.sig.inputs.iter().collect();
    if inputs.len() != 2 {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "a Convex action takes exactly two parameters: `(ctx: &Ctx, args: Args)`",
        ));
    }
    match inputs[0] {
        FnArg::Receiver(r) => {
            return Err(syn::Error::new_spanned(r, "actions cannot take `self`"));
        }
        FnArg::Typed(pt) => match &*pt.ty {
            Type::Reference(r) if r.mutability.is_none() => {}
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "the first parameter must be `ctx: &Ctx` (a shared reference)",
                ))
            }
        },
    }
    if let FnArg::Receiver(r) = inputs[1] {
        return Err(syn::Error::new_spanned(r, "actions cannot take `self`"));
    }
    if matches!(func.sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "actions must return `convex_fn::Result<R>`",
        ));
    }
    if !func.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &func.sig.generics,
            "actions cannot be generic",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(src: &str) -> Result<(), String> {
        let f: ItemFn = syn::parse_str(src).unwrap();
        check_signature(&f).map_err(|e| e.to_string())
    }

    fn expansion(src: &str, vis: &str) -> String {
        let f: ItemFn = syn::parse_str(src).unwrap();
        let ts = expand_action(&f, vis, false);
        // Normalise whitespace so the assertions do not depend on token printing.
        ts.to_string().split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn expansion_exports_on_all_targets() {
        let out = expansion("async fn f(ctx: &Ctx, a: A) -> Result<R> { todo!() }", "public");
        // The run export is not gated on the target.
        let run = out.find("convex_run:").expect("run export");
        assert!(!out[..run].contains("# [cfg (target_arch"), "{out}");
        assert!(out.contains("pub extern \"C\" fn __convex_run_f"), "{out}");
        assert!(out.contains("__rt :: run_export"), "{out}");
        // wasm manifest: custom section; native manifest: exported symbol.
        assert!(out.contains(
            "# [cfg (all (target_arch = \"wasm32\" , any (not (convex_split) , convex_keep = \"f\")))] # [used] # [link_section = \"convex:manifest\"] static __CONVEX_MANIFEST"
        ), "{out}");
        assert!(out.contains(
            "# [cfg (all (not (target_arch = \"wasm32\") , any (not (convex_split) , convex_keep = \"f\")))] # [used] # [doc (hidden)] # [export_name = concat ! (\"convex_mf:\" , module_path ! () , \":\" , \"f\")] pub static __CONVEX_MANIFEST"
        ), "{out}");
        assert!(!out.contains("convex_native"), "{out}");
    }

    #[test]
    fn http_expansion_runs_the_http_export_and_marks_the_manifest() {
        let f: ItemFn = syn::parse_str("async fn h(ctx: &Ctx, r: HttpRequest) -> Result<HttpResponse> { todo!() }").unwrap();
        let out = expand_action(&f, "public", true).to_string().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(out.contains("__rt :: run_http_export"), "{out}");
        assert!(!out.contains("__rt :: run_export"), "{out}");
        assert!(out.contains("\"public\" , \"http\" , __ARGS , __RETURNS"), "{out}");
        assert!(out.contains("Probe :: < :: convex_fn :: __rt :: NoReturnType >"), "{out}");
        // Actions carry an empty kind (omitted from the manifest).
        let a = expansion("async fn f(ctx: &Ctx, a: A) -> Result<R> { todo!() }", "public");
        assert!(a.contains("\"public\" , \"\" , __ARGS"), "{a}");
    }

    #[test]
    fn expansion_internal_visibility() {
        let out = expansion("async fn g(ctx: &Ctx, a: A) -> Result<R> { todo!() }", "internal");
        assert!(out.contains("\"internal\""), "{out}");
        assert!(out.contains("\"convex_mf:\""), "{out}");
    }

    #[test]
    fn accepts_valid_signature() {
        assert!(check("async fn f(ctx: &Ctx, a: Args) -> Result<R> { todo!() }").is_ok());
        assert!(check("async fn f(_: &Ctx, _: ()) -> convex_fn::Result<()> { Ok(()) }").is_ok());
    }

    #[test]
    fn rejects_non_async() {
        let e = check("fn f(ctx: &Ctx, a: A) -> Result<R> { todo!() }").unwrap_err();
        assert!(e.contains("async fn"), "{e}");
    }

    #[test]
    fn rejects_wrong_arity() {
        for src in [
            "async fn f(ctx: &Ctx) -> Result<R> { todo!() }",
            "async fn f(ctx: &Ctx, a: A, b: B) -> Result<R> { todo!() }",
            "async fn f() -> Result<R> { todo!() }",
        ] {
            let e = check(src).unwrap_err();
            assert!(e.contains("exactly two parameters"), "{e}");
        }
    }

    #[test]
    fn rejects_bad_ctx_and_receivers() {
        let e = check("async fn f(ctx: Ctx, a: A) -> Result<R> { todo!() }").unwrap_err();
        assert!(e.contains("ctx: &Ctx"), "{e}");
        let e = check("async fn f(ctx: &mut Ctx, a: A) -> Result<R> { todo!() }").unwrap_err();
        assert!(e.contains("shared reference"), "{e}");
        let e = check("async fn f(&self, a: A) -> Result<R> { todo!() }").unwrap_err();
        assert!(e.contains("self"), "{e}");
    }

    #[test]
    fn rejects_generics_and_missing_return() {
        let e = check("async fn f<T>(ctx: &Ctx, a: T) -> Result<R> { todo!() }").unwrap_err();
        assert!(e.contains("generic"), "{e}");
        let e = check("async fn f(ctx: &Ctx, a: A) { }").unwrap_err();
        assert!(e.contains("convex_fn::Result"), "{e}");
    }
}
