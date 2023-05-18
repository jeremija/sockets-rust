use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::base64;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "type")]
pub enum ClientMessage {
    AuthenticateRequest{api_key: String},
    /// ExposeRequest is sent when a TCPListener is successfully tunneled to
    /// the server side.
    ExposeRequest {
        local_id: u32,
        kind: StreamKind
    },
    /// UnexposeRequest is sent when a TCPListener is successfully removed
    /// from the server side.
    UnexposeRequest {
        tunnel_id: TunnelId
    },
    /// NewStreamResponse is sent when a new tunneled connection has been
    /// establihsed.
    NewStreamResponse{
        id: TunnelledStreamId,
        result: Result<(), String>,
    },
    /// Data is sent when data was read from a local connection.
    Data {
        id: TunnelledStreamId,
        #[serde(with = "base64")]
        bytes: Vec<u8>,
    },
    /// StreamClosed is sent when the local connection has closed.
    StreamClosed {
        id: TunnelledStreamId,
        side: StreamSide,
    },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "type")]
pub enum ServerMessage{
    Unauthorized,
    /// ExposeResponse is sent after a tunnel has been established.
    ExposeResponse(Result<ExposeResponse, String>),
    /// UnexposeResponse is sent after a tunnel has been removed.
    UnexposeResponse {
        tunnel_id: TunnelId,
    },
    /// NewStreamRequest is sent when a new remote stream wants to connect to
    /// an exposed connection.
    NewStreamRequest(TunnelledStreamId),
    /// Data contains the data received from teh remote stream.
    Data {
        id: TunnelledStreamId,
        #[serde(with = "base64")]
        bytes: Vec<u8>,
    },
    /// StreamClosed is sent when the remote stream has terminated.
    StreamClosed {
        id: TunnelledStreamId,
        side: StreamSide,
    },
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct ExposeResponse {
    pub local_id: u32,
    pub tunnel_id: TunnelId,
    pub url: String,
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
    // Both,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Copy, Hash, Clone)]
pub struct TunnelId(uuid::Uuid);

impl TunnelId {
    pub fn rand() -> Self {
        Self(Uuid::new_v4())
    }

}

impl std::fmt::Display for TunnelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Copy, Hash, Clone)]
pub struct StreamId(uuid::Uuid);

impl StreamId {
    pub fn rand() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Hash, Eq)]
pub struct TunnelledStreamId {
    pub tunnel_id: TunnelId,
    pub stream_id: StreamId,
}

#[cfg(test)]
mod tests {
    use crate::message::{ExposeResponse, TunnelId};

    use super::{ClientMessage, ServerMessage, StreamKind};

    #[test]
    fn serde_json_client_message() {
        let msg = ClientMessage::ExposeRequest{
            local_id: 123,
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
        let msg = ServerMessage::ExposeResponse(
            Ok(
                ExposeResponse{
                    local_id: 123,
                    url: "tcp.test.com:52131".to_string(),
                    tunnel_id: TunnelId::rand(),
                },
            ),
        );

        let json = serde_json::to_value(msg.clone())
            .expect("failed to serialize json");

        let msg2: ServerMessage = serde_json::from_value(json)
            .expect("failed to deserialize json");

        assert_eq!(msg, msg2, "expected {:?} to be eqaul to {:?}", msg, msg2);
    }
}
