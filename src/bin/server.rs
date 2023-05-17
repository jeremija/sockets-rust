use std::{net::SocketAddr, collections::HashMap};

use bytes::BytesMut;
use clap::{command, Parser};
use anyhow::{anyhow, Result};
use sockets::{server, protocol::{Message, self}, message::{ClientMessage, ServerMessage, ExposeResponse, StreamKind, TunnelId, StreamId, StreamSide, TunnelledStreamId}};
use tokio::{net::{TcpListener, TcpStream}, select, sync::mpsc, io::{WriteHalf, ReadHalf, AsyncWriteExt}};
use tokio_tungstenite::MaybeTlsStream;
use tokio::io::AsyncReadExt;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the person to greet
    #[arg(short, long, default_value = "localhost")]
    hostname: String,

    #[arg(
        short, long,
        help = "Bind args for webserver socket",
        default_value = "127.0.0.1:3000"
    ) ]
    listen_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let listener = TcpListener::bind(args.listen_addr).await?;

    loop {
        let (stream, _addr) = listener.accept().await?;

        tokio::spawn(async move {
            let stream = MaybeTlsStream::Plain(stream);

            match handle_stream(stream).await {
                Ok(()) => {},
                Err(err) => {
                    eprintln!("error: {:?}", err);
                }
            }

        });
    }
}

async fn handle_stream(stream: MaybeTlsStream<TcpStream>) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;

    let (tx, mut rx) = server::from_stream(ws_stream).await?;

    let (stream_tx, mut stream_rx) = mpsc::channel::<(WritableStream, ReadableStream)>(16);
    let (read_done_tx, mut read_done_rx) = mpsc::channel::<TunnelledStreamId>(16);

    let mut writables = HashMap::new();
    let mut cancels = HashMap::new();

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
                let c = match value {
                    None => return Ok(()),
                    Some(Message::Ping) => continue,
                    Some(Message::Pong) => continue,
                    Some(Message::Message(v)) => v,
                };

                match c {
                    ClientMessage::ExposeRequest { kind } => {
                        if kind != StreamKind::Tcp {
                            tx.send(protocol::Message::Message(ServerMessage::ExposeResponse(
                                Err("only tcp is supported".to_string()),
                            ))).await?;
                            continue
                        }

                        match TcpListener::bind("0.0.0.0:0").await {
                            Ok(listener) => {
                                let port = listener.local_addr()?.port();

                                let tunnel_id = TunnelId::rand();

                                let stream_tx2 = stream_tx.clone();

                                tokio::spawn(async move {
                                    match accept_tcp(tunnel_id, listener, stream_tx2).await {
                                        Ok(()) => {},
                                        Err(err) => eprintln!("error accepting tcp: {:?}", err),
                                    }
                                });

                                tx.send(protocol::Message::Message(ServerMessage::ExposeResponse(
                                    Ok(
                                        ExposeResponse{
                                            tunnel_id,
                                            url: format!("{}:{}", "localhost:", port),
                                        },
                                    )
                                ))).await?;
                            },
                            Err(err) => {
                                eprintln!("failed to create server listener: {:?}", err);
                                tx.send(protocol::Message::Message(ServerMessage::ExposeResponse(
                                    Err("failed to create server listener".to_string()),
                                ))).await?;
                            },
                        }
                    },
                    ClientMessage::UnexposeRequest { tunnel_id } => {
                        writables.retain(|k, _| k.tunnel_id != tunnel_id);
                        cancels.retain(|k, c| {
                            let keep = k.tunnel_id != tunnel_id;

                            if !keep {
                                c.cancel()
                            }

                            keep
                        });
                    },
                    ClientMessage::NewStreamResponse(result) => {
                        eprintln!("new stream {:?}", result);
                    },
                    ClientMessage::Data { id, bytes } => {
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

                                tx.send(protocol::Message::Message(ServerMessage::StreamClosed {
                                    id,
                                    side: StreamSide::Write,
                                })).await?;

                                continue
                            }
                        }
                    },
                    ClientMessage::StreamClosed {id, side } => {
                        // When the client has closed the read side, it means there will be no more
                        // writes, so we can remove our writable.
                        if side == StreamSide::Read {
                            writables.remove(&id);
                        }

                        if side == StreamSide::Write {
                            if let Some(s) = cancels.remove(&id) {
                                s.cancel();
                            }
                        }
                    },
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

                let msg = protocol::Message::Message(
                    ServerMessage::StreamClosed{
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
    tx: &mpsc::Sender<Message<ServerMessage>>,
) -> Result<()> {
    let msg = protocol::Message::Message(
        ServerMessage::NewStreamRequest(
            TunnelledStreamId{
                tunnel_id: readable.id.tunnel_id,
                stream_id: readable.id.stream_id,
            }
        ),
    );

    tx.send(msg).await?;

    loop {
        let mut buf = BytesMut::with_capacity(4096);

        let n;

        select!{
            result = readable.rx.read_buf(&mut buf) => {
                n = match result {
                    Ok(0) => {
                        // let msg = protocol::Message::Message(
                        //     ServerMessage::StreamClosed{
                        //         id: readable.id,
                        //         side: StreamSide::Read,
                        //     },
                        // );

                        // _ = tx.send(msg).await;

                        // Connection closed.
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

        let msg = protocol::Message::Message(
            ServerMessage::Data{
                id: readable.id,
                bytes,
            },
        );

        tx.send(msg).await?
    }
}

async fn accept_tcp(
    tunnel_id: TunnelId,
    listener: TcpListener,
    tx: mpsc::Sender<(WritableStream, ReadableStream)>,
) -> Result<()> {
    loop {
        let (tcp_stream, _addr) = listener.accept().await?;

        let stream_id = StreamId::rand();

        let (tcp_rx, tcp_tx) = tokio::io::split(tcp_stream);

        let id = TunnelledStreamId { tunnel_id, stream_id };

        let writable = WritableStream{
            tx: tcp_tx,
            id,
        };

        let readable = ReadableStream{
            rx: tcp_rx,
            id,
        };

        tx.send((writable, readable)).await?;
    }
}
