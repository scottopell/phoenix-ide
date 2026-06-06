//! The shared domain-model closure: serializable types co-owned by the
//! db, state-machine, llm and tools layers. Lives in the base crate so those
//! layers depend *down* onto a common vocabulary instead of onto each other.

pub mod bash_types;
pub mod db_schema;
pub mod kill_signal;
pub mod llm_error_kind;
pub mod llm_types;
pub mod mode_context;
pub mod patch_types;
pub mod pr_display_state;
pub mod process_inspection;
pub mod quota_details;
pub mod retry_policy;
pub mod skill_invocation;
pub mod sm_event;
pub mod sm_state;
pub mod tool_wire;
pub mod work_scope_inventory;
