use std::{path::Path, io::{self, BufReader}, fs::File, task::{Context, Poll}, pin::Pin};

use rustls_pemfile::{certs, read_one, Item};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::rustls::{Certificate, PrivateKey};

pub fn load_certs(path: &Path) -> io::Result<Vec<Certificate>> {
    certs(&mut BufReader::new(File::open(path)?))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid cert"))
        .map(|mut certs| certs.drain(..).map(Certificate).collect())
}

pub fn load_keys(path: &Path) -> io::Result<Vec<PrivateKey>> {
    let mut reader = BufReader::new(File::open(path)?);

    let mut keys = Vec::new();

    for item in std::iter::from_fn(|| read_one(&mut reader).transpose()) {
        match item.unwrap() {
            // Item::X509Certificate(cert) => keys.push(PrivateKey(cert)),
            Item::RSAKey(key) => keys.push(PrivateKey(key)),
            Item::PKCS8Key(key) => keys.push(PrivateKey(key)),
            Item::ECKey(key) => keys.push(PrivateKey(key)),
            _ => {}, // Ignore errors as certificate might be in the same file.
        }
    }

    Ok(keys)
}

/// A stream that might be protected with TLS.
/// Copied fro tokio_tungstenite because it only expected allowed the `tokio_rustls::client:TlsStream` type.
#[non_exhaustive]
#[derive(Debug)]
pub enum MaybeTlsStream<S> {
    Plain(S),
    RustlsServer(tokio_rustls::server::TlsStream<S>),
    RustlsClient(tokio_rustls::client::TlsStream<S>),
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for MaybeTlsStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(ref mut s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::RustlsServer(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::RustlsClient(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for MaybeTlsStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(ref mut s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::RustlsServer(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::RustlsClient(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(ref mut s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::RustlsServer(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::RustlsClient(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(ref mut s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::RustlsServer(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::RustlsClient(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
