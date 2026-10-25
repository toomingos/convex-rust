//! The JSON wire format of contracts/ABI.md in one place: task requests the
//! guest submits, events the host delivers, and log levels. Used by `Ctx` and
//! the executor, and (through the hidden `__wire` export) by `convex_fn_test`
//! so the mock host does not re-spell the keys.

use base64::Engine;
use serde_json::{json, Value};

pub(crate) const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Host `log` import levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl LogLevel {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    pub fn from_i32(level: i32) -> Option<Self> {
        match level {
            0 => Some(Self::Debug),
            1 => Some(Self::Info),
            2 => Some(Self::Warn),
            3 => Some(Self::Error),
            _ => None,
        }
    }
}

// ---- task requests (guest -> host) ----

pub fn syscall_request(name: &str, args: Value) -> Value {
    json!({ "type": "syscall", "name": name, "args": args })
}

/// How a request body travels (contracts/ABI.md "Task requests").
#[derive(Clone, Copy, Debug)]
pub enum BodyRef<'a> {
    None,
    /// Base64 string inside the request JSON.
    Inline(&'a [u8]),
    /// `{"ptr","len"}` into guest memory (needs `net/caps` `bodyRef`).
    Ref(&'a [u8]),
    /// Fed by the guest through `net/body_write` (needs `requestStream`).
    Stream,
}

impl BodyRef<'_> {
    fn to_json(self) -> Value {
        match self {
            BodyRef::None => Value::Null,
            BodyRef::Inline(b) => Value::String(B64.encode(b)),
            BodyRef::Ref(b) => crate::net::bytes_ref(b),
            BodyRef::Stream => json!({ "stream": true }),
        }
    }
}

pub fn fetch_request(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> Value {
    fetch_request_with(
        url,
        method,
        headers,
        body.map_or(BodyRef::None, BodyRef::Inline),
    )
}

pub fn fetch_request_with(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: BodyRef<'_>,
) -> Value {
    json!({
        "type": "fetch",
        "url": url,
        "method": method,
        "headers": headers,
        "body": body.to_json(),
    })
}

pub fn sleep_request(ms: u64) -> Value {
    json!({ "type": "sleep", "ms": ms })
}

pub fn storage_store_request(data: &[u8], content_type: Option<&str>) -> Value {
    storage_store_request_with(BodyRef::Inline(data), content_type)
}

pub fn storage_store_request_with(body: BodyRef<'_>, content_type: Option<&str>) -> Value {
    json!({
        "type": "storageStore",
        "contentType": content_type,
        "body": body.to_json(),
    })
}

pub fn storage_get_request(storage_id: &str) -> Value {
    json!({ "type": "storageGet", "storageId": storage_id })
}

/// Starts the response of an HTTP action whose body is streamed (V8 host only).
pub fn http_start_request(status: u16, headers: &[(String, String)]) -> Value {
    json!({ "type": "httpStart", "status": status, "headers": headers })
}

/// One chunk of a streamed HTTP action response body.
pub fn http_write_request(data: &[u8]) -> Value {
    json!({ "type": "httpWrite", "body": B64.encode(data) })
}

/// Ends a streamed HTTP action response body.
pub fn http_end_request() -> Value {
    json!({ "type": "httpEnd" })
}

/// A task request as the host sees it (used by the mock host).
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Syscall {
        name: String,
        args: Value,
    },
    Sleep {
        ms: u64,
    },
    StorageStore {
        content_type: Option<String>,
        body: WireBody,
    },
    StorageGet {
        storage_id: String,
    },
    Fetch {
        url: String,
        method: String,
        headers: Vec<(String, String)>,
        body: WireBody,
    },
    HttpStart {
        status: u16,
        headers: Vec<(String, String)>,
    },
    HttpWrite {
        /// Base64 body, as sent.
        body: String,
    },
    HttpEnd,
    /// Unknown `type` (`None` when absent).
    Other(Option<String>),
}

/// A request body as received by a host.
#[derive(Clone, Debug, PartialEq)]
pub enum WireBody {
    None,
    /// Base64 body, as sent.
    Base64(String),
    /// A reference into guest memory.
    Ref {
        ptr: usize,
        len: usize,
    },
    /// Guest-fed (`net/body_write`).
    Stream,
}

