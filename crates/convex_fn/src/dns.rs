//! DNS lookups over HTTPS (RFC 8484 JSON API, as served by Cloudflare and
//! Google). A wasm action has no sockets, so `dns.resolve*` goes through the
//! host `fetch` instead.
//!
//! ```ignore
//! let mx = convex_fn::dns::resolve("example.com", RecordType::Mx).await?;
//! for a in mx {
//!     if let Record::Mx { preference, exchange } = a.record { /* ... */ }
//! }
//! ```
//!
//! The default endpoint is `https://cloudflare-dns.com/dns-query`; use
//! [`Resolver::new`] for another JSON endpoint (for example
//! `https://dns.google/resolve`).

use std::net::{Ipv4Addr, Ipv6Addr};

use serde_json::Value;

use crate::{Ctx, Error, FetchRequest, Result};

/// Cloudflare's DNS-over-HTTPS JSON endpoint.
pub const CLOUDFLARE: &str = "https://cloudflare-dns.com/dns-query";
/// Google's DNS-over-HTTPS JSON endpoint.
pub const GOOGLE: &str = "https://dns.google/resolve";

/// Record types [`resolve`] understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordType {
    A,
    Ns,
    Cname,
    Mx,
    Txt,
    Aaaa,
    Srv,
}

impl RecordType {
    /// The IANA type number.
    pub const fn number(self) -> u16 {
        match self {
            RecordType::A => 1,
            RecordType::Ns => 2,
            RecordType::Cname => 5,
            RecordType::Mx => 15,
            RecordType::Txt => 16,
            RecordType::Aaaa => 28,
            RecordType::Srv => 33,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Ns => "NS",
            RecordType::Cname => "CNAME",
            RecordType::Mx => "MX",
            RecordType::Txt => "TXT",
            RecordType::Aaaa => "AAAA",
            RecordType::Srv => "SRV",
        }
    }

    /// Parses `"MX"`, `"txt"`, ... (ASCII case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        [
            RecordType::A,
            RecordType::Ns,
            RecordType::Cname,
            RecordType::Mx,
            RecordType::Txt,
            RecordType::Aaaa,
            RecordType::Srv,
        ]
        .into_iter()
        .find(|t| t.as_str().eq_ignore_ascii_case(s))
    }
}

/// A resolved record's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    /// Host name, without the trailing dot.
    Ns(String),
    Cname(String),
    Mx {
        preference: u16,
        exchange: String,
    },
    /// The character-strings of the record concatenated, as Node's `resolveTxt(...).join("")`.
    Txt(String),
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        target: String,
    },
}

/// One answer: the owner name, its TTL in seconds and the data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub name: String,
    pub ttl: u32,
    pub record: Record,
}

/// A DNS-over-HTTPS client.
#[derive(Clone, Debug)]
pub struct Resolver {
    endpoint: String,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new(CLOUDFLARE)
    }
}

impl Resolver {
    /// A resolver using the given JSON DoH endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    /// Looks up `name`. A name that does not exist (NXDOMAIN) or has no
    /// records of `rtype` gives an empty list; a server failure is an error.
    pub async fn resolve(&self, name: &str, rtype: RecordType) -> Result<Vec<Answer>> {
        let url = self.url(name, rtype);
        let resp = Ctx::default()
            .fetch(FetchRequest::get(url).header("accept", "application/dns-json"))
            .await?;
        if resp.status != 200 {
            return Err(Error::msg(format!(
                "dns: {} answered {} for {name} {}",
                self.endpoint,
                resp.status,
                rtype.as_str()
            )));
        }
        parse_response(&resp.body, rtype)
    }

    fn url(&self, name: &str, rtype: RecordType) -> String {
        let sep = if self.endpoint.contains('?') {
            '&'
        } else {
            '?'
        };
        format!(
            "{}{sep}name={}&type={}",
            self.endpoint,
            percent_encode(name),
            rtype.as_str()
        )
    }
}

