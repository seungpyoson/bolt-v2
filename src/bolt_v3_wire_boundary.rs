//! Runtime wire-boundary adapters for transport surfaces that feed deploy or
//! readiness evidence.

use std::sync::{Arc, atomic::AtomicU8};

use nautilus_network::{
    error::SendError,
    mode::ConnectionMode,
    ratelimiter::quota::Quota,
    transport::{Message, TransportError},
    websocket::{MessageHandler, PingHandler, WebSocketClient},
};

pub use nautilus_network::websocket::{TransportBackend, WebSocketConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMessage {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

impl WireMessage {
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into().into_bytes())
    }

    #[must_use]
    pub fn binary(value: impl AsRef<[u8]>) -> Self {
        Self::Binary(value.as_ref().to_vec())
    }

    fn from_message(message: Message) -> Self {
        match message {
            Message::Text(bytes) => Self::Text(bytes.to_vec()),
            Message::Binary(bytes) => Self::Binary(bytes.to_vec()),
            Message::Ping(bytes) => Self::Ping(bytes.to_vec()),
            Message::Pong(bytes) => Self::Pong(bytes.to_vec()),
            Message::Close(_) => Self::Close,
        }
    }
}

pub type WireMessageHandler = Arc<dyn Fn(WireMessage) + Send + Sync>;
pub type WirePingHandler = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

#[derive(Debug)]
pub struct BoundaryWebSocket {
    inner: WebSocketClient,
}

impl BoundaryWebSocket {
    #[must_use]
    pub fn connection_mode(&self) -> ConnectionMode {
        self.inner.connection_mode()
    }

    #[must_use]
    pub fn connection_mode_atomic(&self) -> Arc<AtomicU8> {
        self.inner.connection_mode_atomic()
    }

    pub async fn disconnect(&self) {
        self.inner.disconnect().await;
    }

    pub async fn send_text(&self, data: String) -> Result<(), SendError> {
        self.inner.send_text(data, None).await
    }
}

pub async fn connect_websocket(
    config: WebSocketConfig,
    message_handler: Option<WireMessageHandler>,
    ping_handler: Option<WirePingHandler>,
    post_reconnection: Option<Arc<dyn Fn() + Send + Sync>>,
    keyed_quotas: Vec<(String, Quota)>,
    default_quota: Option<Quota>,
) -> Result<BoundaryWebSocket, TransportError> {
    let message_handler = message_handler.map(|handler| -> MessageHandler {
        Arc::new(move |message| handler(WireMessage::from_message(message)))
    });
    let ping_handler = ping_handler.map(|handler| -> PingHandler { handler });
    let inner = WebSocketClient::connect(
        config,
        message_handler,
        ping_handler,
        post_reconnection,
        keyed_quotas,
        default_quota,
    )
    .await?;
    Ok(BoundaryWebSocket { inner })
}