impl WireBody {
    pub fn parse(v: &Value) -> Self {
        if let Some(s) = v.as_str() {
            return WireBody::Base64(s.to_string());
        }
        if v.get("stream").and_then(Value::as_bool) == Some(true) {
            return WireBody::Stream;
        }
        match (
            v.get("ptr").and_then(Value::as_u64),
            v.get("len").and_then(Value::as_u64),
        ) {
            (Some(p), Some(l)) => WireBody::Ref {
                ptr: p as usize,
                len: l as usize,
            },
            _ => WireBody::None,
        }
    }
}

impl Request {
    pub fn parse(req: &Value) -> Self {
        let s = |k: &str| req[k].as_str().unwrap_or_default().to_string();
        match req["type"].as_str() {
            Some("syscall") => Request::Syscall {
                name: s("name"),
                args: req["args"].clone(),
            },
            Some("sleep") => Request::Sleep {
                ms: req["ms"].as_u64().unwrap_or_default(),
            },
            Some("storageStore") => Request::StorageStore {
                content_type: req["contentType"].as_str().map(str::to_string),
                body: WireBody::parse(&req["body"]),
            },
            Some("storageGet") => Request::StorageGet {
                storage_id: s("storageId"),
            },
            Some("fetch") => Request::Fetch {
                url: s("url"),
                method: req["method"].as_str().unwrap_or("GET").to_string(),
                headers: serde_json::from_value(req["headers"].clone()).unwrap_or_default(),
                body: WireBody::parse(&req["body"]),
            },
            Some("httpStart") => Request::HttpStart {
                status: req["status"].as_u64().unwrap_or_default() as u16,
                headers: serde_json::from_value(req["headers"].clone()).unwrap_or_default(),
            },
            Some("httpWrite") => Request::HttpWrite { body: s("body") },
            Some("httpEnd") => Request::HttpEnd,
            other => Request::Other(other.map(str::to_string)),
        }
    }
}

// ---- task results (host -> guest) ----

/// The `ok` value of a `fetch` task: the response head.
pub fn fetch_head(
    status: u16,
    status_text: &str,
    headers: &[(String, String)],
    url: &str,
    body_stream_id: Option<&str>,
) -> Value {
    json!({
        "status": status,
        "statusText": status_text,
        "headers": headers,
        "url": url,
        "bodyStreamId": body_stream_id,
    })
}

/// The `ok` value of a `storageGet` task for an existing file.
pub fn storage_file_head(
    body_stream_id: &str,
    content_length: usize,
    content_type: Option<&str>,
) -> Value {
    json!({
        "bodyStreamId": body_stream_id,
        "contentLength": content_length,
        "contentType": content_type,
    })
}

/// `bodyStreamId` of a fetch / storageGet head, if any.
pub fn body_stream_id(head: &Value) -> Option<&str> {
    head.get("bodyStreamId").and_then(Value::as_str)
}

// ---- events (host -> guest) ----

/// A host event (contracts/ABI.md "Events").
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A task finished; `result` is `Err(error json)` when it failed.
    Done {
        task_id: i64,
        result: Result<Value, Value>,
    },
    /// A body stream chunk. `terminal` is true for `end` or any `error` key;
    /// `error` is set when the error is a string.
    Chunk {
        stream_id: String,
        data: Option<String>,
        error: Option<String>,
        terminal: bool,
    },
}

/// First byte of a binary chunk event (contracts/ABI.md "Binary chunk
/// events"): `0x01 | u8 id_len | stream id | raw bytes`. JSON events start
/// with `{`, so the two never collide. A host sends binary events only to a
/// module that exports `convex_binary_events`.
pub const BIN_CHUNK: u8 = 1;

/// Capability marker: its presence as an export tells the host this module
/// parses binary chunk events. Old modules lack it and keep getting JSON.
#[no_mangle]
pub extern "C" fn convex_binary_events() -> i32 {
    1
}

