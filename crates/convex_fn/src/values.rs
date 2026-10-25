//! Convex values that plain JSON can't carry, in the Convex JSON export
//! format: `{"$integer": …}`, `{"$bytes": …}`, `{"$float": …}`.
//! Everything else (strings, `f64`, bools, null, arrays, objects) is plain serde.

use base64::Engine;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn tagged<S: Serializer>(s: S, tag: &str, bytes: &[u8]) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut m = s.serialize_map(Some(1))?;
    m.serialize_entry(tag, &B64.encode(bytes))?;
    m.end()
}

fn untag<'de, D: Deserializer<'de>>(d: D, tag: &str) -> Result<Vec<u8>, D::Error> {
    let v = Value::deserialize(d)?;
    let s = v
        .get(tag)
        .and_then(Value::as_str)
        .ok_or_else(|| D::Error::custom(format!("expected {{\"{tag}\": <base64>}}")))?;
    B64.decode(s).map_err(D::Error::custom)
}

/// Convex `int64` (`bigint` in JS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Int64(pub i64);

impl Serialize for Int64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        tagged(s, "$integer", &self.0.to_le_bytes())
    }
}

impl<'de> Deserialize<'de> for Int64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let b: [u8; 8] = untag(d, "$integer")?
            .try_into()
            .map_err(|_| D::Error::custom("$integer must be 8 bytes"))?;
        Ok(Int64(i64::from_le_bytes(b)))
    }
}

/// Convex `bytes` (`ArrayBuffer` in JS).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bytes(pub Vec<u8>);

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        tagged(s, "$bytes", &self.0)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Bytes(untag(d, "$bytes")?))
    }
}

/// Convex `float64` that may be NaN, ±Infinity or -0 (plain `f64` covers the rest).
#[derive(Clone, Copy, Debug, Default)]
pub struct Float64(pub f64);

impl PartialEq for Float64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Serialize for Float64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let f = self.0;
        if f.is_finite() && !(f == 0.0 && f.is_sign_negative()) {
            s.serialize_f64(f)
        } else {
            tagged(s, "$float", &f.to_le_bytes())
        }
    }
}

impl<'de> Deserialize<'de> for Float64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        if let Some(f) = v.as_f64() {
            return Ok(Float64(f));
        }
        let s = v
            .get("$float")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("expected a number or {\"$float\": <base64>}"))?;
        let b: [u8; 8] = B64
            .decode(s)
            .map_err(D::Error::custom)?
            .try_into()
            .map_err(|_| D::Error::custom("$float must be 8 bytes"))?;
        Ok(Float64(f64::from_le_bytes(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        for f in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0, 1.5] {
            let j = serde_json::to_string(&Float64(f)).unwrap();
            assert_eq!(serde_json::from_str::<Float64>(&j).unwrap(), Float64(f));
        }
        let j = serde_json::to_string(&Int64(-2)).unwrap();
        assert_eq!(j, r#"{"$integer":"/v////////8="}"#);
        assert_eq!(serde_json::from_str::<Int64>(&j).unwrap(), Int64(-2));
        let j = serde_json::to_string(&Bytes(vec![1, 2, 3])).unwrap();
        assert_eq!(
            serde_json::from_str::<Bytes>(&j).unwrap(),
            Bytes(vec![1, 2, 3])
        );
    }
}
