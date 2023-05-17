use std::{net::SocketAddr, collections::HashMap};

use anyhow::{anyhow, Result};
use bytes::BytesMut;
use sockets::{client, message::{ClientMessage, StreamKind, ServerMessage, TunnelId, TunnelledStreamId, StreamSide}, protocol::Message};
use tokio::{sync::mpsc, net::{TcpSocket, TcpStream}, io::{WriteHalf, ReadHalf, AsyncReadExt, AsyncWriteExt}, select};
use url::Url;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the person to greet
    #[arg(long, default_value = "ws://localhost:3000")]
    server_url: Url,

    #[arg(short, long, help = "Local socket to expose, for example 127.0.0.1:22")]
    expose: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let (tx, mut rx) = client::dial(args.server_url).await?;

    let (stream_tx, mut stream_rx) = mpsc::channel::<(WritableStream, ReadableStream)>(16);
    let (read_done_tx, mut read_done_rx) = mpsc::channel::<TunnelledStreamId>(16);

    tx.send(
        Message::Message(
            ClientMessage::ExposeRequest{
                kind: StreamKind::Tcp,
            },
        )
    ).await?;

    let mut tunnel_id: Option<TunnelId> = None;

    let mut writables = HashMap::new();
    let mut cancels = HashMap::new();

    // FIXME a lot of code duplication between server and client.
    loop {
        select! {
            conn = stream_rx.recv() => {
                let writable;
                let mut readable;

                match conn {
                    None => {
                        eprintln!("got empty conn");
                        continue
                    }
                    Some((w, r)) => {
                        writable = w;
                        readable = r;
                    },
                }

                let cancel = tokio_util::sync::CancellationToken::new();
                let cancel2 = cancel.clone();

                writables.insert(writable.id, writable);
                cancels.insert(readable.id, cancel);

                let tx2 = tx.clone();
                let read_done_tx2 = read_done_tx.clone();

                tokio::spawn(async move {
                    match tcp_read_loop(&mut readable, cancel2, &tx2).await {
                        Ok(_) => {
                            eprintln!("conn closed gracefully")
                        }
                        Err(err) => {
                            eprintln!("error reading: {:?}", err)
                        }
                    }

                    match read_done_tx2.send(readable.id).await {
                        Ok(_) => {},
                        Err(err) => eprintln!("failed to send read done: {:?}", err),
                    }
                });
            },
            value = rx.recv() => {
                let msg = match value {
                    None => return Ok(()),
                    Some(Message::Ping) => continue,
                    Some(Message::Pong) => continue,
                    Some(Message::Message(v)) => v,
                };

                match msg {
                    ServerMessage::ExposeResponse(exp) => {
                        match exp {
                            Ok(res) => {
                                eprintln!("exposed with tunnel id {}, url: {}", res.tunnel_id, res.url);
                                tunnel_id = Some(res.tunnel_id);
                            }
                            Err(err) => {
                                return Err(anyhow!("failed to expose: {}", err)) // TODO
                            }
                        }
                    }
                    ServerMessage::UnexposeResponse { tunnel_id: _ } => unreachable!(),
                    ServerMessage::NewStreamRequest(id) => {
                        eprintln!("got new stream request: {:?}", id);

                        let tid = match tunnel_id {
                            None => {
                                eprintln!("got new stream request before tunnel was exposed: {:?}", id);

                                continue
                            }
                            Some(v) => v,
                        };

                        if let None = tunnel_id {
                            eprintln!("got new stream request before tunnel was exposed: {:?}", id);

                            continue
                        }

                        if id.tunnel_id != tid {
                            eprintln!("got new stream request with unknown tunnel id: {:?}", tunnel_id);

                            if let Err(err) = tx.send(Message::Message(
                                ClientMessage::NewStreamResponse{
                                    id,
                                    result: Err("unknown tunnel id".to_string()),
                                }
                            )).await {
                                eprintln!("error sending stream response: {:?}", err);
                            }

                            continue
                        }

                        let tx2 = tx.clone();
                        let stream_tx2 = stream_tx.clone();

                        tokio::spawn(async move {
                            let result;

                            match tcp_dial(id, args.expose, stream_tx2).await {
                                Ok(()) => {
                                    result = Ok(())
                                },
                                Err(err) => {
                                    eprintln!("failed to dial tcp: {:?}", err);
                                    result = Err("failed to dial tcp".to_string())
                                }
                            }

                            if let Err(err) = tx2.send(Message::Message(
                                ClientMessage::NewStreamResponse{
                                    id,
                                    result,
                                }
                            )).await {
                                eprintln!("error sending stream response: {:?}", err);
                            }
                        });
                    },
                    ServerMessage::Data { id, bytes } => {
                        let w = writables.get_mut(&id);

                        let w = match w {
                            None => {
                                eprintln!("bytes for missing writable: {:?}", id);
                                continue
                            },
                            Some(w) => w,
                        };

                        // FIXME blocking the main event loop.
                        match w.tx.write_all(&bytes).await {
                            Ok(_) => {},
                            Err(err) => {
                                eprintln!("error writing bytes: {:?}", err);

                                writables.remove(&id);

                                tx.send(Message::Message(ClientMessage::StreamClosed {
                                    id,
                                    side: StreamSide::Write,
                                })).await?;

                                continue
                            }
                        }
                    },
                    ServerMessage::StreamClosed { id, side } => {
                        eprintln!("server stream closed: {:?}, side: {:?}", id, side);

                        if side == StreamSide::Read {
                            writables.remove(&id);
                        }

                        if side == StreamSide::Write {
                            if let Some(s) = cancels.remove(&id) {
                                s.cancel();
                            }
                        }
                    }
                }
            }
            read_done = read_done_rx.recv() => {
                let id = match read_done {
                    None => {
                        eprintln!("got none from read_done");
                        continue
                    },
                    Some(v) => v,
                };

                if let Some(c) = cancels.remove(&id) {
                    c.cancel()
                }

                let msg = Message::Message(
                    ClientMessage::StreamClosed{
                        id,
                        side: StreamSide::Read,
                    },
                );

                match tx.send(msg).await {
                    Ok(_) => {},
                    Err(err) => eprintln!("failed to send read closed: {:?}", err),
                }
            }
        }
    }
}

