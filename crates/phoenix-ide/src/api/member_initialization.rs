//! Typed member initialization: durable transcript evidence and process-local stream evidence.

use super::transcript_observation::{StableTranscriptRead, StreamDbMessageSelection};
use crate::runtime::{
    EnrichedConversation, InitTranscript, SseBroadcaster, SseEvent, TranscriptCoverage,
};

type StreamTranscriptRead = StableTranscriptRead<(
    i64,
    StreamDbMessageSelection,
    Vec<crate::db::Message>,
    Option<i64>,
    TranscriptCoverage,
)>;

/// Constructed only from a generation-stable durable read, never from a replay ring.
pub(super) struct PersistedTranscriptEvidence {
    conversation: crate::db::Conversation,
    last_sequence_id: i64,
    transcript: InitTranscript,
}

impl PersistedTranscriptEvidence {
    pub(super) fn from_stable_read(stable: StreamTranscriptRead) -> Self {
        let (last_sequence_id, _selection, messages, _server_tail, transcript_coverage) =
            stable.value;
        let transcript = transcript_from_coverage(messages, transcript_coverage);
        Self {
            conversation: stable.conversation,
            last_sequence_id,
            transcript,
        }
    }

    pub(super) fn conversation(&self) -> &crate::db::Conversation {
        &self.conversation
    }

    pub(super) fn messages(&self) -> &[crate::db::Message] {
        match &self.transcript {
            InitTranscript::Complete(messages) | InitTranscript::Tail(messages) => messages,
            InitTranscript::Preserve => &[],
        }
    }

    pub(super) fn sequence_floor(&self, stream: &StreamEvidence) -> i64 {
        let highest_message_seq = self
            .messages()
            .iter()
            .map(|m| m.sequence_id)
            .max()
            .unwrap_or(0);
        self.last_sequence_id
            .max(stream.highest_pending_seq)
            .max(highest_message_seq)
    }
}

/// Process-local proof; persisted rows cannot supply an incarnation or pending events.
/// Its only production constructor snapshots an actual broadcaster.
pub(super) struct StreamEvidence {
    stream_incarnation: String,
    pending_anchor_sequence_id: i64,
    pending_truncated: bool,
    highest_pending_seq: i64,
    pending_events: Vec<SseEvent>,
    cursor_replay_served: bool,
}

impl StreamEvidence {
    pub(super) fn capture(
        broadcast_tx: &SseBroadcaster,
        after_event_sequence: Option<i64>,
    ) -> Self {
        let (
            pending_anchor_sequence_id,
            pending_truncated,
            highest_pending_seq,
            pending_events,
            cursor_replay_served,
        ) = snapshot_pending_for_stream(broadcast_tx, after_event_sequence);
        Self {
            stream_incarnation: broadcast_tx.stream_incarnation().to_string(),
            pending_anchor_sequence_id,
            pending_truncated,
            highest_pending_seq,
            pending_events,
            cursor_replay_served,
        }
    }

    pub(super) fn cursor_replay_served(&self) -> bool {
        self.cursor_replay_served
    }

    #[cfg(test)]
    pub(super) fn into_snapshot_parts(self) -> (i64, bool, i64, Vec<SseEvent>, bool) {
        (
            self.pending_anchor_sequence_id,
            self.pending_truncated,
            self.highest_pending_seq,
            self.pending_events,
            self.cursor_replay_served,
        )
    }
}

pub(super) struct MemberInitializationMetadata {
    pub(super) conversation: EnrichedConversation,
    pub(super) steering_messages: Vec<crate::state_machine::event::SteerEntry>,
    pub(super) context_window_size: u64,
    pub(super) project_name: Option<String>,
}

