use std::net::AddrParseError;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("parsing net address")]
    AddrParseError(#[from] AddrParseError),
    #[error("io error")]
    IOError(#[from] std::io::Error),
    #[error("dynamic")]
    Dynamic(#[from] DynamicError),
}

pub type Result<T> = core::result::Result<T, Error>;

pub type DynamicError = Box<dyn std::error::Error + Send + Sync + 'static>;
