use url::Url;

use crate::error::{Error, Result};

/// A trait to try to convert some type into a `Url`.
pub trait IntoUrl: IntoUrlSealed {}

impl IntoUrl for Url {}
impl IntoUrl for String {}
impl IntoUrl for &str {}
impl IntoUrl for &String {}

#[doc(hidden)]
pub trait IntoUrlSealed {
    fn into_url(self) -> Result<Url>;
    fn as_str(&self) -> &str;
}

impl IntoUrlSealed for Url {
    fn into_url(self) -> Result<Url> {
        if self.has_host() {
            Ok(self)
        } else {
            Err(Error::builder_msg("URL scheme is not allowed").with_url(self))
        }
    }
    fn as_str(&self) -> &str {
        self.as_ref()
    }
}

impl IntoUrlSealed for &str {
    fn into_url(self) -> Result<Url> {
        Url::parse(self).map_err(Error::builder)?.into_url()
    }
    fn as_str(&self) -> &str {
        self
    }
}

impl IntoUrlSealed for &String {
    fn into_url(self) -> Result<Url> {
        (&**self).into_url()
    }
    fn as_str(&self) -> &str {
        self.as_ref()
    }
}

impl IntoUrlSealed for String {
    fn into_url(self) -> Result<Url> {
        (&*self).into_url()
    }
    fn as_str(&self) -> &str {
        self.as_ref()
    }
}
