//! Types for options that the Convex host owns (redirects, proxies, TLS).
//! They exist so vendor crates compile; they have no effect.

pub mod redirect {
    use http::StatusCode;
    use url::Url;

    /// A type that controls the policy on how to handle the following of redirects.
    pub struct Policy {
        _max: Option<usize>,
    }

    impl Policy {
        pub fn limited(max: usize) -> Self {
            Policy { _max: Some(max) }
        }
        pub fn none() -> Self {
            Policy { _max: Some(0) }
        }
        pub fn custom<T>(policy: T) -> Self
        where
            T: Fn(Attempt) -> Action + Send + Sync + 'static,
        {
            let _ = policy;
            Policy { _max: None }
        }
        /// `Policy::none()`: the host returns 3xx responses as is. Any other
        /// policy follows redirects (the host's limit is 20 hops).
        pub(crate) fn is_none(&self) -> bool {
            self._max == Some(0)
        }
        pub fn redirect(&self, attempt: Attempt) -> Action {
            match self._max {
                Some(m) if attempt.previous.len() >= m => attempt.stop(),
                _ => attempt.follow(),
            }
        }
    }

    impl Default for Policy {
        fn default() -> Policy {
            Policy::limited(10)
        }
    }

    /// A type that holds information on the next request and previous requests in a redirect chain.
    #[derive(Debug)]
    pub struct Attempt<'a> {
        status: StatusCode,
        next: &'a Url,
        previous: &'a [Url],
    }

    /// An action to perform when a redirect status code is found.
    #[derive(Debug)]
    pub struct Action {
        _follow: bool,
    }

    impl<'a> Attempt<'a> {
        pub fn status(&self) -> StatusCode {
            self.status
        }
        pub fn url(&self) -> &Url {
            self.next
        }
        pub fn previous(&self) -> &[Url] {
            self.previous
        }
        pub fn follow(self) -> Action {
            Action { _follow: true }
        }
        pub fn stop(self) -> Action {
            Action { _follow: false }
        }
        pub fn error<E: Into<Box<dyn std::error::Error + Send + Sync>>>(self, error: E) -> Action {
            let _ = error.into();
            Action { _follow: false }
        }
    }
}

use crate::{error::Result, into_url::IntoUrl};

/// Configuration of a proxy (accepted, ignored).
#[derive(Debug, Clone)]
pub struct Proxy {
    _p: (),
}

impl Proxy {
    pub fn all<U: IntoUrl>(proxy_scheme: U) -> Result<Proxy> {
        proxy_scheme.into_url().map(|_| Proxy { _p: () })
    }
    pub fn http<U: IntoUrl>(proxy_scheme: U) -> Result<Proxy> {
        Self::all(proxy_scheme)
    }
    pub fn https<U: IntoUrl>(proxy_scheme: U) -> Result<Proxy> {
        Self::all(proxy_scheme)
    }
    pub fn basic_auth(self, username: &str, password: &str) -> Proxy {
        let _ = (username, password);
        self
    }
}

pub mod tls {
    use crate::error::Result;

    /// A TLS protocol version.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Version(u8);

    impl Version {
        pub const TLS_1_0: Version = Version(0);
        pub const TLS_1_1: Version = Version(1);
        pub const TLS_1_2: Version = Version(2);
        pub const TLS_1_3: Version = Version(3);
    }

    /// A root certificate (accepted, ignored).
    #[derive(Debug, Clone)]
    pub struct Certificate {
        _p: (),
    }

    impl Certificate {
        pub fn from_der(der: &[u8]) -> Result<Certificate> {
            let _ = der;
            Ok(Certificate { _p: () })
        }
        pub fn from_pem(pem: &[u8]) -> Result<Certificate> {
            let _ = pem;
            Ok(Certificate { _p: () })
        }
    }

    /// A client identity (accepted, ignored).
    #[derive(Debug, Clone)]
    pub struct Identity {
        _p: (),
    }

    impl Identity {
        pub fn from_pem(buf: &[u8]) -> Result<Identity> {
            let _ = buf;
            Ok(Identity { _p: () })
        }
    }
}
