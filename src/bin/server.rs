use std::{net::SocketAddr, collections::HashMap, env, sync::Arc};

use bytes::BytesMut;
use clap::{command, Parser};
use anyhow::Result;
use env_logger::Env;
use log::{info, error};
use sockets::{server, protocol::Message, message::{ServerMessage, TunnelId, StreamId, StreamSide, TunnelledStreamId, ClientMessage, StreamKind, ExposeResponse}, tcp::{ReadableStream, WritableStream}, auth::{Authenticator, SingleKeyAuthenticator}, error::Error};
use tokio::{net::{TcpListener, TcpStream}, select, sync::mpsc};
use tokio_tungstenite::MaybeTlsStream;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "localhost")]
    hostname: String,

    #[arg(
        long,
        help = "Bind args for webserver socket",
        default_value = "127.0.0.1:3000"
    ) ]
    listen_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    let listener = TcpListener::bind(args.listen_addr).await?;

    info!("listening on {}", listener.local_addr()?);

    let api_key = env::var("SOCKETS_SINGLE_API_KEY").unwrap_or("".to_string());

    let authenticator: Arc<dyn Authenticator + Send + Sync> = Arc::new(SingleKeyAuthenticator::new(api_key));

    loop {
        let (stream, addr) = listener.accept().await?;
        let hostname = args.hostname.clone();

        info!("new TCP connection from: {} on {}", addr, args.listen_addr);

        let authenticator = authenticator.clone();

        tokio::spawn(async move {
            // TODO add TLS support.
            let stream = MaybeTlsStream::Plain(stream);

            match handle_stream(authenticator, stream, addr, hostname).await {
                Ok(()) => {},
                Err(err) => {
                    error!("error handling stream: {}: {:?}", addr, err);
                }
            }

        });
    }
}

