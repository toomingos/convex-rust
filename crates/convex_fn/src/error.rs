use std::fmt;

use serde_json::Value;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An action error. Like `anyhow::Error`, any `std::error::Error` converts into
/// it with `?`. `Error::Convex` reaches clients as a `ConvexError` with `data`.
#[derive(Debug, Clone)]
pub enum Error {
    /// Application error with structured data (`throw new ConvexError(data)`).
    Convex { message: String, data: Value },
    /// Any other error; clients see "Server Error" plus the message in logs.
    Message(String),
}

/// Constructor for application errors, mirroring the JS `ConvexError` class.
pub struct ConvexError;

impl ConvexError {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(data: impl Into<Value>) -> Error {
        Error::convex(data)
    }
}

impl Error {
    /// An application error with structured data (`throw new ConvexError(data)`).
    pub fn convex(data: impl Into<Value>) -> Self {
        let data = data.into();
        let message = match &data {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        Error::Convex { message, data }
    }

    /// The `ConvexError` data, if this is an application error.
    pub fn data(&self) -> Option<&Value> {
        match self {
            Error::Convex { data, .. } => Some(data),
            Error::Message(_) => None,
        }
    }

    pub fn msg(message: impl Into<String>) -> Self {
        Error::Message(message.into())
    }

    pub fn message(&self) -> &str {
        match self {
            Error::Convex { message, .. } | Error::Message(message) => message,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        match self {
            Error::Convex { message, data } => {
                serde_json::json!({ "message": message, "data": data, "kind": "convex" })
            }
            Error::Message(message) => serde_json::json!({ "message": message, "kind": "user" }),
        }
    }

    /// Parses `{"message": …, "data"?: …}` as returned in task events.
    pub(crate) fn from_json(v: &Value) -> Self {
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        match v.get("data") {
            // A present `data` (even null) is a ConvexError, like `new ConvexError(null)` in JS.
            Some(data) => Error::Convex {
                message,
                data: data.clone(),
            },
            None => Error::Message(message),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl<E: std::error::Error> From<E> for Error {
    fn from(e: E) -> Self {
        Error::Message(e.to_string())
    }
}

/// Formats the panic report: `Uncaught panic: <message> at <file:line:col>`.
/// The host (`PANIC_LOG_PREFIX` in the fork's rust/core.rs) takes the last such
/// level-3 log line as the error message when the abort trap follows.
#[cfg_attr(
    not(any(target_arch = "wasm32", convex_native, test)),
    allow(dead_code)
)]
pub(crate) fn format_panic(message: &str, location: Option<(&str, u32, u32)>) -> String {
    match location {
        Some((file, line, col)) => format!("Uncaught panic: {message} at {file}:{line}:{col}"),
        None => format!("Uncaught panic: {message}"),
    }
}

/// Reports panics (message + location) as an error log line before the abort
/// trap, so the host can surface them instead of "unreachable".
#[cfg(any(target_arch = "wasm32", convex_native))]
pub(crate) fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "Box<dyn Any>".to_string());
            let location = info.location().map(|l| (l.file(), l.line(), l.column()));
            #[cfg(convex_native)]
            crate::native::record_panic(match location {
                Some((file, line, col)) => format!("{message} at {file}:{line}:{col}"),
                None => message.clone(),
            });
            crate::sys::log_line(
                crate::wire::LogLevel::Error,
                &format_panic(&message, location),
            );
            // Distinguishes the real panic line from a user log line with the same prefix.
            crate::sys::panic_marker();
        }));
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{format_panic, ConvexError, Error};

    #[test]
    fn convex_error_json_round_trip() {
        let e = Error::convex(json!({ "code": 7, "tags": ["a"] }));
        assert_eq!(e.data(), Some(&json!({ "code": 7, "tags": ["a"] })));
        let j = e.to_json();
        assert_eq!(j["kind"], "convex");
        let back = Error::from_json(&j);
        assert_eq!(back.data(), e.data());
        assert_eq!(back.message(), e.message());
        // String data uses the string itself as message.
        let s = ConvexError::new("nope");
        assert_eq!(s.message(), "nope");
        assert_eq!(Error::from_json(&s.to_json()).data(), Some(&json!("nope")));
    }

    #[test]
    fn message_error_json_round_trip() {
        let e = Error::msg("boom");
        assert_eq!(e.data(), None);
        let j = e.to_json();
        assert_eq!(j, json!({ "message": "boom", "kind": "user" }));
        let back = Error::from_json(&j);
        assert_eq!(back.data(), None);
        assert_eq!(back.message(), "boom");
        // Missing message falls back.
        assert_eq!(Error::from_json(&json!({})).message(), "unknown error");
    }

    #[test]
    fn panic_line_has_message_and_location() {
        assert_eq!(
            format_panic("kaboom", Some(("src/spec.rs", 12, 5))),
            "Uncaught panic: kaboom at src/spec.rs:12:5"
        );
    }
}