/// Typed view of a JSON event, borrowing from the input; `ok` / `error`
/// payloads are the only parts that become `Value`s.
#[derive(serde::Deserialize)]
struct RawEvent<'a> {
    #[serde(rename = "type", borrow)]
    ty: std::borrow::Cow<'a, str>,
    #[serde(rename = "taskId", default)]
    task_id: Option<i64>,
    #[serde(rename = "streamId", borrow, default)]
    stream_id: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow, default)]
    data: Option<std::borrow::Cow<'a, str>>,
    #[serde(default)]
    end: Option<bool>,
    #[serde(default)]
    ok: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}

/// A host event as the executor consumes it: chunk bytes are already decoded.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Done {
        task_id: i64,
        result: Result<Value, Value>,
    },
    Chunk {
        stream_id: String,
        data: Option<Vec<u8>>,
        error: Option<String>,
        terminal: bool,
    },
}

impl Incoming {
    /// Parses a JSON or binary event without building an intermediate `Value`
    /// for chunks; `None` for malformed or unknown events.
    /// Like [`Incoming::parse`], but takes the event buffer: a binary chunk reuses it (the
    /// header is shifted out in place) instead of allocating and copying the payload.
    pub fn parse_owned(mut bytes: Vec<u8>) -> Option<Self> {
        if bytes.first() == Some(&BIN_CHUNK) {
            let id_len = *bytes.get(1)? as usize;
            let id = std::str::from_utf8(bytes.get(2..2 + id_len)?).ok()?.to_string();
            bytes.drain(..2 + id_len);
            return Some(Incoming::Chunk {
                stream_id: id,
                data: Some(bytes),
                error: None,
                terminal: false,
            });
        }
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.first() == Some(&BIN_CHUNK) {
            let id_len = *bytes.get(1)? as usize;
            let id = std::str::from_utf8(bytes.get(2..2 + id_len)?).ok()?;
            return Some(Incoming::Chunk {
                stream_id: id.to_string(),
                data: Some(bytes[2 + id_len..].to_vec()),
                error: None,
                terminal: false,
            });
        }
        let ev: RawEvent = serde_json::from_slice(bytes).ok()?;
        match &*ev.ty {
            "done" => Some(Incoming::Done {
                task_id: ev.task_id.unwrap_or(-1),
                result: match ev.error {
                    Some(err) => Err(err),
                    None => Ok(ev.ok.unwrap_or(Value::Null)),
                },
            }),
            "chunk" => {
                let data = match ev.data {
                    Some(d) => {
                        let mut out = Vec::with_capacity(d.len() / 4 * 3 + 3);
                        // An undecodable chunk is dropped, as before.
                        B64.decode_vec(d.as_bytes(), &mut out).ok().map(|_| out)
                    }
                    None => None,
                };
                Some(Incoming::Chunk {
                    stream_id: ev.stream_id.map(|s| s.into_owned()).unwrap_or_default(),
                    data,
                    terminal: ev.error.is_some() || ev.end == Some(true),
                    error: ev.error.and_then(|e| e.as_str().map(str::to_string)),
                })
            }
            _ => None,
        }
    }
}