async fn handle_stream(
    authenticator: Arc<dyn Authenticator + Send + Sync>,
    stream: MaybeTlsStream<TcpStream>,
    addr: SocketAddr,
    hostname: String,
) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;

    let (tx, mut rx) = server::from_stream(ws_stream, addr).await?;

    let (stream_tx, mut stream_rx) = mpsc::channel::<(WritableStream, ReadableStream)>(16);
    let (read_done_tx, mut read_done_rx) = mpsc::channel::<TunnelledStreamId>(16);

    // TODO check if we keep some entries for too long here.
    // FIXME sometimes when the connection is done, the writable stays here
    // so we only remove an entry on a failed write. Need to figure out how to
    // do this properly.
    let mut writables = HashMap::new();
    let mut cancels = HashMap::new();

    let auth_needed = authenticator.auth_needed();
    let mut authorized = false;

    // FIXME a lot of code duplication between server and client.
    loop {
        select! {
            conn = stream_rx.recv() => {
                let writable;
                let mut readable;

                match conn {
                    None => {
                        error!("stream_rx.recv() returned None: {:?}", addr);
                        continue
                    }
                    Some((w, r)) => {
                        writable = w;
                        readable = r;
                    },
                }

                let id = writable.id();
                let cancel = readable.cancellation_token();

                writables.insert(id, writable);
                cancels.insert(id, cancel);

                let tx2 = tx.clone();
                let read_done_tx2 = read_done_tx.clone();

                tokio::spawn(async move {
                    match tcp_read_loop(&mut readable, &tx2).await {
                        Ok(_) => {
                            info!("read stream closed gracefully {:?}", id)
                        }
                        Err(err) => {
                            error!("read stream closed: {:?} {:?}", id, err)
                        }
                    }

                    match read_done_tx2.send(id).await {
                        Ok(_) => {},
                        Err(err) => error!("sending read done: {:?} {:?}", id, err),
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

                if auth_needed && !authorized {
                    match c {
                        ClientMessage::AuthenticateRequest{ .. } => {},
                        _ => {
                            tx.send(Message::Message(ServerMessage::Unauthorized)).await?;
                            return Err(Error::Unauthorized.into());
                        },
                    }
                }

                match c {
                    ClientMessage::AuthenticateRequest{ api_key } => {
                        match authenticator.authenticate(api_key) {
                            Ok(_) => {
                                info!("authenticated {}", addr);
                            },
                            Err(err) => {
                                tx.send(Message::Message(ServerMessage::Unauthorized)).await?;

                                return Err(err)
                            }
                        }

                        authorized = true;
                    }
                    ClientMessage::ExposeRequest { local_id, kind } => {
                        if kind != StreamKind::Tcp {
                            tx.send(Message::Message(ServerMessage::ExposeResponse(
                                Err("only tcp is supported".to_string()),
                            ))).await?;
                            continue
                        }

                        let listener = match TcpListener::bind("0.0.0.0:0").await {
                            Ok(listener) => listener,
                            Err(err) => {
                                error!("creating server listener: {:?}", err);
                                tx.send(Message::Message(ServerMessage::ExposeResponse(
                                    Err("failed to create server listener".to_string()),
                                ))).await?;

                                continue
                            },
                        };

                        let port = listener.local_addr()?.port();

                        let tunnel_id = TunnelId::rand();

                        let stream_tx2 = stream_tx.clone();

                        tokio::spawn(async move {
                            match accept_tcp(tunnel_id, listener, stream_tx2).await {
                                Ok(()) => {},
                                Err(err) => error!("accepting tcp: {:?}", err),
                            }
                        });

                        tx.send(Message::Message(ServerMessage::ExposeResponse(
                            Ok(
                                ExposeResponse{
                                    local_id,
                                    tunnel_id,
                                    url: format!("{}:{}", hostname, port),
                                },
                            )
                        ))).await?
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
                    ClientMessage::NewStreamResponse{ id, result } => {
                        info!("new stream {:?}: response {:?}", id, result);

                        if let Err(_) = result {
                            writables.remove(&id);

                            if let Some(s) = cancels.remove(&id) {
                                s.cancel();
                            }
                        }
                    },
                    ClientMessage::Data { id, bytes } => {
                        let w = writables.get_mut(&id);

                        let w = match w {
                            None => {
                                info!("bytes for missing writable: {:?}", id);
                                continue
                            },
                            Some(w) => w,
                        };

                        // FIXME blocking the main event loop.
                        match w.write_all(&bytes).await {
                            Ok(_) => {},
                            Err(err) => {
                                info!("write failed: {:?} {:?}", id, err);

                                writables.remove(&id);

                                tx.send(Message::Message(ServerMessage::StreamClosed {
                                    id,
                                    side: StreamSide::Write,
                                })).await?;

                                continue
                            }
                        }
                    },
                    ClientMessage::StreamClosed {id, side } => {
                        info!("client server stream closed: {:?}, side: {:?}", id, side);
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
                        info!("got none from read_done");
                        continue
                    },
                    Some(v) => v,
                };

                if let Some(c) = cancels.remove(&id) {
                    c.cancel()
                }

                let msg = Message::Message(
                    ServerMessage::StreamClosed{
                        id,
                        side: StreamSide::Read,
                    },
                );

                match tx.send(msg).await {
                    Ok(_) => {},
                    Err(err) => error!("failed to send read closed: {:?}", err),
                }
            }
        }
    }
}

async fn tcp_read_loop(
    readable: &mut ReadableStream,
    tx: &mpsc::Sender<Message<ServerMessage>>,
) -> Result<()> {
    let id = readable.id();

    let msg = Message::Message(
        ServerMessage::NewStreamRequest(
            TunnelledStreamId{
                tunnel_id: id.tunnel_id,
                stream_id: id.stream_id,
            }
        ),
    );

    tx.send(msg).await?;

    loop {
        let mut buf = BytesMut::with_capacity(4096);

        let n = readable.read_buf(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        let bytes = Vec::from(&buf[..n]);

        let msg = Message::Message(
            ServerMessage::Data{
                id,
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

        let writable = WritableStream::new(id, tcp_tx);

        let readable = ReadableStream::new(id, tcp_rx);

        tx.send((writable, readable)).await?;
    }
}
