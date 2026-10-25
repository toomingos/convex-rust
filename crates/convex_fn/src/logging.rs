//! Optional bridges from the `log` and `tracing` ecosystems to the host `log`
//! import (contracts/ABI.md levels: 0 debug, 1 info, 2 warn, 3 error).
//!
//! Both are installed automatically by the `#[action]` entry (and by
//! `convex_fn_test`) when the corresponding cargo feature is on and the user has
//! not installed a logger / default subscriber of their own.
//!
//! Plain `println!` / `eprintln!` need no feature: the WASI stdout / stderr
//! file descriptors are turned into log lines by the host (stdout at info,
//! stderr at error).

#[cfg(feature = "log")]
mod log_bridge {
    use log_crate::{Level, LevelFilter, Log, Metadata, Record};

    use crate::{sys, wire::LogLevel};

    pub struct HostLogger;

    pub fn level(l: Level) -> LogLevel {
        match l {
            Level::Trace | Level::Debug => LogLevel::Debug,
            Level::Info => LogLevel::Info,
            Level::Warn => LogLevel::Warn,
            Level::Error => LogLevel::Error,
        }
    }

    impl Log for HostLogger {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &Record<'_>) {
            sys::log_line(level(record.level()), &record.args().to_string());
        }
        fn flush(&self) {}
    }

    static LOGGER: HostLogger = HostLogger;

    pub fn install() {
        if log_crate::set_logger(&LOGGER).is_ok() {
            log_crate::set_max_level(LevelFilter::Trace);
        }
    }
}

#[cfg(feature = "tracing")]
mod tracing_bridge {
    use std::{
        collections::HashMap,
        fmt::Write as _,
        sync::{
            atomic::{AtomicU64, Ordering},
            Mutex,
        },
    };

    use tracing_core::{
        field::{Field, Visit},
        span, Event, Level, Metadata, Subscriber,
    };

    use crate::{sys, wire::LogLevel};

    #[derive(Default)]
    struct Fields {
        message: String,
        rest: String,
    }

    impl Fields {
        fn kv(&mut self, name: &str, value: std::fmt::Arguments<'_>) {
            if !self.rest.is_empty() {
                self.rest.push(' ');
            }
            let _ = write!(self.rest, "{name}={value}");
        }
    }

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                let _ = write!(self.message, "{value:?}");
            } else {
                self.kv(field.name(), format_args!("{value:?}"));
            }
        }
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.message.push_str(value);
            } else {
                self.kv(field.name(), format_args!("{value:?}"));
            }
        }
    }

    struct SpanData {
        name: &'static str,
        fields: String,
        refs: usize,
    }

    /// Formats events as `span{fields}: message key=value ...` and writes them
    /// to the host log.
    #[derive(Default)]
    pub struct HostSubscriber {
        next_id: AtomicU64,
        spans: Mutex<HashMap<u64, SpanData>>,
        stack: Mutex<Vec<u64>>,
    }

    pub fn level(l: &Level) -> LogLevel {
        match *l {
            Level::TRACE | Level::DEBUG => LogLevel::Debug,
            Level::INFO => LogLevel::Info,
            Level::WARN => LogLevel::Warn,
            Level::ERROR => LogLevel::Error,
        }
    }

    impl Subscriber for HostSubscriber {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, attrs: &span::Attributes<'_>) -> span::Id {
            let mut f = Fields::default();
            attrs.record(&mut f);
            let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            self.spans.lock().unwrap().insert(
                id,
                SpanData {
                    name: attrs.metadata().name(),
                    fields: f.rest,
                    refs: 1,
                },
            );
            span::Id::from_u64(id)
        }
        fn record(&self, id: &span::Id, values: &span::Record<'_>) {
            let mut f = Fields::default();
            values.record(&mut f);
            if let Some(s) = self.spans.lock().unwrap().get_mut(&id.into_u64()) {
                if !s.fields.is_empty() && !f.rest.is_empty() {
                    s.fields.push(' ');
                }
                s.fields.push_str(&f.rest);
            }
        }
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn clone_span(&self, id: &span::Id) -> span::Id {
            if let Some(s) = self.spans.lock().unwrap().get_mut(&id.into_u64()) {
                s.refs += 1;
            }
            id.clone()
        }
        fn try_close(&self, id: span::Id) -> bool {
            let mut spans = self.spans.lock().unwrap();
            let k = id.into_u64();
            if let Some(s) = spans.get_mut(&k) {
                s.refs -= 1;
                if s.refs == 0 {
                    spans.remove(&k);
                    return true;
                }
            }
            false
        }
        fn enter(&self, id: &span::Id) {
            self.stack.lock().unwrap().push(id.into_u64());
        }
        fn exit(&self, id: &span::Id) {
            let mut st = self.stack.lock().unwrap();
            if let Some(i) = st.iter().rposition(|x| *x == id.into_u64()) {
                st.remove(i);
            }
        }
        fn event(&self, event: &Event<'_>) {
            let mut f = Fields::default();
            event.record(&mut f);
            let mut line = String::new();
            {
                let spans = self.spans.lock().unwrap();
                for id in self.stack.lock().unwrap().iter() {
                    if let Some(s) = spans.get(id) {
                        line.push_str(s.name);
                        if !s.fields.is_empty() {
                            let _ = write!(line, "{{{}}}", s.fields);
                        }
                        line.push(':');
                    }
                }
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&f.message);
            if !f.rest.is_empty() {
                if !f.message.is_empty() {
                    line.push(' ');
                }
                line.push_str(&f.rest);
            }
            sys::log_line(level(event.metadata().level()), &line);
        }
    }

    pub fn install() {
        // Fails (harmlessly) when a global default subscriber already exists.
        let _ = tracing_core::dispatcher::set_global_default(tracing_core::Dispatch::new(
            HostSubscriber::default(),
        ));
    }
}

/// Installs the enabled bridges unless the user already installed their own.
pub(crate) fn install() {
    #[cfg(feature = "log")]
    log_bridge::install();
    #[cfg(feature = "tracing")]
    tracing_bridge::install();
}
