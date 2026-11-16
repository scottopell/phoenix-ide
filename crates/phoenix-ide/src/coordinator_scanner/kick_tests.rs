use std::{
    future::Future,
    sync::Arc,
    task::{Context, Waker},
};

use phoenix_core::domain::db_schema::{InputOrigin, Message, MessageContent, MessageType};
use tokio::sync::Notify;

use super::{SseBroadcaster, SseEvent};

fn pending_kick(notify: &Notify) -> bool {
    let mut future = std::pin::pin!(notify.notified());
    future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_ready()
}

fn message(sequence_id: i64) -> Message {
    Message {
        message_id: format!("scanner-kick-{sequence_id}"),
        conversation_id: "scanner-kick-source".into(),
        sequence_id,
        message_type: MessageType::User,
        content: MessageContent::user("Keep the requested outcome."),
        origin: InputOrigin::UserApi,
        display_data: None,
        usage_data: None,
        created_at: chrono::Utc::now(),
    }
}

#[test]
fn scanner_kick_follows_persisted_messages_without_sse_receivers_and_coalesces() {
    let notify = Arc::new(Notify::new());
    let broadcaster = SseBroadcaster::new(16, 0).with_scanner_kick(notify.clone());
    assert!(broadcaster.send_ephemeral_message(message(1)).is_err());
    assert!(!pending_kick(&notify));
    assert!(broadcaster.send_persisted_message(message(1)).is_err());
    assert!(broadcaster.send_persisted_message(message(2)).is_err());
    assert!(pending_kick(&notify));
    assert!(!pending_kick(&notify));
}

#[test]
fn scanner_kick_distinguishes_committed_generation_change_from_ephemeral_update() {
    let notify = Arc::new(Notify::new());
    let broadcaster = SseBroadcaster::new(16, 0).with_scanner_kick(notify.clone());
    for (generation, expected) in [(None, false), (Some(2), true)] {
        let _ = broadcaster.send_seq(|sequence_id| SseEvent::MessageUpdated {
            opening: None,
            sequence_id,
            message_id: "scanner-existing".into(),
            transcript_generation: generation,
            display_data: None,
            content: None,
            duration_ms: None,
        });
        assert_eq!(pending_kick(&notify), expected);
    }
}
