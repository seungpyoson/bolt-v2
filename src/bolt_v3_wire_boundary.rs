//! Runtime wire-boundary adapters for transport surfaces that feed deploy or
//! readiness evidence.

use std::sync::Arc;

use nautilus_network::{
    ratelimiter::quota::Quota,
    transport::TransportError,
    websocket::{MessageHandler, PingHandler, WebSocketClient, WebSocketConfig},
};

pub async fn connect_websocket(
    config: WebSocketConfig,
    message_handler: Option<MessageHandler>,
    ping_handler: Option<PingHandler>,
    post_reconnection: Option<Arc<dyn Fn() + Send + Sync>>,
    keyed_quotas: Vec<(String, Quota)>,
    default_quota: Option<Quota>,
) -> Result<WebSocketClient, TransportError> {
    WebSocketClient::connect(
        config,
        message_handler,
        ping_handler,
        post_reconnection,
        keyed_quotas,
        default_quota,
    )
    .await
}