#[derive(Debug)]
struct WritableStream {
    tx: WriteHalf<TcpStream>,
    id: TunnelledStreamId,
}

#[derive(Debug)]
struct ReadableStream {
    rx: ReadHalf<TcpStream>,
    id: TunnelledStreamId,
}

async fn tcp_read_loop(
    readable: &mut ReadableStream,
    cancel: tokio_util::sync::CancellationToken,
    tx: &mpsc::Sender<Message<ClientMessage>>,
) -> Result<()> {
    loop {
        let mut buf = BytesMut::with_capacity(4096);

        let n;

        select!{
            result = readable.rx.read_buf(&mut buf) => {
                n = match result {
                    Ok(0) => {
                        return Ok(());
                    },
                    Ok(n) => n,
                    Err(err) => {
                        return Err(err.into());
                    },
                };
            }
            _ = cancel.cancelled() => {
                return Err(anyhow!("canceled")) // TODO
            }
        }

        let bytes = Vec::from(&buf[..n]);

        let msg = Message::Message(
            ClientMessage::Data{
                id: readable.id,
                bytes,
            },
        );

        tx.send(msg).await?
    }
}

async fn tcp_dial(
    id: TunnelledStreamId,
    addr: SocketAddr,
    tx: mpsc::Sender<(WritableStream, ReadableStream)>,
) -> Result<()> {
    let stream = TcpSocket::new_v4()?.connect(addr).await?;

    let (tcp_rx, tcp_tx) = tokio::io::split(stream);

    let writable = WritableStream{
        tx: tcp_tx,
        id,
    };

    let readable = ReadableStream{
        rx: tcp_rx,
        id,
    };

    tx.send((writable, readable)).await?;

    Ok(())
}
