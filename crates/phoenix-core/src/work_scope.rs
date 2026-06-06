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
/// on the conversation id. The `Global` variant is a singleton scope used
/// by surfaces that want a work-affine resource not bound to any single
/// conversation — currently the `/new` page's global terminal
/// (REQ-TERM-WS-001).
///
/// Resources that adopt this primitive should construct it from existing
/// fields rather than threading both a worktree path and a conversation id
/// through their callsites. See REQ-PROJ-WS-001, REQ-TMUX-WS-001,
/// REQ-TERM-WS-001.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum WorkScope {
    Worktree(String),
    Conversation(String),
    Global,
}

impl WorkScope {
    pub fn resolve(conversation_id: impl Into<String>, worktree_path: Option<&Path>) -> Self {
        match worktree_path {
            Some(path) => Self::Worktree(path.to_string_lossy().into_owned()),
            None => Self::Conversation(conversation_id.into()),
        }
    }

    /// Stable string form for use as a registry/map key. Worktree,
    /// conversation, and global namespaces are kept disjoint so values
    /// that happen to look alike across variants cannot collide.
    #[must_use]
    pub fn stable_key(&self) -> String {
        match self {
            Self::Worktree(path) => format!("worktree:{path}"),
            Self::Conversation(id) => format!("conversation:{id}"),
            Self::Global => "global:".to_string(),
        }
    }

    /// Inverse of [`Self::stable_key`]: parse a `stable_key()`-shaped string
    /// back into a `WorkScope`. Used by surfaces that receive a scope key as
    /// an opaque path segment (e.g. the work-scope inventory endpoint) and
    /// must reconstruct the scope to query the in-memory registries.
    ///
    /// Returns `None` when the prefix is not one of the three known
    /// namespaces. The inner value is taken verbatim after the first `:`,
    /// so worktree paths or conversation ids containing a `:` round-trip
    /// faithfully (the namespace prefix never collides with the body
    /// because `stable_key` splits on the *first* colon).
    #[must_use]
    pub fn from_stable_key(key: &str) -> Option<Self> {
        if let Some(path) = key.strip_prefix("worktree:") {
            Some(Self::Worktree(path.to_string()))
        } else if let Some(id) = key.strip_prefix("conversation:") {
            Some(Self::Conversation(id.to_string()))
        } else if key == "global:" {
            Some(Self::Global)
        } else {
            None
        }
    }
}

impl fmt::Display for WorkScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worktree(path) => write!(f, "worktree:{path}"),
            Self::Conversation(id) => write!(f, "conversation:{id}"),
            Self::Global => write!(f, "global:"),
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
        assert_ne!(conv.stable_key(), WorkScope::Global.stable_key());
        assert_ne!(wt.stable_key(), WorkScope::Global.stable_key());
        assert_eq!(conv.stable_key(), "conversation:/tmp/wt-x");
        assert_eq!(wt.stable_key(), "worktree:/tmp/wt-x");
        assert_eq!(WorkScope::Global.stable_key(), "global:");
    }

    #[test]
    fn from_stable_key_round_trips_every_variant() {
        let cases = [
            WorkScope::Worktree("/tmp/wt-x".to_string()),
            // A worktree path containing a colon must round-trip — the
            // namespace split is on the FIRST colon only.
            WorkScope::Worktree("/tmp/odd:path".to_string()),
            WorkScope::Conversation("conv-1".to_string()),
            WorkScope::Conversation("id:with:colons".to_string()),
            WorkScope::Global,
        ];
        for scope in cases {
            let key = scope.stable_key();
            assert_eq!(
                WorkScope::from_stable_key(&key),
                Some(scope.clone()),
                "round-trip failed for {scope:?} (key={key})"
            );
        }
    }

    #[test]
    fn from_stable_key_rejects_unknown_namespace() {
        assert_eq!(WorkScope::from_stable_key("bogus:foo"), None);
        assert_eq!(WorkScope::from_stable_key(""), None);
        // `global` without the trailing colon is not the singleton key.
        assert_eq!(WorkScope::from_stable_key("global"), None);
    }

    #[test]
    fn display_matches_stable_key() {
        let conv = WorkScope::Conversation("c1".to_string());
        let wt = WorkScope::Worktree("/tmp/x".to_string());
        assert_eq!(format!("{conv}"), conv.stable_key());
        assert_eq!(format!("{wt}"), wt.stable_key());
        assert_eq!(
            format!("{}", WorkScope::Global),
            WorkScope::Global.stable_key()
        );
    }
}