/// [`Resolver::resolve`] with the default (Cloudflare) endpoint.
pub async fn resolve(name: &str, rtype: RecordType) -> Result<Vec<Answer>> {
    Resolver::default().resolve(name, rtype).await
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Parses a DoH JSON response, keeping the answers of type `rtype` (a CNAME
/// chain in front of an A answer is dropped).
pub fn parse_response(body: &[u8], rtype: RecordType) -> Result<Vec<Answer>> {
    let v: Value = serde_json::from_slice(body)?;
    // RCODE: 0 NOERROR, 3 NXDOMAIN (no such name: nothing to return).
    match v.get("Status").and_then(Value::as_u64) {
        Some(0) | Some(3) => {}
        Some(code) => {
            return Err(Error::msg(format!("dns: server failure (RCODE {code})")));
        }
        None => return Err(Error::msg("dns: response has no Status")),
    }
    let mut out = vec![];
    for a in v
        .get("Answer")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        if a.get("type").and_then(Value::as_u64) != Some(u64::from(rtype.number())) {
            continue;
        }
        let data = a.get("data").and_then(Value::as_str).unwrap_or_default();
        let Some(record) = parse_data(rtype, data) else {
            return Err(Error::msg(format!(
                "dns: cannot parse {} data {data:?}",
                rtype.as_str()
            )));
        };
        out.push(Answer {
            name: trim_dot(a.get("name").and_then(Value::as_str).unwrap_or_default()),
            ttl: a.get("TTL").and_then(Value::as_u64).unwrap_or(0) as u32,
            record,
        });
    }
    Ok(out)
}

fn trim_dot(s: &str) -> String {
    s.strip_suffix('.').unwrap_or(s).to_string()
}

fn parse_data(rtype: RecordType, data: &str) -> Option<Record> {
    let mut it = data.split_whitespace();
    Some(match rtype {
        RecordType::A => Record::A(data.trim().parse().ok()?),
        RecordType::Aaaa => Record::Aaaa(data.trim().parse().ok()?),
        RecordType::Ns => Record::Ns(trim_dot(data.trim())),
        RecordType::Cname => Record::Cname(trim_dot(data.trim())),
        RecordType::Mx => Record::Mx {
            preference: it.next()?.parse().ok()?,
            exchange: trim_dot(it.next()?),
        },
        RecordType::Srv => Record::Srv {
            priority: it.next()?.parse().ok()?,
            weight: it.next()?.parse().ok()?,
            port: it.next()?.parse().ok()?,
            target: trim_dot(it.next()?),
        },
        RecordType::Txt => Record::Txt(unquote_txt(data)),
    })
}

/// `"a" "b"` -> `ab`; text without quotes is returned as is.
fn unquote_txt(data: &str) -> String {
    let t = data.trim();
    if !t.starts_with('"') {
        return t.to_string();
    }
    let mut out = String::new();
    let mut chars = t.chars();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        match (in_str, c) {
            (false, '"') => in_str = true,
            (true, '"') => in_str = false,
            (true, '\\') => match chars.next() {
                Some(d) if d.is_ascii_digit() => {
                    // \DDD decimal escape
                    let mut n = d.to_digit(10).unwrap_or(0);
                    for _ in 0..2 {
                        if let Some(e) = chars.next() {
                            n = n * 10 + e.to_digit(10).unwrap_or(0);
                        }
                    }
                    out.push(char::from_u32(n).unwrap_or('?'));
                }
                Some(d) => out.push(d),
                None => {}
            },
            (true, c) => out.push(c),
            (false, _) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mx_with_cname_chain_and_dots() {
        let body = br#"{"Status":0,"Answer":[
            {"name":"www.x.test.","type":5,"TTL":60,"data":"x.test."},
            {"name":"x.test.","type":15,"TTL":300,"data":"10 mx1.x.test."},
            {"name":"x.test.","type":15,"TTL":300,"data":"20 mx2.x.test."}]}"#;
        let a = parse_response(body, RecordType::Mx).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].name, "x.test");
        assert_eq!(a[0].ttl, 300);
        assert_eq!(
            a[1].record,
            Record::Mx {
                preference: 20,
                exchange: "mx2.x.test".into()
            }
        );
    }

    #[test]
    fn parses_a_aaaa_srv_txt() {
        let a = parse_response(
            br#"{"Status":0,"Answer":[{"name":"h.","type":1,"TTL":1,"data":"1.2.3.4"}]}"#,
            RecordType::A,
        )
        .unwrap();
        assert_eq!(a[0].record, Record::A(Ipv4Addr::new(1, 2, 3, 4)));
        let a = parse_response(
            br#"{"Status":0,"Answer":[{"name":"h.","type":28,"TTL":1,"data":"2606:4700::1"}]}"#,
            RecordType::Aaaa,
        )
        .unwrap();
        assert_eq!(a[0].record, Record::Aaaa("2606:4700::1".parse().unwrap()));
        let a = parse_response(
            br#"{"Status":0,"Answer":[{"name":"_s._tcp.h.","type":33,"TTL":1,"data":"1 2 443 t.h."}]}"#,
            RecordType::Srv,
        )
        .unwrap();
        assert_eq!(
            a[0].record,
            Record::Srv {
                priority: 1,
                weight: 2,
                port: 443,
                target: "t.h".into()
            }
        );
        let a = parse_response(
            br#"{"Status":0,"Answer":[{"name":"h.","type":16,"TTL":1,"data":"\"v=spf1 \" \"a \\\"q\\\" -all\""}]}"#,
            RecordType::Txt,
        )
        .unwrap();
        assert_eq!(a[0].record, Record::Txt("v=spf1 a \"q\" -all".into()));
    }

    #[test]
    fn nxdomain_is_empty_servfail_is_error() {
        assert!(parse_response(br#"{"Status":3}"#, RecordType::A)
            .unwrap()
            .is_empty());
        assert!(parse_response(br#"{"Status":2}"#, RecordType::A).is_err());
        assert!(parse_response(br#"{"Answer":[]}"#, RecordType::A).is_err());
    }

    #[test]
    fn url_encoding_and_types() {
        let r = Resolver::new("https://dns.google/resolve");
        assert_eq!(
            r.url("exa mple.com", RecordType::Txt),
            "https://dns.google/resolve?name=exa%20mple.com&type=TXT"
        );
        assert_eq!(RecordType::parse("mx"), Some(RecordType::Mx));
        assert_eq!(RecordType::parse("zz"), None);
        assert_eq!(RecordType::Srv.number(), 33);
    }
}
