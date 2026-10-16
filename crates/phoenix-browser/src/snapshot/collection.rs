use async_tungstenite::tokio::{connect_async_with_config, ConnectStream};
use async_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};
use async_tungstenite::WebSocketStream;
use chromiumoxide::cdp::browser_protocol::{
    accessibility::GetFullAxTreeReturns, page::FrameId, target::TargetId,
};
use futures::StreamExt;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::BrowserError;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
struct Response<T> {
    id: Option<u32>,
    result: Option<T>,
    error: Option<ProtocolError>,
}

#[derive(Deserialize)]
struct ProtocolError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attached {
    session_id: String,
}

fn collection_error(failure: impl std::fmt::Display) -> BrowserError {
    super::error(format!(
        "Accessibility collection failed (maximum response 8 MiB): {failure}. Narrow the page content and call browser_snapshot again"
    ))
}

fn config(limit: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(limit))
        .max_frame_size(Some(limit))
}

async fn command<T: DeserializeOwned>(
    socket: &mut WebSocketStream<ConnectStream>,
    request: Value,
    id: u32,
) -> Result<T, BrowserError> {
    socket
        .send(Message::Text(request.to_string().into()))
        .await
        .map_err(collection_error)?;
    while let Some(message) = socket.next().await {
        let message = message.map_err(collection_error)?;
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                return Err(collection_error("browser closed the snapshot connection"));
            }
            continue;
        };
        let response: Response<T> = serde_json::from_str(&text).map_err(collection_error)?;
        if response.id != Some(id) {
            continue;
        }
        if let Some(failure) = response.error {
            return Err(collection_error(format!(
                "CDP {}: {}",
                failure.code, failure.message
            )));
        }
        return response
            .result
            .ok_or_else(|| collection_error("missing CDP result"));
    }
    Err(collection_error("snapshot connection ended"))
}

pub(super) async fn collect(
    address: &str,
    target: &TargetId,
    frame: FrameId,
) -> Result<GetFullAxTreeReturns, BrowserError> {
    let (mut socket, _) = connect_async_with_config(address, Some(config(MAX_RESPONSE_BYTES)))
        .await
        .map_err(collection_error)?;
    let attached: Attached = command(
        &mut socket,
        json!({"id":1,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}),
        1,
    )
    .await?;
    command(
        &mut socket,
        json!({"id":2,"sessionId":attached.session_id,"method":"Accessibility.getFullAXTree","params":{"frameId":frame}}),
        2,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn oversized_response(fragmented: bool) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = async_tungstenite::tokio::accept_async(stream)
                .await
                .unwrap();
            let _ = socket.next().await;
            if fragmented {
                use async_tungstenite::tungstenite::protocol::frame::{
                    coding::{Data, OpCode},
                    Frame,
                };
                for final_frame in [false, true] {
                    let opcode = if final_frame {
                        Data::Continue
                    } else {
                        Data::Text
                    };
                    let _ = socket
                        .send(Message::Frame(Frame::message(
                            vec![b'x'; 800],
                            OpCode::Data(opcode),
                            final_frame,
                        )))
                        .await;
                }
            } else {
                let _ = socket.send(Message::Text("x".repeat(2048).into())).await;
            }
        });
        let (mut socket, _) =
            connect_async_with_config(format!("ws://{address}"), Some(config(1024)))
                .await
                .unwrap();
        let result: Result<Value, _> = command(&mut socket, json!({"id":1}), 1).await;
        let failure = result.unwrap_err().to_string();
        assert!(failure.contains("Space limit exceeded"), "{failure}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn collection_rejects_oversized_frame_before_deserialization() {
        oversized_response(false).await;
    }

    #[tokio::test]
    async fn collection_rejects_fragmented_oversized_message_before_deserialization() {
        oversized_response(true).await;
    }
}
