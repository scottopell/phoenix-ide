use async_tungstenite::tokio::{connect_async_with_config, ConnectStream};
use async_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};
use async_tungstenite::WebSocketStream;
use chromiumoxide::cdp::browser_protocol::target::TargetId;
use futures::StreamExt;
use serde_json::{json, Value};

pub(crate) type Socket = WebSocketStream<ConnectStream>;

pub(crate) fn config(limit: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(limit))
        .max_frame_size(Some(limit))
}

pub(crate) async fn connect_target(
    address: &str,
    target: &TargetId,
    limit: usize,
) -> Result<(Socket, String), String> {
    let (mut socket, _) = connect_async_with_config(address, Some(config(limit)))
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(Message::Text(
            json!({"id":1,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    while let Some(message) = socket.next().await {
        let message = message.map_err(|error| error.to_string())?;
        match message {
            Message::Text(text) => {
                let response: Value =
                    serde_json::from_str(&text).map_err(|error| error.to_string())?;
                if response.get("id") != Some(&json!(1)) {
                    continue;
                }
                if let Some(error) = response.get("error") {
                    return Err(error.to_string());
                }
                let session = response["result"]["sessionId"]
                    .as_str()
                    .ok_or_else(|| "target attachment returned no session ID".to_owned())?
                    .to_owned();
                return Ok((socket, session));
            }
            Message::Close(_) => break,
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Err("target attachment connection ended".to_owned())
}
