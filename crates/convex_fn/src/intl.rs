//! Time zones and locale formatting through the host's `Intl` (`intl/*` sync ops, see
//! contracts/ABI.md "Optional sync ops: intl/*").
//!
//! A wasm32 guest has no time zone database: `chrono::Local` and `std` know only UTC. Asking for a
//! zone here never falls back to UTC; an unknown zone, or a host without these ops, is an error.
//! (To stay self-contained instead, `jiff` with its bundled tzdb or `chrono-tz` work on wasm32 too, at
//! roughly 0.4-0.5 MB of wasm per call, which the cloud pays at every cold call.)

use serde_json::{json, Value};

use crate::{Error, Result};

fn call(op: &str, args: Value) -> Result<Value> {
    let out = crate::sys::sync(op, args.to_string().as_bytes());
    let v: Value = serde_json::from_slice(&out)?;
    match v.get("error") {
        Some(err) => {
            let e = Error::from_json(err);
            let msg = e.message().to_string();
            if msg.contains("unknown sync op") {
                Err(Error::msg(format!(
                    "host lacks {op}: update convex-rust to >= {MIN_CONVEX_RUST} (V8 host); the wasmtime host has no intl ops, use jiff or chrono-tz with a bundled tzdb"
                )))
            } else {
                Err(e)
            }
        }
        None => Ok(v.get("ok").cloned().unwrap_or(Value::Null)),
    }
}

/// First `convex-rust` release that serves the `intl/*` ops on V8 hosts.
pub const MIN_CONVEX_RUST: &str = "0.2.0";

/// Offset of the IANA zone `tz` from UTC at `epoch_ms`, in seconds (positive east of UTC), DST included.
/// Errors on an unknown zone; never returns 0 as a stand-in.
pub fn tz_offset_secs(tz: &str, epoch_ms: f64) -> Result<i32> {
    let v = call("intl/tzOffset", json!({ "tz": tz, "epochMs": epoch_ms }))?;
    v.as_i64()
        .map(|n| n as i32)
        .ok_or_else(|| Error::msg(format!("intl/tzOffset: unexpected reply {v}")))
}

/// Converts a wall-clock time in `tz` (milliseconds since the epoch as if the wall clock were UTC) to the real
/// epoch milliseconds. In a DST gap or overlap (a wall-clock time that does not exist, or exists twice) the
/// result is one of the two candidate instants.
pub fn local_to_epoch_ms(tz: &str, local_ms: f64) -> Result<f64> {
    let first = tz_offset_secs(tz, local_ms)? as f64 * 1000.0;
    let guess = local_ms - first;
    let second = tz_offset_secs(tz, guess)? as f64 * 1000.0;
    Ok(if second == first { guess } else { local_ms - second })
}

/// `new Intl.DateTimeFormat(locale, options).format(epoch_ms)`. `options` is the Intl options object, for example
/// `{"timeZone": "Europe/Lisbon", "dateStyle": "full", "timeStyle": "short"}`. An unknown `timeZone` or locale
/// is an error.
pub fn format_date(epoch_ms: f64, locale: &str, options: Value) -> Result<String> {
    format("date", json!(epoch_ms), locale, options)
}

/// `new Intl.NumberFormat(locale, options).format(value)`, for example options
/// `{"style": "currency", "currency": "EUR"}`.
pub fn format_number(value: f64, locale: &str, options: Value) -> Result<String> {
    format("number", json!(value), locale, options)
}

fn format(kind: &str, value: Value, locale: &str, options: Value) -> Result<String> {
    let v = call(
        "intl/format",
        json!({ "kind": kind, "value": value, "locale": locale, "options": options }),
    )?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::msg(format!("intl/format: unexpected reply {v}")))
}
