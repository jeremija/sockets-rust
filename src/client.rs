use std::{net::SocketAddr, collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use bytes::BytesMut;
use futures::stream::SplitSink;
use tokio::{net::TcpStream, select};
use tokio_tungstenite::{WebSocketStream, MaybeTlsStream, tungstenite::protocol::Message as WSMessage, connect_async_tls_with_config, Connector};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use log::{error, info};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{message::{ClientMessage, ServerMessage, TunnelledStreamId, StreamKind, StreamSide}, protocol::{deserialize, serialize, Message}, tcp::{WritableStream, ReadableStream, self}, error::Error, tls::new_client_config};

type Sender = mpsc::Sender<Message<ClientMessage>>;
type Receiver = mpsc::Receiver<Message<ServerMessage>>;

pub async fn dial<'a, 'b>(server_url: url::Url) -> Result<(Sender, Receiver)> {
    let connector = match new_client_config() {
        None => None,
        Some(config) => Some(Connector::Rustls(Arc::new(config))),
    };

    let (ws_stream, _) = connect_async_tls_with_config(
        server_url,
        None,
        false,
        connector,
    ).await?;

    let (mut tx, mut rx) = ws_stream.split();

    let (mut server_msg_tx, server_msg_rx) = mpsc::channel::<Message<ServerMessage>>(16);
    let (client_msg_tx, mut client_msg_rx) = mpsc::channel::<Message<ClientMessage>>(16);

    tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            let msg = msg.map_err(anyhow::Error::msg);

            match recv_server_message(&mut server_msg_tx, msg).await {
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
        while let Some(msg) = client_msg_rx.recv().await {
            match send_client_message(&mut tx, msg).await {
                Ok(()) => continue,
                Err(err) => {
                    error!("sending client message: {:?}", err);
                    return
                }
            }
        }
    });

    Ok((client_msg_tx, server_msg_rx))
}

async fn recv_server_message(
    tx: &mut mpsc::Sender<Message<ServerMessage>>,
    msg: Result<WSMessage>,
) -> Result<Option<()>> {
    let msg: Option<Message<ServerMessage>> = deserialize(msg)?;

    match msg {
        Some(val) => {
            tx.send(val).await?;
            Ok(Some(()))
        }
        None => Ok(None)
    }
}

async fn send_client_message(
    tx: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WSMessage>,
    msg: Message<ClientMessage>,
    ) -> Result<()> {
    let msg = serialize(msg)?;
    tx.send(msg).await?;
    Ok(())
}

pub async fn init(
    api_key: String,
    server_url: Url,
    expose: Vec<SocketAddr>,
) -> Result<()> {
    let (tx, mut rx) = dial(server_url).await?;

    let (stream_tx, mut stream_rx) = mpsc::channel::<(WritableStream, ReadableStream)>(16);
    let (read_done_tx, mut read_done_rx) = mpsc::channel::<TunnelledStreamId>(16);

    if api_key != "" {
        tx.send(
            Message::Message(
                ClientMessage::AuthenticateRequest{ api_key },
            )
        ).await?;
    }

    for (i, _) in expose.iter().enumerate() {
        tx.send(
            Message::Message(
                ClientMessage::ExposeRequest{
                    kind: StreamKind::Tcp,
                    local_id: i as u32,
                },
            )
        ).await?;
    }

    let mut tunnels =  HashMap::with_capacity(expose.len());

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
                                let &socket_addr = expose.get(res.local_id as usize).ok_or_else(|| Error::InvalidLocalId)?;
                                tunnels.insert(res.tunnel_id, socket_addr);

                                info!("created tunnel {} to {} via {}", res.tunnel_id, socket_addr, res.url);
                            }
                            Err(err) => {
                                 // TODO do not use anyhow!.
                                return Err(anyhow!("failed to expose: {}", err))
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
    let (writable, readable) = tcp::dial(id, addr, cancel).await?;

    tx.send((writable, readable)).await?;

    Ok(())
}
