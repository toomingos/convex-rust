//! Static Convex validator descriptions for Rust types (`#[derive(ConvexArgs)]`).
//!
//! A [`Validator`] is a `const`-constructible tree that mirrors the Convex
//! validator JSON (`v.object({...})`, contracts/MANIFEST.md). It is rendered
//! to JSON at compile time by `const fn`s so `#[action]` can embed it in the
//! `convex:manifest` custom section without running any code.
//!
//! The description matches what `serde` (de)serialises, in the Convex JSON
//! format of `values.rs`: `String` -> `v.string()`, `f64` and every integer
//! type other than [`Int64`](crate::Int64) -> `v.float64()` (a JSON number),
//! `Int64` -> `v.int64()`, [`Bytes`](crate::Bytes) -> `v.bytes()`,
//! `Option<T>` as a struct field -> optional, `Vec<T>` -> `v.array(..)`,
//! `HashMap<String, V>` -> `v.record(..)`, `serde_json::Value` -> `v.any()`.

use std::collections::{BTreeMap, HashMap};

/// A Convex validator. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validator {
    Any,
    Null,
    Float64,
    Int64,
    Boolean,
    String,
    Bytes,
    /// `v.id("table")`.
    Id(&'static str),
    /// `v.literal("text")`.
    LiteralStr(&'static str),
    Array(&'static Validator),
    /// `v.record(v.string(), value)`.
    Record(&'static Validator),
    Object(&'static [Field]),
    Union(&'static [Validator]),
}

/// One field of a [`Validator::Object`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub ty: &'static Validator,
    pub optional: bool,
}

impl Field {
    pub const fn new(name: &'static str, ty: &'static Validator, optional: bool) -> Self {
        Field { name, ty, optional }
    }
}

/// A type with a static Convex validator. Derive it with
/// `#[derive(ConvexArgs)]` (structs) or `#[derive(ConvexValue)]` (structs and
/// enums).
pub trait ConvexType {
    const VALIDATOR: Validator;
}

impl Validator {
    /// The validator as Convex validator JSON.
    pub fn to_json(&self) -> String {
        let n = write_validator(&mut [], 0, self, false);
        let mut buf = vec![0u8; n];
        write_validator(&mut buf, 0, self, true);
        String::from_utf8(buf).expect("validator JSON is UTF-8")
    }
}

// ---------------------------------------------------------------------------
// const JSON rendering. `write == false` only measures.

const fn put(buf: &mut [u8], pos: usize, s: &str, write: bool) -> usize {
    let b = s.as_bytes();
    if write {
        let mut i = 0;
        while i < b.len() {
            buf[pos + i] = b[i];
            i += 1;
        }
    }
    pos + b.len()
}

/// A JSON string body must not need escaping (names come from Rust
/// identifiers and `rename` attributes); fail the build otherwise.
const fn put_quoted(buf: &mut [u8], pos: usize, s: &str, write: bool) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        assert!(
            b[i] != b'"' && b[i] != b'\\' && b[i] >= 0x20,
            "names in a Convex validator cannot contain quotes, backslashes or control characters"
        );
        i += 1;
    }
    let pos = put(buf, pos, "\"", write);
    let pos = put(buf, pos, s, write);
    put(buf, pos, "\"", write)
}

const fn write_validator(buf: &mut [u8], pos: usize, v: &Validator, write: bool) -> usize {
    match v {
        Validator::Any => put(buf, pos, "{\"type\":\"any\"}", write),
        Validator::Null => put(buf, pos, "{\"type\":\"null\"}", write),
        Validator::Float64 => put(buf, pos, "{\"type\":\"number\"}", write),
        Validator::Int64 => put(buf, pos, "{\"type\":\"bigint\"}", write),
        Validator::Boolean => put(buf, pos, "{\"type\":\"boolean\"}", write),
        Validator::String => put(buf, pos, "{\"type\":\"string\"}", write),
        Validator::Bytes => put(buf, pos, "{\"type\":\"bytes\"}", write),
        Validator::Id(table) => {
            let pos = put(buf, pos, "{\"type\":\"id\",\"tableName\":", write);
            let pos = put_quoted(buf, pos, table, write);
            put(buf, pos, "}", write)
        }
        Validator::LiteralStr(s) => {
            let pos = put(buf, pos, "{\"type\":\"literal\",\"value\":", write);
            let pos = put_quoted(buf, pos, s, write);
            put(buf, pos, "}", write)
        }
        Validator::Array(inner) => {
            let pos = put(buf, pos, "{\"type\":\"array\",\"value\":", write);
            let pos = write_validator(buf, pos, inner, write);
            put(buf, pos, "}", write)
        }
        Validator::Record(inner) => {
            let pos = put(
                buf,
                pos,
                "{\"type\":\"record\",\"keys\":{\"type\":\"string\"},\"values\":{\"fieldType\":",
                write,
            );
            let pos = write_validator(buf, pos, inner, write);
            put(buf, pos, ",\"optional\":false}}", write)
        }
        Validator::Object(fields) => {
            let mut pos = put(buf, pos, "{\"type\":\"object\",\"value\":{", write);
            let mut i = 0;
            while i < fields.len() {
                if i > 0 {
                    pos = put(buf, pos, ",", write);
                }
                pos = put_quoted(buf, pos, fields[i].name, write);
                pos = put(buf, pos, ":{\"fieldType\":", write);
                pos = write_validator(buf, pos, fields[i].ty, write);
                pos = put(
                    buf,
                    pos,
                    if fields[i].optional {
                        ",\"optional\":true}"
                    } else {
                        ",\"optional\":false}"
                    },
                    write,
                );
                i += 1;
            }
            put(buf, pos, "}}", write)
        }
        Validator::Union(members) => {
            let mut pos = put(buf, pos, "{\"type\":\"union\",\"value\":[", write);
            let mut i = 0;
            while i < members.len() {
                if i > 0 {
                    pos = put(buf, pos, ",", write);
                }
                pos = write_validator(buf, pos, &members[i], write);
                i += 1;
            }
            put(buf, pos, "]}", write)
        }
    }
}

const fn is_object(v: &Validator) -> bool {
    matches!(v, Validator::Object(_))
}

// ---------------------------------------------------------------------------
// Manifest assembly, used by `#[action]`.

#[allow(clippy::too_many_arguments)]
const fn write_manifest(
    buf: &mut [u8],
    module_path: &str,
    name: &str,
    visibility: &str,
    kind: &str,
    args: Option<&Validator>,
    returns: Option<&Validator>,
    write: bool,
) -> usize {
    let pos = put(buf, 0, "{\"abiVersion\":1,\"modulePath\":", write);
    let pos = put_quoted(buf, pos, module_path, write);
    let pos = put(buf, pos, ",\"name\":", write);
    let pos = put_quoted(buf, pos, name, write);
    let pos = put(buf, pos, ",\"visibility\":", write);
    let pos = put_quoted(buf, pos, visibility, write);
    // Absent for actions (the default), so their manifests are unchanged.
    let pos = if kind.is_empty() {
        pos
    } else {
        let pos = put(buf, pos, ",\"kind\":", write);
        put_quoted(buf, pos, kind, write)
    };
    let mut pos = put(buf, pos, ",\"args\":", write);
    pos = match args {
        // Args are always an object validator; anything else stays unvalidated.
        Some(v) if is_object(v) => write_validator(buf, pos, v, write),
        _ => put(buf, pos, "null", write),
    };
    pos = put(buf, pos, ",\"returns\":", write);
    pos = match returns {
        Some(Validator::Any) | None => put(buf, pos, "null", write),
        Some(v) => write_validator(buf, pos, v, write),
    };
    put(buf, pos, "}\n", write)
}

#[doc(hidden)]
pub const fn manifest_len(
    module_path: &str,
    name: &str,
    visibility: &str,
    kind: &str,
    args: Option<&Validator>,
    returns: Option<&Validator>,
) -> usize {
    write_manifest(&mut [], module_path, name, visibility, kind, args, returns, false)
}

#[doc(hidden)]
pub const fn manifest_bytes<const N: usize>(
    module_path: &str,
    name: &str,
    visibility: &str,
    kind: &str,
    args: Option<&Validator>,
    returns: Option<&Validator>,
) -> [u8; N] {
    let mut out = [0u8; N];
    let n = write_manifest(&mut out, module_path, name, visibility, kind, args, returns, true);
    assert!(n == N);
    out
}

#[doc(hidden)]
pub fn manifest_string(
    module_path: &str,
    name: &str,
    visibility: &str,
    kind: &str,
    args: Option<&Validator>,
    returns: Option<&Validator>,
) -> String {
    let n = manifest_len(module_path, name, visibility, kind, args, returns);
    let mut buf = vec![0u8; n];
    write_manifest(&mut buf, module_path, name, visibility, kind, args, returns, true);
    String::from_utf8(buf).expect("manifest is UTF-8")
}

// ---------------------------------------------------------------------------
// "Does this type have a validator?" probing without specialization. Inherent
// associated items win over trait ones, and the inherent impl only applies when
// `T: ConvexType`:  `Probe::<T>::VALIDATOR` is `Some(..)` for types with a
// validator and `None` for every other type (with `NoValidator` in scope).

#[doc(hidden)]
pub struct Probe<T: ?Sized>(std::marker::PhantomData<T>);

impl<T: ConvexType> Probe<T> {
    pub const VALIDATOR: Option<&'static Validator> = Some(&T::VALIDATOR);
}

#[doc(hidden)]
pub trait NoValidator {
    const VALIDATOR: Option<&'static Validator> = None;
}

impl<T: ?Sized> NoValidator for Probe<T> {}

// ---------------------------------------------------------------------------
// Impls for std and SDK types.

macro_rules! convex_type {
    ($($t:ty => $v:expr),* $(,)?) => {$(
        impl ConvexType for $t { const VALIDATOR: Validator = $v; }
    )*};
}

convex_type! {
    bool => Validator::Boolean,
    String => Validator::String,
    f32 => Validator::Float64,
    f64 => Validator::Float64,
    i8 => Validator::Float64,
    i16 => Validator::Float64,
    i32 => Validator::Float64,
    i64 => Validator::Float64,
    isize => Validator::Float64,
    u8 => Validator::Float64,
    u16 => Validator::Float64,
    u32 => Validator::Float64,
    u64 => Validator::Float64,
    usize => Validator::Float64,
    () => Validator::Null,
    serde_json::Value => Validator::Any,
    crate::values::Int64 => Validator::Int64,
    crate::values::Float64 => Validator::Float64,
    crate::values::Bytes => Validator::Bytes,
}

impl<T: ConvexType> ConvexType for Vec<T> {
    const VALIDATOR: Validator = Validator::Array(&T::VALIDATOR);
}

impl<T: ConvexType, const N: usize> ConvexType for [T; N] {
    const VALIDATOR: Validator = Validator::Array(&T::VALIDATOR);
}

impl<T: ConvexType + ?Sized> ConvexType for Box<T> {
    const VALIDATOR: Validator = T::VALIDATOR;
}

/// `Option<T>` as a plain value (not a struct field): `T` or `null`.
impl<T: ConvexType> ConvexType for Option<T> {
    const VALIDATOR: Validator = Validator::Union(&[T::VALIDATOR, Validator::Null]);
}

impl<V: ConvexType, S> ConvexType for HashMap<String, V, S> {
    const VALIDATOR: Validator = Validator::Record(&V::VALIDATOR);
}

impl<V: ConvexType> ConvexType for BTreeMap<String, V> {
    const VALIDATOR: Validator = Validator::Record(&V::VALIDATOR);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_json() {
        const V: Validator = Validator::Object(&[
            Field::new("name", &Validator::String, false),
            Field::new("n", &Validator::Float64, true),
            Field::new("tags", &Validator::Array(&Validator::String), false),
            Field::new("owner", &Validator::Id("users"), false),
        ]);
        assert_eq!(
            V.to_json(),
            r#"{"type":"object","value":{"name":{"fieldType":{"type":"string"},"optional":false},"n":{"fieldType":{"type":"number"},"optional":true},"tags":{"fieldType":{"type":"array","value":{"type":"string"}},"optional":false},"owner":{"fieldType":{"type":"id","tableName":"users"},"optional":false}}}"#
        );
        let u = Validator::Union(&[Validator::LiteralStr("a"), Validator::Null]);
        assert_eq!(
            u.to_json(),
            r#"{"type":"union","value":[{"type":"literal","value":"a"},{"type":"null"}]}"#
        );
    }

    #[test]
    fn std_types() {
        assert_eq!(
            Vec::<String>::VALIDATOR.to_json(),
            r#"{"type":"array","value":{"type":"string"}}"#
        );
        assert_eq!(
            <HashMap<String, i64>>::VALIDATOR.to_json(),
            r#"{"type":"record","keys":{"type":"string"},"values":{"fieldType":{"type":"number"},"optional":false}}"#
        );
        assert_eq!(
            <Option<bool>>::VALIDATOR.to_json(),
            r#"{"type":"union","value":[{"type":"boolean"},{"type":"null"}]}"#
        );
    }

    struct Plain;
    struct Obj;
    impl ConvexType for Obj {
        const VALIDATOR: Validator = Validator::Object(&[]);
    }

    #[test]
    fn probe_distinguishes_types() {
        use NoValidator as _;
        assert!(Probe::<Plain>::VALIDATOR.is_none());
        assert!(Probe::<Obj>::VALIDATOR.is_some());
        assert!(Probe::<Vec<Obj>>::VALIDATOR.is_some());
    }

    #[test]
    fn manifest_shapes() {
        let obj = Probe::<Obj>::VALIDATOR;
        let m = manifest_string("app::m", "f", "public", "", obj, Some(&Validator::String));
        assert_eq!(
            m,
            "{\"abiVersion\":1,\"modulePath\":\"app::m\",\"name\":\"f\",\"visibility\":\"public\",\"args\":{\"type\":\"object\",\"value\":{}},\"returns\":{\"type\":\"string\"}}\n"
        );
        // Non-object args and `any` returns stay unvalidated.
        let m = manifest_string(
            "app::m",
            "f",
            "internal",
            "",
            Some(&Validator::String),
            Some(&Validator::Any),
        );
        assert!(m.contains("\"args\":null,\"returns\":null}"));
        let m = manifest_string("app::m", "f", "internal", "", None, None);
        assert!(m.contains("\"args\":null,\"returns\":null}"));
        let m = manifest_string("app::m", "h", "public", "http", None, None);
        assert_eq!(
            m,
            "{\"abiVersion\":1,\"modulePath\":\"app::m\",\"name\":\"h\",\"visibility\":\"public\",\"kind\":\"http\",\"args\":null,\"returns\":null}\n"
        );
    }
}
