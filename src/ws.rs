#[allow(unused)]
fn print_stack_high() {
    let stack_high =
        unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark2(std::ptr::null_mut()) };
    log::info!("Stack high: {}", stack_high);
}

use crate::{app::Event, protocol::ServerEvent};
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio_websockets::Message;

pub struct Server {
    pub uri: String,
    timeout: std::time::Duration,
    ws: tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>,
}

impl Server {
    // 基于tokio websocket创建一个 server 连接
    pub async fn new(uri: String) -> anyhow::Result<Self> {
        let (ws, _resp) = tokio_websockets::ClientBuilder::new()
            .uri(&uri)?
            .connect()
            .await?;

        let timeout = std::time::Duration::from_secs(30);

        Ok(Self { uri, timeout, ws })
    }

    pub fn set_timeout(&mut self, timeout: std::time::Duration) {
        self.timeout = timeout;
    }
    // 用于向 server 发送 msg
    pub async fn send(&mut self, msg: Message) -> anyhow::Result<()> {
        tokio::time::timeout(self.timeout, self.ws.send(msg))
            .map_err(|_| anyhow::anyhow!("Timeout sending message"))
            .await??;
        Ok(())
    }
    // 用于接收 server 的事件
    pub async fn recv(&mut self) -> anyhow::Result<Event> {
        let msg = self
            .ws
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("WS channel closed"))??;

        if msg.is_binary() {
            let payload = msg.into_payload();
            let evt = rmp_serde::from_slice::<ServerEvent>(&payload)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize binary data: {}", e))?;
            Ok(Event::ServerEvent(evt))
        } else {
            Err(anyhow::anyhow!("Invalid message type"))
        }
    }
}
