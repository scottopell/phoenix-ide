use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Durable owner for work-affine resources.
///
/// `conversation_id` identifies a transcript/runtime instance. A `WorkScope`
/// identifies the unit of work the resource belongs to and is independent of
/// any single transcript: managed/branch worktrees survive context-exhaustion
/// continuations, so the scope must too. Direct-mode conversations have no
/// durable owner beyond the transcript itself, so they fall back to keying
/// on the conversation id.
///
/// Resources that adopt this primitive should construct it from existing
/// fields rather than threading both a worktree path and a conversation id
/// through their callsites. See REQ-PROJ-WS-001, REQ-TMUX-WS-001.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum WorkScope {
    Worktree(String),
    Conversation(String),
}

impl WorkScope {
    pub fn resolve(conversation_id: impl Into<String>, worktree_path: Option<&Path>) -> Self {
        match worktree_path {
            Some(path) => Self::Worktree(path.to_string_lossy().into_owned()),
            None => Self::Conversation(conversation_id.into()),
        }
    }

    /// Stable string form for use as a registry/map key. Worktree and
    /// conversation namespaces are kept disjoint so a worktree path that
    /// happens to look like a conversation id (or vice versa) cannot
    /// collide.
    pub fn stable_key(&self) -> String {
        match self {
            Self::Worktree(path) => format!("worktree:{path}"),
            Self::Conversation(id) => format!("conversation:{id}"),
        }
    }
}

impl fmt::Display for WorkScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worktree(path) => write!(f, "worktree:{path}"),
            Self::Conversation(id) => write!(f, "conversation:{id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_with_worktree_picks_worktree_scope() {
        let path = PathBuf::from("/tmp/wt-x");
        let scope = WorkScope::resolve("conv-1", Some(path.as_path()));
        assert_eq!(scope, WorkScope::Worktree("/tmp/wt-x".to_string()));
    }

    #[test]
    fn resolve_without_worktree_falls_back_to_conversation() {
        let scope = WorkScope::resolve("conv-1", None);
        assert_eq!(scope, WorkScope::Conversation("conv-1".to_string()));
    }

    #[test]
    fn stable_key_namespaces_are_disjoint() {
        let conv = WorkScope::Conversation("/tmp/wt-x".to_string());
        let wt = WorkScope::Worktree("/tmp/wt-x".to_string());
        assert_ne!(conv.stable_key(), wt.stable_key());
        assert_eq!(conv.stable_key(), "conversation:/tmp/wt-x");
        assert_eq!(wt.stable_key(), "worktree:/tmp/wt-x");
    }

    #[test]
    fn display_matches_stable_key() {
        let conv = WorkScope::Conversation("c1".to_string());
        let wt = WorkScope::Worktree("/tmp/x".to_string());
        assert_eq!(format!("{conv}"), conv.stable_key());
        assert_eq!(format!("{wt}"), wt.stable_key());
    }
}
