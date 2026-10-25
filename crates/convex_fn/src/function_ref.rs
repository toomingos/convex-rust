//! Typed function references, as generated into `_generated/api.rs` by
//! `npx convex dev` (the Rust counterpart of `FunctionReference` in JS).

use std::marker::PhantomData;

/// A reference to a Convex function taking `A` and returning `R`.
///
/// ```ignore
/// ctx.run_query(api::messages::list, api::messages::ListArgs {}).await?;
/// ```
pub struct FunctionRef<A, R> {
    path: &'static str,
    _types: PhantomData<fn(A) -> R>,
}

impl<A, R> FunctionRef<A, R> {
    /// `path` is a Convex function path such as `"messages:list"`.
    pub const fn new(path: &'static str) -> Self {
        FunctionRef {
            path,
            _types: PhantomData,
        }
    }

    pub const fn path(&self) -> &'static str {
        self.path
    }
}

impl<A, R> Clone for FunctionRef<A, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<A, R> Copy for FunctionRef<A, R> {}

impl<A, R> std::fmt::Debug for FunctionRef<A, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FunctionRef({:?})", self.path)
    }
}

/// How a function is addressed on the wire: the key of the syscall args
/// (`name`, `functionHandle` or `reference`) plus its string value.
pub trait FunctionAddress {
    /// The path, `function://` handle or component reference path.
    fn function_path(&self) -> &str;

    /// The syscall args key the path goes under. Plain strings that start with
    /// `function://` are handles, as in JS `getFunctionAddress`.
    fn address_key(&self) -> &'static str {
        "name"
    }
}

fn str_key(s: &str) -> &'static str {
    if s.starts_with(FUNCTION_HANDLE_PREFIX) {
        "functionHandle"
    } else {
        "name"
    }
}

const FUNCTION_HANDLE_PREFIX: &str = "function://";

/// A serialized function handle (`function://...`), as returned by
/// `createFunctionHandle` in JS. Works with `ctx.run_*` and the scheduler, and
/// is stable across deployments of the same function.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionHandle(String);

impl FunctionHandle {
    pub fn new(handle: impl Into<String>) -> Self {
        FunctionHandle(handle.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A reference to a function of a component, by the serialized reference path
/// that JS `getFunctionAddress` sends as `reference` (the value stored under
/// `Symbol.for("toReferencePath")` of `components.<name>.<fn>`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentFunctionRef(String);

impl ComponentFunctionRef {
    pub fn new(reference_path: impl Into<String>) -> Self {
        ComponentFunctionRef(reference_path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Something `ctx.run_query` / `run_mutation` / `run_action` can call: a
/// string path (untyped, as before), a typed [`FunctionRef`], a
/// [`FunctionHandle`] or a [`ComponentFunctionRef`].
pub trait FunctionTarget<A, R>: FunctionAddress {}

impl FunctionAddress for &str {
    fn function_path(&self) -> &str {
        self
    }
    fn address_key(&self) -> &'static str {
        str_key(self)
    }
}
impl<A, R> FunctionTarget<A, R> for &str {}

impl FunctionAddress for String {
    fn function_path(&self) -> &str {
        self
    }
    fn address_key(&self) -> &'static str {
        str_key(self)
    }
}
impl<A, R> FunctionTarget<A, R> for String {}

impl FunctionAddress for &String {
    fn function_path(&self) -> &str {
        self
    }
    fn address_key(&self) -> &'static str {
        str_key(self)
    }
}
impl<A, R> FunctionTarget<A, R> for &String {}

impl<A, R> FunctionAddress for FunctionRef<A, R> {
    fn function_path(&self) -> &str {
        self.path
    }
}
impl<A, R> FunctionTarget<A, R> for FunctionRef<A, R> {}

impl<A, R> FunctionAddress for &FunctionRef<A, R> {
    fn function_path(&self) -> &str {
        self.path
    }
}
impl<A, R> FunctionTarget<A, R> for &FunctionRef<A, R> {}

impl FunctionAddress for FunctionHandle {
    fn function_path(&self) -> &str {
        &self.0
    }
    fn address_key(&self) -> &'static str {
        "functionHandle"
    }
}
impl<A, R> FunctionTarget<A, R> for FunctionHandle {}

impl FunctionAddress for &FunctionHandle {
    fn function_path(&self) -> &str {
        &self.0
    }
    fn address_key(&self) -> &'static str {
        "functionHandle"
    }
}
impl<A, R> FunctionTarget<A, R> for &FunctionHandle {}

impl FunctionAddress for ComponentFunctionRef {
    fn function_path(&self) -> &str {
        &self.0
    }
    fn address_key(&self) -> &'static str {
        "reference"
    }
}
impl<A, R> FunctionTarget<A, R> for ComponentFunctionRef {}

impl FunctionAddress for &ComponentFunctionRef {
    fn function_path(&self) -> &str {
        &self.0
    }
    fn address_key(&self) -> &'static str {
        "reference"
    }
}
impl<A, R> FunctionTarget<A, R> for &ComponentFunctionRef {}

/// The `{name | functionHandle | reference: path}` part of a syscall's args.
pub(crate) fn address_json(target: &impl FunctionAddress) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert(
        target.address_key().to_string(),
        serde_json::Value::String(target.function_path().to_string()),
    );
    serde_json::Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let r: FunctionRef<(), ()> = FunctionRef::new("a:b");
        assert_eq!(address_json(&r), serde_json::json!({"name": "a:b"}));
        assert_eq!(address_json(&"a:b"), serde_json::json!({"name": "a:b"}));
        assert_eq!(
            address_json(&"function://abc"),
            serde_json::json!({"functionHandle": "function://abc"})
        );
        assert_eq!(
            address_json(&FunctionHandle::new("function://x")),
            serde_json::json!({"functionHandle": "function://x"})
        );
        assert_eq!(
            address_json(&ComponentFunctionRef::new("_reference/childComponent/c/f")),
            serde_json::json!({"reference": "_reference/childComponent/c/f"})
        );
    }
}
