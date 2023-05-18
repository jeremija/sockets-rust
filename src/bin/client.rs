use std::{net::SocketAddr, collections::HashMap, env};

use anyhow::{anyhow, Result};
use bytes::BytesMut;
use env_logger::Env;
use log::{error, info};
use sockets::{client, message::{ClientMessage, StreamKind, ServerMessage, TunnelledStreamId, StreamSide}, protocol::Message, tcp::{ReadableStream, WritableStream, dial}, error::Error};
use tokio::{sync::mpsc, select};
use tokio_util::sync::CancellationToken;
use url::Url;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the person to greet
    #[arg(long, default_value = "ws://localhost:3000")]
    server_url: Url,

    #[arg(short, long, help = "Local socket to expose, for example 127.0.0.1:22")]
    expose: Vec<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let api_key = env::var("SOCKETS_API_KEY").unwrap_or("".to_string());

    let args = Args::parse();

    let (tx, mut rx) = client::dial(args.server_url).await?;

    let (stream_tx, mut stream_rx) = mpsc::channel::<(WritableStream, ReadableStream)>(16);
    let (read_done_tx, mut read_done_rx) = mpsc::channel::<TunnelledStreamId>(16);

    if api_key != "" {
        tx.send(
            Message::Message(
                ClientMessage::AuthenticateRequest{ api_key },
            )
        ).await?;
    }

    for (i, _) in args.expose.iter().enumerate() {
        tx.send(
            Message::Message(
                ClientMessage::ExposeRequest{
                    kind: StreamKind::Tcp,
                    local_id: i as u32,
                },
            )
        ).await?;
    }

    let mut tunnels =  HashMap::with_capacity(args.expose.len());

    let cancel = CancellationToken::new();

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
                        error!("got empty conn");
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
                            info!("conn closed gracefully")
                        }
                        Err(err) => {
                            error!("error reading: {:?}", err)
                        }
                    }

                    match read_done_tx2.send(id).await {
                        Ok(_) => {},
                        Err(err) => error!("failed to send read done: {:?}", err),
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
                    ServerMessage::Unauthorized => {
                        return Err(Error::Unauthorized)?;
                    },
                    ServerMessage::ExposeResponse(exp) => {
                        match exp {
                            Ok(res) => {
                                let &socket_addr = args.expose.get(res.local_id as usize).ok_or_else(|| Error::InvalidLocalId)?;
                                tunnels.insert(res.tunnel_id, socket_addr);

                                info!("created tunnel {} to {} via {}", res.tunnel_id, socket_addr, res.url);
                            }
                            Err(err) => {
                                return Err(anyhow!("failed to expose: {}", err)) // TODO
                            }
                        }
                    }
                    ServerMessage::UnexposeResponse { tunnel_id: _ } => unreachable!(),
                    ServerMessage::NewStreamRequest(id) => {
                        info!("got new stream request: {:?}", id);

                        let socket_addr = match tunnels.get(&id.tunnel_id) {
                            Some(&addr) => addr,
                            None => {
                                error!("got request for unknown tunnel id: {}", id.tunnel_id);

                                if let Err(err) = tx.send(Message::Message(
                                    ClientMessage::NewStreamResponse{
                                        id,
                                        result: Err("unknown tunnel id".to_string()),
                                    }
                                )).await {
                                    error!("error sending stream response: {:?}", err);
                                }

                                continue
                            }
                        };

                        let tx2 = tx.clone();
                        let stream_tx2 = stream_tx.clone();
                        let cancel2 = cancel.clone();

                        tokio::spawn(async move {
                            let result;

                            match tcp_dial(id, socket_addr, stream_tx2, cancel2).await {
                                Ok(()) => {
                                    result = Ok(())
                                },
                                Err(err) => {
                                    error!("failed to dial tcp: {:?}", err);
                                    result = Err("failed to dial tcp".to_string())
                                }
                            }

                            if let Err(err) = tx2.send(Message::Message(
                                ClientMessage::NewStreamResponse{
                                    id,
                                    result,
                                }
                            )).await {
                                error!("error sending stream response: {:?}", err);
                            }
                        });
                    },
                    ServerMessage::Data { id, bytes } => {
                        let w = writables.get_mut(&id);

                        let w = match w {
                            None => {
                                error!("bytes for missing writable: {:?}", id);
                                continue
                            },
                            Some(w) => w,
                        };

                        // FIXME blocking the main event loop.
                        match w.write_all(&bytes).await {
                            Ok(_) => {},
                            Err(err) => {
                                error!("writing bytes: {:?}", err);

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
                        error!("server stream closed: {:?}, side: {:?}", id, side);

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
                        error!("got none from read_done");
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
                    Err(err) => error!("sending read closed: {:?}", err),
                }
            }
        }
    }
}

async fn tcp_read_loop(
    readable: &mut ReadableStream,
    tx: &mpsc::Sender<Message<ClientMessage>>,
) -> Result<()> {
    loop {
        let mut buf = BytesMut::with_capacity(4096);

        let n = readable.read_buf(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        let bytes = Vec::from(&buf[..n]);

        let msg = Message::Message(
            ClientMessage::Data{
                id: readable.id(),
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
    cancel: CancellationToken,
) -> Result<()> {
    let (writable, readable) = dial(id, addr, cancel).await?;

    tx.send((writable, readable)).await?;

    Ok(())
}
