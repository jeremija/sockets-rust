use std::net::AddrParseError;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("parsing net address")]
    AddrParseError(#[from] AddrParseError),
}

pub type Result<T> = core::result::Result<T, Error>;
