use super::db_schema::{Message, MessageContent};

/// Stable identity of a provider tool result within its owning assistant message.
#[must_use]
pub fn tool_result_message_id(assistant_message_id: &str, tool_use_id: &str) -> String {
    format!(
        "tool-result:{}:{assistant_message_id}:{tool_use_id}",
        assistant_message_id.len()
    )
}

/// Locate the newest durable result for an awaited provider tool ID.
#[must_use]
pub fn latest_tool_result_message_id<'a>(
    messages: &'a [Message],
    tool_use_id: &str,
) -> Option<&'a str> {
    messages.iter()
        .filter(|message| matches!(&message.content, MessageContent::Tool(content) if content.tool_use_id == tool_use_id))
        .max_by_key(|message| (message.sequence_id, &message.message_id))
        .map(|message| message.message_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_separates_rounds_and_unambiguously_encodes_components() {
        assert_ne!(
            tool_result_message_id("first", "same"),
            tool_result_message_id("second", "same")
        );
        assert_ne!(
            tool_result_message_id("a:b", "c"),
            tool_result_message_id("a", "b:c")
        );
        assert_eq!(
            tool_result_message_id("first", "same"),
            tool_result_message_id("first", "same")
        );
    }
}
