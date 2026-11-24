use phoenix_core::domain::executing_source::verify_executing_source;
use phoenix_core::domain::{db_schema::SourceToolCall, sm_state::ConvState};

use crate::{DbError, DbResult};

pub(crate) fn require_executing_source(
    state: &ConvState,
    source: &SourceToolCall,
    tool_name: &str,
    requested: &serde_json::Value,
) -> DbResult<()> {
    verify_executing_source(state, source, tool_name, requested).map_err(|_| {
        DbError::MessageConflict(
            "command requires the exact executing source occurrence and payload".into(),
        )
    })
}
