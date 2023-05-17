use serde::{Deserialize, Serialize};

use crate::base64;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// ExposeRequest is sent when a TCPListener is successfully tunneled to
    /// the server side.
    ExposeRequest {
        kind: StreamKind
    },
    /// UnexposeRequest is sent when a TCPListener is successfully removed
    /// from the server side.
    UnexposeRequest {
        tunnel_id: TunnelId
    },
    /// NewStreamResponse is sent when a new tunneled connection has been
    /// establihsed.
    NewStreamResponse(Result<StreamId, String>),
    /// Data is sent when data was read from a local connection.
    Data {
        stream_id: StreamId,
        #[serde(with = "base64")]
        bytes: Vec<u8>,
    },
    /// StreamClosed is sent when the local connection has closed.
    StreamClosed {
        stream_id: StreamId,
        side: StreamSide,
    },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "type")]
pub enum ServerMessage{
    /// ExposeResponse is sent after a tunnel has been established.
    ExposeResponse {
        tunnel_id: TunnelId,
        url: String,
    },
    /// UnexposeResponse is sent after a tunnel has been removed.
    UnexposeResponse {
        tunnel_id: TunnelId,
    },
    /// NewStreamRequest is sent when a new remote stream wants to connect to
    /// an exposed connection.
    NewStreamRequest {
        tunnel_id: TunnelId,
        stream_id: StreamId,
    },
    /// Data contains the data received from teh remote stream.
    Data {
        stream_id: StreamId,
        #[serde(with = "base64")]
        bytes: Vec<u8>,
    },
    /// StreamClosed is sent when the remote stream has terminated.
    StreamClosed {
        stream_id: StreamId,
        side: StreamSide,
    },
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Copy, Clone)]
pub enum StreamKind {
    /// Tcp is used for exposing raw Tcp listeners.
    /// TODO add Http(s) and Websocket support.
    Tcp,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Copy, Clone)]
pub enum StreamSide {
    Read,
    Write,
    Both,
}

pub type TunnelId = uuid::Uuid;

pub type StreamId = uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::{ClientMessage, ServerMessage, StreamKind};
    use uuid::Uuid;

    #[test]
    fn serde_json_client_message() {
        let msg = ClientMessage::ExposeRequest{
            kind: StreamKind::Tcp,
        };

        let json = serde_json::to_value(msg.clone())
            .expect("failed to serialize json");

        let msg2: ClientMessage = serde_json::from_value(json)
            .expect("failed to deserialize json");

        assert_eq!(msg, msg2, "expected {:?} to be eqaul to {:?}", msg, msg2);
    }

    #[test]
    fn serde_json_server_message() {
        let msg = ServerMessage::ExposeResponse{
            url: "tcp.test.com:52131".to_string(),
            tunnel_id: Uuid::new_v4(),
        };

        let json = serde_json::to_value(msg.clone())
            .expect("failed to serialize json");

        let msg2: ServerMessage = serde_json::from_value(json)
            .expect("failed to deserialize json");

        assert_eq!(msg, msg2, "expected {:?} to be eqaul to {:?}", msg, msg2);
    }
}
