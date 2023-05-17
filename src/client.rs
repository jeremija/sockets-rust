use anyhow::Result;
use futures::stream::{SplitSink, SplitStream};
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, MaybeTlsStream, tungstenite::{protocol::Message, Error}};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::message::{ClientMessage, ServerMessage};

type Sender = mpsc::Sender<ClientMessage>;
type Receiver = mpsc::Receiver<ServerMessage>;

pub async fn split_stream(ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> (Sender, Receiver) {
    let (mut tx, mut rx) = ws_stream.split();

    let (mut server_msg_tx, server_msg_rx) = mpsc::channel::<ServerMessage>(16);
    let (client_msg_tx, mut client_msg_rx) = mpsc::channel::<ClientMessage>(16);

    tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            let msg = msg.map_err(anyhow::Error::msg);

            match recv_server_message(&mut server_msg_tx, msg).await {
                Ok(()) => continue,
                Err(err) => {
                    eprintln!("error receiving server message: {:?}", err);
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
                    eprintln!("error sending client message: {:?}", err);
                    return
                }
            }
        }
        // while let Some(msg) = rx.next().await {
        //     let msg = msg.map_err(anyhow::Error::msg);

        //     match process_server_message(&mut server_msg_tx, msg).await {
        //         Ok(()) => continue,
        //         Err(err) => {
        //             eprintln!("error processing message: {:?}", err);
        //             return
        //         }
        //     }
        // }
    });

    (client_msg_tx, server_msg_rx)
}

fn parse_server_message(msg: Result<Message>) -> Result<Option<ServerMessage>> {
    let msg = msg?;

    let json: Vec<u8>;

    match msg {
    Message::Binary(b) => {
        json = b
    },
    Message::Ping(_) => return Ok(None),
    Message::Pong(_) => return Ok(None),
    Message::Text(_) => return Ok(None),
    Message::Close(_msg) => return Ok(None),
    Message::Frame(_msg) => unreachable!(),
    }

    let srv_msg: ServerMessage = serde_json::from_slice(json.as_slice())?;

    Ok(Some(srv_msg))
}

async fn recv_server_message(
    tx: &mut mpsc::Sender<ServerMessage>,
    msg: Result<Message>,
) -> Result<()> {
    let srv_msg = parse_server_message(msg)?;

    if let Some(val) = srv_msg {
        tx.send(val).await?;
    }

    Ok(())
}

async fn send_client_message(
    tx: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    msg: ClientMessage,
    ) -> Result<()> {
    let json = serde_json::to_vec(&msg)?;
    tx.send(Message::Binary(json)).await?;
    Ok(())
}
