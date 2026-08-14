//! Runtime wire-boundary adapters for transport surfaces that feed deploy or
//! readiness evidence.

use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

use nautilus_network::{
    RECONNECTED,
    error::SendError,
    mode::ConnectionMode,
    ratelimiter::{RateLimiter, quota::Quota},
    transport::{Message, TransportError},
    websocket::{EpochMessageHandler, PingHandler, WebSocketClient},
};
use ustr::Ustr;

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

fn adapt_epoch_message_handler(
    message_handler: Option<WireMessageHandler>,
    post_reconnection: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Option<EpochMessageHandler> {
    message_handler.as_ref()?;

    let last_reconnect_epoch = AtomicU64::new(0);
    Some(Arc::new(move |connection_epoch, message| {
        let is_reconnection =
            matches!(&message, Message::Text(bytes) if bytes.as_ref() == RECONNECTED.as_bytes());
        if let Some(handler) = &message_handler {
            handler(WireMessage::from_message(message));
        }
        let first_for_epoch = is_reconnection
            && connection_epoch
                > last_reconnect_epoch.fetch_max(connection_epoch, Ordering::AcqRel);
        if let (true, Some(callback)) = (first_for_epoch, post_reconnection.as_ref()) {
            callback();
        }
    }))
}

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
    let epoch_handler = adapt_epoch_message_handler(message_handler, post_reconnection)
        .ok_or_else(|| {
            TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Handler mode requires message_handler to be set",
            ))
        })?;
    let ping_handler = ping_handler.map(|handler| -> PingHandler { handler });
    let keyed_quotas = keyed_quotas
        .into_iter()
        .map(|(key, quota)| (Ustr::from(&key), quota))
        .collect();
    let rate_limiter = Arc::new(RateLimiter::new_with_quota(default_quota, keyed_quotas));
    let inner = WebSocketClient::connect_with_rate_limiter_and_epoch_handler(
        config,
        epoch_handler,
        ping_handler,
        rate_limiter,
    )
    .await?;
    Ok(BoundaryWebSocket { inner })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use nautilus_network::RECONNECTED;

    use super::*;

    #[test]
    fn adapted_handler_forwards_ordinary_messages_without_reconnect_callback() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_for_handler = Arc::clone(&received);
        let message_handler: WireMessageHandler = Arc::new(move |message| {
            received_for_handler
                .lock()
                .expect("received-message mutex poisoned")
                .push(message);
        });
        let reconnect_count = Arc::new(AtomicUsize::new(0));
        let reconnect_count_for_handler = Arc::clone(&reconnect_count);
        let post_reconnection: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            reconnect_count_for_handler.fetch_add(1, Ordering::SeqCst);
        });

        let handler = adapt_epoch_message_handler(Some(message_handler), Some(post_reconnection))
            .expect("message adapter should be present");
        handler(0, Message::Text("payload".into()));

        assert_eq!(
            received
                .lock()
                .expect("received-message mutex poisoned")
                .as_slice(),
            &[WireMessage::Text(b"payload".to_vec())]
        );
        assert_eq!(reconnect_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reconnect_callback_without_message_handler_preserves_handler_mode_rejection() {
        let post_reconnection: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});

        assert!(adapt_epoch_message_handler(None, Some(post_reconnection)).is_none());
    }

    #[test]
    fn epoch_zero_reconnect_text_cannot_invoke_reconnect_callback() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_message = Arc::clone(&calls);
        let message_handler: WireMessageHandler = Arc::new(move |_| {
            calls_for_message
                .lock()
                .expect("call-order mutex poisoned")
                .push("message");
        });
        let calls_for_reconnect = Arc::clone(&calls);
        let post_reconnection: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            calls_for_reconnect
                .lock()
                .expect("call-order mutex poisoned")
                .push("reconnect");
        });

        let handler = adapt_epoch_message_handler(Some(message_handler), Some(post_reconnection))
            .expect("message adapter should be present");
        handler(0, Message::Text(RECONNECTED.into()));

        assert_eq!(
            calls.lock().expect("call-order mutex poisoned").as_slice(),
            &["message"]
        );
    }

    #[test]
    fn reconnect_callback_runs_once_per_new_nonzero_epoch_after_forwarding() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_message = Arc::clone(&calls);
        let message_handler: WireMessageHandler = Arc::new(move |_| {
            calls_for_message
                .lock()
                .expect("call-order mutex poisoned")
                .push("message");
        });
        let calls_for_reconnect = Arc::clone(&calls);
        let post_reconnection: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            calls_for_reconnect
                .lock()
                .expect("call-order mutex poisoned")
                .push("reconnect");
        });

        let handler = adapt_epoch_message_handler(Some(message_handler), Some(post_reconnection))
            .expect("message adapter should be present");
        handler(1, Message::Text(RECONNECTED.into()));
        handler(1, Message::Text(RECONNECTED.into()));
        handler(0, Message::Text(RECONNECTED.into()));
        handler(2, Message::Text(RECONNECTED.into()));

        assert_eq!(
            calls.lock().expect("call-order mutex poisoned").as_slice(),
            &[
                "message",
                "reconnect",
                "message",
                "message",
                "message",
                "reconnect"
            ]
        );
    }
}
