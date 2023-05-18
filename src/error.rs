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

    #[error("IOError")]
    IOError(#[from] std::io::Error),

    #[error("no TLS certificates loaded")]
    NoTlsCertificates,

    #[error("no TLS certificate keys loaded")]
    NoTlsCertificateKeys,

    #[error("invalid socket address")]
    InvalidSocketAddress,
}

pub type Result<T> = core::result::Result<T, Error>;
