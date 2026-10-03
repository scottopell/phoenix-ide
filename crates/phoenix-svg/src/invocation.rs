/// A tool invocation within its durable assistant-message incarnation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SvgInvocationId {
    pub assistant_message_id: String,
    pub tool_use_id: String,
}

impl SvgInvocationId {
    #[must_use]
    pub fn new(assistant_message_id: impl Into<String>, tool_use_id: impl Into<String>) -> Self {
        Self {
            assistant_message_id: assistant_message_id.into(),
            tool_use_id: tool_use_id.into(),
        }
    }
}
