use std::net::SocketAddr;

use bytes::BufMut;
use anyhow::Result;
use tokio::{select, io::{AsyncReadExt, AsyncWriteExt}, net::{tcp::{OwnedWriteHalf, OwnedReadHalf}, TcpListener, TcpSocket, TcpStream}};
use tokio_util::sync::CancellationToken;

use crate::{message::{TunnelledStreamId, StreamId, TunnelId}, error::Error};

/// WritableStream wraps `OwnedWriteHalf`. It provides a `write_all` method, and does two extra
/// things: it makes the `id` method available, and it allows a blocking write to be cancelled.
#[derive(Debug)]
pub struct WritableStream {
    tx: OwnedWriteHalf,
    id: TunnelledStreamId,
    cancel: CancellationToken,
}

impl WritableStream {
    pub fn new(id: TunnelledStreamId, tx: OwnedWriteHalf, cancel: CancellationToken) -> Self {
        Self { id, tx, cancel }
    }

    pub fn id(&self) -> TunnelledStreamId {
        return self.id
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        return self.cancel.clone();
    }

    pub async fn write_all<'a>(&'a mut self, src: &'a [u8]) -> Result<()>
    where
        Self: Unpin,
    {
        select! {
            res = self.tx.write_all(src) => res?,
            _ = self.cancel.cancelled() => return Err(Error::Canceled.into()),
        }
        Ok(())
    }
}


/// WritableStream wraps `OwnedReadHalf`. It provides a `read_buf` method, and does two extra
/// things: it makes the `id` method available, and it allows a blocking read to be cancelled.
#[derive(Debug)]
pub struct ReadableStream {
    id: TunnelledStreamId,
    rx: OwnedReadHalf,
    cancel: CancellationToken,
}

impl ReadableStream {
    pub fn new(id: TunnelledStreamId, rx: OwnedReadHalf, cancel: CancellationToken) -> Self {
        Self { id, rx, cancel }
    }

    pub fn id(&self) -> TunnelledStreamId {
        return self.id
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        return self.cancel.clone();
    }

    pub async fn read_buf<'a, B>(&'a mut self, buf: &'a mut B) -> Result<usize>
    where
        Self: Sized + Unpin,
        B: BufMut,
    {
        select!{
            result = self.rx.read_buf(buf) => {
                let n = result?;
                Ok(n)
            },
            _ = self.cancel.cancelled() => Err(Error::Canceled.into()),
        }
    }
}

/// Listener wraps a `TcpListener` and it allows a blocking listen to be cancelled using the
/// `cancellation_token`.
pub struct Listener {
    tunnel_id: TunnelId,
    listener: TcpListener,
    cancel: CancellationToken,
}

impl Listener {
    pub fn new(
        tunnel_id: TunnelId,
        listener: TcpListener,
        cancel: CancellationToken,
    ) -> Self {
        Self { tunnel_id, listener, cancel }
    }

    pub async fn accept(&self) -> Result<(WritableStream, ReadableStream, SocketAddr)> {
        let b = select! {
            res = self.listener.accept() => res.map_err(Error::IOError),
            _ = self.cancel.cancelled() => Err(Error::Canceled.into()),
        };

        let tunnel_id = self.tunnel_id;
        let stream_id = StreamId::rand();

        let id = TunnelledStreamId{
            stream_id,
            tunnel_id,
        };

        let (tcp_stream, addr) = b?;

        let (writable, readable) = split_tcp_stream(id, tcp_stream, self.cancel.child_token());

        return Ok((writable, readable, addr));
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        return self.cancel.clone();
    }
}

/// Dial dials a TCP ssocket
pub async fn dial(
    id: TunnelledStreamId,
    addr: SocketAddr,
    cancel: CancellationToken,
) -> Result<(WritableStream, ReadableStream)> {
    let socket: TcpSocket;

    if addr.is_ipv4() {
        socket = TcpSocket::new_v4()?
    } else if addr.is_ipv6() {
        socket = TcpSocket::new_v6()?
    } else {
        return Err(Error::InvalidSocketAddress.into());
    }

    let tcp_stream = select!{
        res = socket.connect(addr) => res.map_err(Error::IOError),
        _ = cancel.cancelled() => Err(Error::Canceled.into()),
    };

    let tcp_stream = tcp_stream?;

    let (writable, readable) = split_tcp_stream(id, tcp_stream, cancel.child_token());

    Ok((writable, readable))
}

fn split_tcp_stream(
    id: TunnelledStreamId,
    tcp_stream: TcpStream,
    cancel: CancellationToken,
) -> (WritableStream, ReadableStream) {
    let (tcp_rx, tcp_tx) = tcp_stream.into_split();
    // let (tcp_rx, tcp_tx) = tokio::io::split(tcp_stream);

    let writable = WritableStream::new(id, tcp_tx, cancel.clone());

    let readable = ReadableStream::new(id, tcp_rx, cancel.clone());

    return (writable, readable);
}
