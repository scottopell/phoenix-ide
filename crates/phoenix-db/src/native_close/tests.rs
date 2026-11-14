use super::{NativeCloseCommand, NativeCloseCommandError, NativeCloseIntent};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use uuid::Version;

fn command(transcript: &str, message: &str, tool_use: &str, target: &str) -> NativeCloseCommand {
    NativeCloseCommand::new(
        transcript.into(),
        SourceToolCall {
            message_id: message.into(),
            tool_use_id: tool_use.into(),
        },
        NativeCloseIntent::Initial {
            target: ProductConversationId::parse(target).unwrap(),
        },
    )
    .unwrap()
}

#[test]
fn same_source_and_intent_are_stable() {
    let first = command("source", "message", "tool", "target");
    assert_eq!(first, command("source", "message", "tool", "target"));
    assert_eq!(first.request_id().get_version(), Some(Version::Sha1));
    assert_eq!(first.fingerprint().len(), 64);
    assert_eq!(first.source_transcript_id(), "source");
    assert_eq!(first.source().message_id, "message");
    assert!(matches!(first.intent(), NativeCloseIntent::Initial { .. }));
}

#[test]
fn source_tuple_is_length_framed_and_each_component_changes_request_identity() {
    let base = command("ab", "c", "d", "target");
    for (transcript, message, tool_use) in [
        ("a", "bc", "d"), // Same concatenated bytes, different boundary.
        ("other", "c", "d"),
        ("ab", "other", "d"),
        ("ab", "c", "other"),
    ] {
        assert_ne!(
            base.request_id(),
            command(transcript, message, tool_use, "target").request_id()
        );
    }
    assert_ne!(
        command("a", "bc", "d", "target").request_id(),
        command("a", "b", "cd", "target").request_id()
    );
}

#[test]
fn target_changes_fingerprint_but_not_source_request_identity() {
    let first = command("source", "message", "tool", "first");
    let changed_target = command("source", "message", "tool", "second");
    let changed_source = command("source-2", "message", "tool", "first");
    assert_eq!(first.request_id(), changed_target.request_id());
    assert_ne!(first.fingerprint(), changed_target.fingerprint());
    assert_ne!(first.request_id(), changed_source.request_id());
    assert_eq!(first.fingerprint(), changed_source.fingerprint());
}

#[test]
fn empty_source_fields_are_rejected() {
    for (transcript, message, tool_use) in [
        ("", "message", "tool"),
        ("source", "", "tool"),
        ("source", "message", ""),
        (" \t", "message", "tool"),
        ("source", " \t", "tool"),
        ("source", "message", " \t"),
    ] {
        assert_eq!(
            NativeCloseCommand::new(
                transcript.into(),
                SourceToolCall {
                    message_id: message.into(),
                    tool_use_id: tool_use.into(),
                },
                NativeCloseIntent::Initial {
                    target: ProductConversationId::parse("target").unwrap(),
                },
            ),
            Err(NativeCloseCommandError::EmptySource)
        );
    }
}
