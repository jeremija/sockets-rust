use anyhow::Result;
use futures::stream::SplitSink;
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Message, MaybeTlsStream};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use log::{error, info};

use crate::{message::{ClientMessage, ServerMessage}, protocol::{self, serialize, deserialize}};

type Sender = mpsc::Sender<protocol::Message<ServerMessage>>;
type Receiver = mpsc::Receiver<protocol::Message<ClientMessage>>;

pub async fn from_stream(
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    addr: std::net::SocketAddr,
) -> Result<(Sender, Receiver)> {
    let (mut tx, mut rx) = ws_stream.split();

    let (mut client_msg_tx, client_msg_rx) = mpsc::channel::<protocol::Message<ClientMessage>>(16);
    let (server_msg_tx, mut server_msg_rx) = mpsc::channel::<protocol::Message<ServerMessage>>(16);

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
    tx: &mut mpsc::Sender<protocol::Message<ClientMessage>>,
    msg: Result<Message>,
) -> Result<Option<()>> {
    let msg: Option<protocol::Message<ClientMessage>> = deserialize(msg)?;

    match msg {
        Some(val) => {
            tx.send(val).await?;
            Ok(Some(()))
        }
        None => Ok(None)
    }
}

async fn send_server_message(
    tx: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    msg: protocol::Message<ServerMessage>,
    ) -> Result<()> {
    let msg = serialize(msg)?;
    tx.send(msg).await?;
    Ok(())
}
