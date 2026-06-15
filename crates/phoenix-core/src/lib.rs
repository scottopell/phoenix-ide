//! Shared domain vocabulary for Phoenix IDE.
//!
//! This crate is the acyclic base of the workspace: it holds serializable
//! domain *types* (the nouns) and deliberately no business logic (the verbs).
//! Every other phoenix-ide crate depends on it; it depends on nothing in the
//! workspace. See `specs/` for the layering rationale.

pub mod domain;
pub mod git;
pub mod llm_language;
pub mod llm_service;
pub mod platform;
pub mod runtime_env;
pub mod task_handoff;
pub mod task_source;
pub mod work_scope;
