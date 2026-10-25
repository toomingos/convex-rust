//! `multipart/form-data` bodies, buffered (the host fetch task takes one body).

use std::{
    borrow::Cow,
    sync::atomic::{AtomicU64, Ordering},
};

use http::header::HeaderMap;

use crate::error::{Error, Result};

/// A multipart/form-data request body.
#[derive(Debug)]
pub struct Form {
    boundary: String,
    parts: Vec<(Cow<'static, str>, Part)>,
}

/// A field in a multipart form.
#[derive(Debug)]
pub struct Part {
    data: Vec<u8>,
    mime: Option<String>,
    file_name: Option<Cow<'static, str>>,
    headers: HeaderMap,
}

impl Default for Form {
    fn default() -> Self {
        Self::new()
    }
}

impl Form {
    pub fn new() -> Form {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Form {
            boundary: format!("{nanos:016x}{n:016x}"),
            parts: vec![],
        }
    }

    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    pub fn text<T, U>(self, name: T, value: U) -> Form
    where
        T: Into<Cow<'static, str>>,
        U: Into<Cow<'static, str>>,
    {
        self.part(name, Part::text(value))
    }

    pub fn part<T: Into<Cow<'static, str>>>(mut self, name: T, part: Part) -> Form {
        self.parts.push((name.into(), part));
        self
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, part) in &self.parts {
            out.extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
            let mut disp = format!("Content-Disposition: form-data; name=\"{}\"", escape(name));
            if let Some(f) = &part.file_name {
                disp.push_str(&format!("; filename=\"{}\"", escape(f)));
            }
            out.extend_from_slice(disp.as_bytes());
            out.extend_from_slice(b"\r\n");
            if let Some(m) = &part.mime {
                out.extend_from_slice(format!("Content-Type: {m}\r\n").as_bytes());
            }
            for (k, v) in &part.headers {
                out.extend_from_slice(k.as_str().as_bytes());
                out.extend_from_slice(b": ");
                out.extend_from_slice(v.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(&part.data);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        out
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

impl Part {
    pub fn text<T: Into<Cow<'static, str>>>(value: T) -> Part {
        Part::bytes(value.into().into_owned().into_bytes())
    }

    pub fn bytes<T: Into<Cow<'static, [u8]>>>(value: T) -> Part {
        Part {
            data: value.into().into_owned(),
            mime: None,
            file_name: None,
            headers: HeaderMap::new(),
        }
    }

    pub fn file_name<T: Into<Cow<'static, str>>>(mut self, filename: T) -> Part {
        self.file_name = Some(filename.into());
        self
    }

    pub fn mime_str(mut self, mime: &str) -> Result<Part> {
        let m: mime_check::Mime = mime_check::parse(mime)?;
        self.mime = Some(m);
        Ok(self)
    }

    pub fn headers(mut self, headers: HeaderMap) -> Part {
        self.headers = headers;
        self
    }
}

mod mime_check {
    use super::*;
    pub type Mime = String;
    pub fn parse(s: &str) -> Result<Mime> {
        if s.contains('/') && !s.contains(['\r', '\n']) {
            Ok(s.to_string())
        } else {
            Err(Error::builder_msg(format!("invalid mime type: {s}")))
        }
    }
}
