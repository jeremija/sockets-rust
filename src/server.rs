use std::{sync::Arc, net::{SocketAddr, IpAddr}, collections::HashMap};

use anyhow::Result;
use bytes::BytesMut;
use futures::stream::SplitSink;
use tokio::{net::{TcpStream, TcpListener}, select};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message as WSMessage, MaybeTlsStream};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use log::{error, info};
use tokio_util::sync::CancellationToken;

use crate::{message::{ClientMessage, ServerMessage, TunnelledStreamId, StreamKind, TunnelId, ExposeResponse, StreamSide}, protocol::{serialize, deserialize, Message}, auth::Authenticator, tcp::{WritableStream, ReadableStream, Listener}, error::Error};

type Sender = mpsc::Sender<Message<ServerMessage>>;
type Receiver = mpsc::Receiver<Message<ClientMessage>>;

pub async fn from_stream(
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    addr: std::net::SocketAddr,
) -> Result<(Sender, Receiver)> {
    let (mut tx, mut rx) = ws_stream.split();

    let (mut client_msg_tx, client_msg_rx) = mpsc::channel::<Message<ClientMessage>>(16);
    let (server_msg_tx, mut server_msg_rx) = mpsc::channel::<Message<ServerMessage>>(16);

    info!("new websocket connection from {}", addr);

    tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            let msg = msg.map_err(anyhow::Error::msg);

            match recv_client_message(&mut client_msg_tx, msg).await {
                Ok(Some(_)) => continue,
                Ok(None) => return,
                Err(err) => {
                    error!("receiving server message: {:?}", err);
                    return
                }
            }
        }
    });

    tokio::spawn(async move {
        while let Some(msg) = server_msg_rx.recv().await {
            match send_server_message(&mut tx, msg).await {
                Ok(()) => continue,
                Err(err) => {
                    error!("sending client message: {:?}", err);
                    return
                }
            }
        }
    });


    Ok((server_msg_tx, client_msg_rx))
}

async fn recv_client_message(
    tx: &mut mpsc::Sender<Message<ClientMessage>>,
    msg: Result<WSMessage>,
) -> Result<Option<()>> {
    let msg: Option<Message<ClientMessage>> = deserialize(msg)?;

    match msg {
        Some(val) => {
            tx.send(val).await?;
            Ok(Some(()))
        }
        None => Ok(None)
    }
}

async fn send_server_message(
    tx: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WSMessage>,
    msg: Message<ServerMessage>,
    ) -> Result<()> {
    let msg = serialize(msg)?;
    tx.send(msg).await?;
    Ok(())
}

pub async fn handle_stream(
    cancel: CancellationToken,
    authenticator: Arc<dyn Authenticator>,
    stream: MaybeTlsStream<TcpStream>,
    addr: SocketAddr,
    adhoc_bind_addr: IpAddr,
    hostname: String,
) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;

    let (tx, mut rx) = from_stream(ws_stream, addr).await?;

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

                        let local_listener_addr = SocketAddr::new(adhoc_bind_addr, 0);

                        let listener = match TcpListener::bind(local_listener_addr).await {
                            Ok(listener) => listener,
                            Err(err) => {
                                error!("creating server listener: {:?}", err);
                                tx.send(Message::Message(ServerMessage::ExposeResponse(
                                    Err("failed to create server listener".to_string()),
                                ))).await?;

                                continue
                            },
                        };

                        let local_listener_addr = listener.local_addr()?;

                        let port = local_listener_addr.port();

                        let tunnel_id = TunnelId::rand();

                        let stream_tx2 = stream_tx.clone();

                        let listener = Listener::new(tunnel_id, listener, cancel.clone());

                        tokio::spawn(async move {
                            match accept_tcp(listener, stream_tx2).await {
                                Ok(()) => info!("accept_tcp done: {}", tunnel_id),
                                Err(err) => error!("accept_tcp: {}: {:?}", tunnel_id, err),
                            }
                        });

                        let url = format!("{}:{}", hostname, port);

                        tx.send(Message::Message(ServerMessage::ExposeResponse(
                            Ok(
                                ExposeResponse{
                                    local_id,
                                    tunnel_id,
                                    url: url.clone(),
                                },
                            )
                        ))).await?;

                        info!("created tunnel {} to {} available on {}", tunnel_id, addr, url);
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
    listener: Listener,
    tx: mpsc::Sender<(WritableStream, ReadableStream)>,
) -> Result<()> {
    loop {
        let (writable, readable, _addr) = listener.accept().await?;

        tx.send((writable, readable)).await?;
    }
}
