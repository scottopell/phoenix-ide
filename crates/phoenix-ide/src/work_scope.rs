use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
