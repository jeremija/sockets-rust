use serde::{Serialize, de::DeserializeOwned};
use tokio_tungstenite::tungstenite::protocol::Message as WSMessage;
use anyhow::Result;

pub fn deserialize<T>(msg: Result<WSMessage>) -> Result<Option<Message<T>>>
where
    T: DeserializeOwned,
{
    match msg? {
    WSMessage::Binary(b) => {
        // TODO use a more efficient serialization method than JSON.
        let srv_msg: T = serde_json::from_slice(b.as_slice())?;
        Ok(Some(Message::Message(srv_msg)))
    },
    WSMessage::Text(txt) => {
        let srv_msg: T = serde_json::from_str(&txt)?;
        Ok(Some(Message::Message(srv_msg)))
    }
    WSMessage::Ping(_) => return Ok(Some(Message::Ping)),
    WSMessage::Pong(_) => return Ok(Some(Message::Pong)),
    WSMessage::Close(_msg) => return Ok(None),
    WSMessage::Frame(_msg) => unreachable!(),
    }
}

pub fn serialize<T>(msg: Message<T>) -> Result<WSMessage>
where
    T: Serialize
{
    match msg {
        Message::Message(msg) => {
            // TODO use a more efficient serialization method than JSON.
            let json = serde_json::to_vec(&msg)?;
            Ok(WSMessage::Binary(json))
        }
        Message::Ping => Ok(WSMessage::Ping(vec!(1))),
        Message::Pong => Ok(WSMessage::Pong(vec!(1))),
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Message<T> {
    Message(T),
    Ping,
    Pong,
}
