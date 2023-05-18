use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("canceled")]
    Canceled,
    #[error("unauthorized")]
    Unauthorized,
    #[error("invalid local id")]
    InvalidLocalId,
    #[error("unknown tunnel id")]
    UnknownTunnelId,
}

pub type Result<T> = core::result::Result<T, Error>;
