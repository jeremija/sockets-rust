use bytes::BufMut;
use anyhow::Result;
use tokio::{select, io::{ReadHalf, WriteHalf, AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use tokio_util::sync::CancellationToken;

use crate::{message::TunnelledStreamId, error::Error};

#[derive(Debug)]
pub struct WritableStream {
    tx: WriteHalf<TcpStream>,
    id: TunnelledStreamId,
}

impl WritableStream {
    pub fn new(id: TunnelledStreamId, tx: WriteHalf<TcpStream>) -> Self {
        Self { id, tx }
    }

    pub fn id(&self) -> TunnelledStreamId {
        return self.id
    }

    pub async fn write_all<'a>(&'a mut self, src: &'a [u8]) -> Result<()>
    where
        Self: Unpin,
    {
        self.tx.write_all(src).await?;
        Ok(())
    }
}


#[derive(Debug)]
pub struct ReadableStream {
    id: TunnelledStreamId,
    rx: ReadHalf<TcpStream>,
    cancel: CancellationToken,
}

impl ReadableStream {
    pub fn new(id: TunnelledStreamId, rx: ReadHalf<TcpStream>) -> Self {
        let cancel = CancellationToken::new();
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
            _ = self.cancel.cancelled() => {
                Err(Error::Canceled.into())
            }
        }
    }
}
