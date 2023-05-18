use anyhow::Result;
use futures::stream::SplitSink;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, WebSocketStream, MaybeTlsStream, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use log::error;

use crate::{message::{ClientMessage, ServerMessage}, protocol::{self, deserialize, serialize}};

type Sender = mpsc::Sender<protocol::Message<ClientMessage>>;
type Receiver = mpsc::Receiver<protocol::Message<ServerMessage>>;

pub async fn dial<'a, 'b>(server_url: url::Url) -> Result<(Sender, Receiver)> {
    let (ws_stream, _) = connect_async(server_url).await?;

    let (mut tx, mut rx) = ws_stream.split();

    let (mut server_msg_tx, server_msg_rx) = mpsc::channel::<protocol::Message<ServerMessage>>(16);
    let (client_msg_tx, mut client_msg_rx) = mpsc::channel::<protocol::Message<ClientMessage>>(16);

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
    tx: &mut mpsc::Sender<protocol::Message<ServerMessage>>,
    msg: Result<Message>,
) -> Result<Option<()>> {
    let msg: Option<protocol::Message<ServerMessage>> = deserialize(msg)?;

    match msg {
        Some(val) => {
            tx.send(val).await?;
            Ok(Some(()))
        }
        None => Ok(None)
    }
}

async fn send_client_message(
    tx: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    msg: protocol::Message<ClientMessage>,
    ) -> Result<()> {
    let msg = serialize(msg)?;
    tx.send(msg).await?;
    Ok(())
}