pub(super) fn assemble_member_init(
    persisted: PersistedTranscriptEvidence,
    stream: StreamEvidence,
    metadata: MemberInitializationMetadata,
) -> SseEvent {
    let init_seq = persisted.sequence_floor(&stream);
    let conversation = persisted.conversation;
    let transcript = persisted.transcript;
    let MemberInitializationMetadata {
        conversation: init_conversation,
        steering_messages,
        context_window_size,
        project_name,
    } = metadata;
    let StreamEvidence {
        stream_incarnation,
        pending_anchor_sequence_id,
        pending_events,
        pending_truncated,
        ..
    } = stream;
    SseEvent::Init {
        sequence_id: init_seq,
        conversation: Box::new(init_conversation),
        transcript_generation: conversation.transcript_generation,
        transcript,
        steering_messages,
        agent_working: conversation.is_agent_working(),
        presentation_mode: super::handlers::conv_presentation_mode(&conversation).to_string(),
        last_sequence_id: init_seq,
        stream_incarnation,
        context_window_size,
        project_name,
        pending_anchor_sequence_id,
        pending_events,
        pending_truncated,
    }
}
fn snapshot_pending_for_stream(
    broadcast_tx: &crate::runtime::SseBroadcaster,
    after_event_sequence: Option<i64>,
) -> (i64, bool, i64, Vec<SseEvent>, bool) {
    if let Some(after_event_sequence) = after_event_sequence {
        if let Some((anchor, truncated, highest, events)) =
            broadcast_tx.snapshot_pending_after(after_event_sequence)
        {
            (anchor, truncated, highest, events, true)
        } else {
            let server_tip = broadcast_tx.current_seq();
            broadcast_tx.observe_seq(after_event_sequence.min(server_tip));
            let (anchor, truncated, highest, events) = broadcast_tx.snapshot_pending();
            (anchor, truncated, highest, events, false)
        }
    } else {
        let (anchor, truncated, highest, events) = broadcast_tx.snapshot_pending();
        (anchor, truncated, highest, events, false)
    }
}
fn transcript_from_coverage(
    messages: Vec<crate::db::Message>,
    transcript_coverage: TranscriptCoverage,
) -> InitTranscript {
    match transcript_coverage {
        TranscriptCoverage::Complete => InitTranscript::Complete(messages),
        TranscriptCoverage::Tail => InitTranscript::Tail(messages),
        TranscriptCoverage::Preserve => InitTranscript::Preserve,
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn legacy_init(
    conversation: &crate::db::Conversation,
    messages: Vec<crate::db::Message>,
    transcript_coverage: TranscriptCoverage,
    init_seq: i64,
    init_conversation: EnrichedConversation,
    steering_messages: Vec<crate::state_machine::event::SteerEntry>,
    broadcast_tx: &SseBroadcaster,
    context_window_size: u64,
    project_name: Option<String>,
    pending_anchor_sequence_id: i64,
    pending_events: Vec<SseEvent>,
    pending_truncated: bool,
) -> SseEvent {
    use super::handlers::conv_presentation_mode;
    // Create init event with typed data -- serialization deferred to SSE layer
    let transcript = match transcript_coverage {
        crate::runtime::TranscriptCoverage::Complete => {
            crate::runtime::InitTranscript::Complete(messages)
        }
        crate::runtime::TranscriptCoverage::Tail => crate::runtime::InitTranscript::Tail(messages),
        crate::runtime::TranscriptCoverage::Preserve => crate::runtime::InitTranscript::Preserve,
    };
    let init_event = SseEvent::Init {
        sequence_id: init_seq,
        conversation: Box::new(init_conversation),
        transcript_generation: conversation.transcript_generation,
        transcript,
        steering_messages,
        agent_working: conversation.is_agent_working(),
        presentation_mode: conv_presentation_mode(conversation).to_string(),
        last_sequence_id: init_seq,
        stream_incarnation: broadcast_tx.stream_incarnation().to_string(),
        context_window_size,
        project_name,
        pending_anchor_sequence_id,
        pending_events,
        pending_truncated,
    };
    init_event
}

#[cfg(test)]
mod tests {
    use super::super::member_observation::{MemberObservations, MemberProcessObservations};
    use super::super::transcript_observation::stable_transcript_read;
    use super::*;

    async fn database() -> crate::db::Database {
        let db = crate::db::Database::open_in_memory()
            .await
            .expect("database");
        db.create_conversation("init", "Initialization", "/tmp", true, None, None)
            .await
            .expect("conversation");
        db
    }

    async fn evidence(
        db: &crate::db::Database,
        coverage: TranscriptCoverage,
        last_sequence_id: i64,
    ) -> PersistedTranscriptEvidence {
        let stable = stable_transcript_read(db, "init", |db, id, _attempt| {
            Box::pin(async move {
                let messages = if coverage == TranscriptCoverage::Preserve {
                    Vec::new()
                } else {
                    db.get_messages(id).await.expect("messages")
                };
                Ok((
                    last_sequence_id,
                    if coverage == TranscriptCoverage::Preserve {
                        StreamDbMessageSelection::None
                    } else {
                        StreamDbMessageSelection::Latest
                    },
                    messages,
                    None,
                    coverage,
                ))
            })
        })
        .await
        .expect("stable transcript");
        PersistedTranscriptEvidence::from_stable_read(stable)
    }

    async fn metadata(
        db: &crate::db::Database,
        persisted: &PersistedTranscriptEvidence,
    ) -> MemberInitializationMetadata {
        let conv = persisted.conversation();
        let conversation = MemberObservations::new(db.clone())
            .read(conv, true)
            .await
            .expect("member facts")
            .project(
                conv,
                MemberProcessObservations {
                    shell: Some("/observed/shell".into()),
                    home_dir: "/observed/home".into(),
                    terminal_uses_tmux: true,
                    browser_session_active: true,
                },
            );
        MemberInitializationMetadata {
            conversation,
            steering_messages: vec![crate::state_machine::event::SteerEntry {
                origin: phoenix_core::domain::db_schema::InputOrigin::UserApi,
                text: "pending private input".into(),
                llm_text: Some("pending private input".into()),
                images: Vec::new(),
                files: Vec::new(),
                message_id: "steering".into(),
                user_agent: Some("initialization test".into()),
                skill_invocation: None,
            }],
            context_window_size: 1234,
            project_name: Some("observed-project".into()),
        }
    }

    fn wire_bytes(event: SseEvent) -> Vec<u8> {
        serde_json::to_vec(&super::super::wire::SseWireEvent::from(event)).expect("wire")
    }

    #[tokio::test]
    async fn init_wire_matches_original_assembly_for_all_coverage_and_pending_modes() {
        let db = database().await;
        let durable = db
            .add_message(
                "durable",
                "init",
                &crate::db::MessageContent::agent(vec![phoenix_llm::ContentBlock::text("durable")]),
                None,
                None,
            )
            .await
            .expect("persist message");
        for coverage in [
            TranscriptCoverage::Complete,
            TranscriptCoverage::Tail,
            TranscriptCoverage::Preserve,
        ] {
            for mode in 0..4 {
                let broadcaster = SseBroadcaster::new(16, 0);
                let _receiver = broadcaster.subscribe();
                broadcaster
                    .send_persisted_message(durable.clone())
                    .expect("durable anchor");
                if mode > 0 {
                    broadcaster
                        .send_seq(|sequence_id| SseEvent::Token {
                            sequence_id,
                            text: "pending".into(),
                            request_id: "request".into(),
                        })
                        .expect("token");
                    let mut eager = durable.clone();
                    eager.message_id = "eager".into();
                    eager.sequence_id = broadcaster.next_seq();
                    broadcaster
                        .send_ephemeral_message(eager)
                        .expect("eager message");
                }
                if mode == 3 {
                    for _ in 0..520 {
                        broadcaster
                            .send_seq(|sequence_id| SseEvent::Token {
                                sequence_id,
                                text: "overflow".into(),
                                request_id: "request".into(),
                            })
                            .expect("overflow token");
                    }
                }
                let cursor = (mode == 2).then_some(broadcaster.current_seq());
                let stream = StreamEvidence::capture(&broadcaster, cursor);
                let persisted = evidence(&db, coverage, durable.sequence_id).await;
                let metadata = metadata(&db, &persisted).await;
                let (anchor, truncated, highest, events) = if let Some(cursor) = cursor {
                    broadcaster.snapshot_pending_after(cursor).expect("cursor")
                } else {
                    broadcaster.snapshot_pending()
                };
                let highest_message = match coverage {
                    TranscriptCoverage::Preserve => durable.sequence_id,
                    TranscriptCoverage::Complete | TranscriptCoverage::Tail => persisted
                        .messages()
                        .iter()
                        .map(|m| m.sequence_id)
                        .max()
                        .unwrap_or(0),
                };
                let expected = legacy_init(
                    persisted.conversation(),
                    persisted.messages().to_vec(),
                    coverage,
                    durable.sequence_id.max(highest).max(highest_message),
                    metadata.conversation.clone(),
                    metadata.steering_messages.clone(),
                    &broadcaster,
                    metadata.context_window_size,
                    metadata.project_name.clone(),
                    anchor,
                    events,
                    truncated,
                );
                let actual = assemble_member_init(persisted, stream, metadata);
                assert_eq!(
                    wire_bytes(actual),
                    wire_bytes(expected),
                    "coverage={coverage:?}, mode={mode}"
                );
            }
        }
    }

    #[tokio::test]
    async fn sequence_floor_excludes_allocated_but_unincluded_events_and_truncated_tokens() {
        let db = database().await;
        let persisted = evidence(&db, TranscriptCoverage::Preserve, 7).await;
        let broadcaster = SseBroadcaster::new(16, 4);
        let _receiver = broadcaster.subscribe();
        broadcaster
            .send_seq(|sequence_id| SseEvent::Token {
                sequence_id,
                text: "included".into(),
                request_id: "request".into(),
            })
            .expect("token");
        let stream = StreamEvidence::capture(&broadcaster, None);
        broadcaster.observe_seq(999);
        assert_eq!(persisted.sequence_floor(&stream), 7);
        for _ in 0..520 {
            broadcaster
                .send_seq(|sequence_id| SseEvent::Token {
                    sequence_id,
                    text: "overflow".into(),
                    request_id: "request".into(),
                })
                .expect("token");
        }
        let stream = StreamEvidence::capture(&broadcaster, None);
        assert!(stream.pending_truncated);
        assert!(stream.pending_events.is_empty());
        assert_eq!(persisted.sequence_floor(&stream), 7);
        assert!(!StreamEvidence::capture(&broadcaster, Some(i64::MAX)).cursor_replay_served());
        assert_eq!(broadcaster.current_seq(), 1519);
    }
}