impl Event {
    /// Parses one JSON event; `None` if it is not JSON or of an unknown type.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        Some(match Incoming::parse(bytes)? {
            Incoming::Done { task_id, result } => Event::Done { task_id, result },
            Incoming::Chunk {
                stream_id,
                data,
                error,
                terminal,
            } => Event::Chunk {
                stream_id,
                data: data.map(|d| B64.encode(d)),
                error,
                terminal,
            },
        })
    }

    /// The wire JSON of this event (used by the mock host).
    pub fn to_json(&self) -> Value {
        match self {
            Event::Done {
                task_id,
                result: Ok(v),
            } => json!({ "type": "done", "taskId": task_id, "ok": v }),
            Event::Done {
                task_id,
                result: Err(e),
            } => json!({ "type": "done", "taskId": task_id, "error": e }),
            Event::Chunk {
                stream_id,
                data,
                error,
                terminal,
            } => {
                let mut v = json!({ "type": "chunk", "streamId": stream_id });
                if let Some(d) = data {
                    v["data"] = Value::String(d.clone());
                }
                if let Some(e) = error {
                    v["error"] = Value::String(e.clone());
                } else if *terminal {
                    v["end"] = Value::Bool(true);
                }
                v
            }
        }
    }

    pub fn done_ok(task_id: i64, ok: Value) -> Self {
        Event::Done {
            task_id,
            result: Ok(ok),
        }
    }

    pub fn done_err(task_id: i64, error: Value) -> Self {
        Event::Done {
            task_id,
            result: Err(error),
        }
    }

    pub fn chunk_data(stream_id: &str, bytes: &[u8]) -> Self {
        Event::Chunk {
            stream_id: stream_id.to_string(),
            data: Some(B64.encode(bytes)),
            error: None,
            terminal: false,
        }
    }

    pub fn chunk_end(stream_id: &str) -> Self {
        Event::Chunk {
            stream_id: stream_id.to_string(),
            data: None,
            error: None,
            terminal: true,
        }
    }

    pub fn chunk_error(stream_id: &str, message: &str) -> Self {
        Event::Chunk {
            stream_id: stream_id.to_string(),
            data: None,
            error: Some(message.to_string()),
            terminal: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_round_trip() {
        for ev in [
            Event::done_ok(3, json!({"a": 1})),
            Event::done_err(4, json!({"message": "x"})),
            Event::chunk_data("s", b"hi"),
            Event::chunk_end("s"),
            Event::chunk_error("s", "boom"),
        ] {
            let bytes = ev.to_json().to_string().into_bytes();
            assert_eq!(Event::parse(&bytes), Some(ev));
        }
        assert_eq!(Event::parse(b"{\"type\":\"nope\"}"), None);
        assert_eq!(Event::parse(b"not json"), None);
    }

    #[test]
    fn binary_chunk_events_parse() {
        let mut ev = vec![BIN_CHUNK, 3];
        ev.extend_from_slice(b"abc");
        ev.extend_from_slice(&[0, 255, 7]);
        assert_eq!(
            Incoming::parse(&ev),
            Some(Incoming::Chunk {
                stream_id: "abc".into(),
                data: Some(vec![0, 255, 7]),
                error: None,
                terminal: false
            })
        );
        assert_eq!(Incoming::parse_owned(ev.clone()), Incoming::parse(&ev));
        let json = br#"{"type":"chunk","streamId":"s","end":true}"#.to_vec();
        assert_eq!(Incoming::parse_owned(json.clone()), Incoming::parse(&json));
        // Truncated header.
        assert_eq!(Incoming::parse(&[BIN_CHUNK, 9, b'a']), None);
        assert_eq!(Incoming::parse_owned(vec![BIN_CHUNK, 9, b'a']), None);
        // JSON chunk decodes straight to bytes.
        assert_eq!(
            Incoming::parse(br#"{"type":"chunk","streamId":"s","data":"aGk="}"#),
            Some(Incoming::Chunk {
                stream_id: "s".into(),
                data: Some(b"hi".to_vec()),
                error: None,
                terminal: false
            })
        );
    }

    #[test]
    fn request_keys_are_stable() {
        let r = fetch_request(
            "http://x/",
            "POST",
            &[("a".into(), "b".into())],
            Some(b"hi"),
        );
        assert_eq!(
            r.to_string(),
            r#"{"body":"aGk=","headers":[["a","b"]],"method":"POST","type":"fetch","url":"http://x/"}"#
        );
        assert_eq!(sleep_request(5).to_string(), r#"{"ms":5,"type":"sleep"}"#);
        assert_eq!(
            storage_get_request("id").to_string(),
            r#"{"storageId":"id","type":"storageGet"}"#
        );
        assert_eq!(
            Request::parse(&storage_store_request(b"hi", Some("t/x"))),
            Request::StorageStore {
                content_type: Some("t/x".into()),
                body: WireBody::Base64("aGk=".into())
            }
        );
    }

    #[test]
    fn log_levels() {
        assert_eq!(LogLevel::Error.as_i32(), 3);
        assert_eq!(LogLevel::from_i32(0), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_i32(9), None);
    }
}
