use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque durable identifier for a unit of work.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkScopeId(String);

impl WorkScopeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// # Errors
    /// Returns [`WorkScopeIdError`] when the supplied identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, WorkScopeIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WorkScopeIdError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WorkScopeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for WorkScopeId {
    type Err = WorkScopeIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Namespace key for persisted work and structurally separate global actors.
/// Coordinator scope is routing-only: its tool registry exposes no ordinary
/// work-affine resources.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ResourceScopeKey {
    Work(WorkScopeId),
    Coordinator,
    GlobalTerminal,
}

impl ResourceScopeKey {
    #[must_use]
    pub fn stable_key(&self) -> String {
        match self {
            Self::Work(id) => format!("work:{}", id.as_str()),
            Self::Coordinator => "coordinator".to_string(),
            Self::GlobalTerminal => "global_terminal".to_string(),
        }
    }

    #[must_use]
    pub fn from_stable_key(key: &str) -> Option<Self> {
        if key == "global_terminal" {
            return Some(Self::GlobalTerminal);
        }
        if key == "coordinator" {
            return Some(Self::Coordinator);
        }
        key.strip_prefix("work:")
            .and_then(|id| WorkScopeId::parse(id).ok())
            .map(Self::Work)
    }

    #[must_use]
    pub fn work_scope_id(&self) -> Option<&WorkScopeId> {
        match self {
            Self::Work(id) => Some(id),
            Self::Coordinator | Self::GlobalTerminal => None,
        }
    }
}

impl fmt::Display for ResourceScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.stable_key())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRole {
    #[default]
    User,
    SubAgent,
    Coordinator,
}

impl RuntimeRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::SubAgent => "sub_agent",
            Self::Coordinator => "coordinator",
        }
    }

    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "user" => Self::User,
            "sub_agent" => Self::SubAgent,
            "coordinator" => Self::Coordinator,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    RestrictedExplore,
    Work,
}

impl AuthorityKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RestrictedExplore => "restricted_explore",
            Self::Work => "work",
        }
    }
}

/// Authority stamped onto same-scope process and browser resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceAuthority {
    Restricted,
    Work,
}

/// The actor identity and effective authority presented to a resource manager.
/// Scope routing remains independent: this value only decides whether an actor
/// may control the resource found at that scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveResourceAccess {
    conversation_id: String,
    authority: ResourceAuthority,
    restricted_private: bool,
}

impl EffectiveResourceAccess {
    #[must_use]
    pub fn new(conversation_id: impl Into<String>, authority: ResourceAuthority) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            authority,
            restricted_private: authority == ResourceAuthority::Restricted,
        }
    }

    #[must_use]
    pub fn shared_restricted(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            authority: ResourceAuthority::Restricted,
            restricted_private: false,
        }
    }

    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    #[must_use]
    pub fn authority(&self) -> ResourceAuthority {
        self.authority
    }

    /// Work actors control resources in their scope. Restricted actors may
    /// control only restricted resources they created themselves.
    #[must_use]
    pub fn can_control(
        &self,
        creator_conversation_id: &str,
        resource_authority: ResourceAuthority,
    ) -> bool {
        self.authority == ResourceAuthority::Work
            || (resource_authority == ResourceAuthority::Restricted
                && self.conversation_id == creator_conversation_id)
    }

    #[must_use]
    pub fn with_authority(&self, authority: ResourceAuthority) -> Self {
        Self {
            conversation_id: self.conversation_id.clone(),
            authority,
            restricted_private: authority == ResourceAuthority::Restricted
                && self.restricted_private,
        }
    }

    /// Stable key for resources private to restricted sub-agents. User-facing
    /// Explore conversations return `None` and therefore share scope resources.
    #[must_use]
    pub fn restricted_private_key(&self) -> Option<&str> {
        self.restricted_private
            .then_some(self.conversation_id.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkScopeLifecycle {
    Active,
    Retired,
}

impl WorkScopeLifecycle {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnvironmentContext {
    AllocatedWorktree {
        cwd: String,
        worktree_path: String,
        branch_name: Option<String>,
        base_branch: Option<String>,
    },
    UnownedCwd {
        cwd: String,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkScopeRecord {
    pub id: WorkScopeId,
    pub authority_kind: AuthorityKind,
    pub lifecycle: WorkScopeLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkScopeEnvironmentRecord {
    pub work_scope_id: WorkScopeId,
    pub context: EnvironmentContext,
}

/// Proof that the runtime inspected every in-memory resource registry for one
/// scope and found no live resource. Its private field prevents database-only
/// callers from manufacturing retirement authority from durable state alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopeRetirementPrecondition {
    scope_id: WorkScopeId,
}

impl WorkScopeRetirementPrecondition {
    /// Construct the proof after the runtime has inventoried bash, tmux,
    /// terminal, and browser registries and established that none is live.
    #[must_use]
    pub fn after_runtime_inventory_found_no_live_resource(scope_id: WorkScopeId) -> Self {
        Self { scope_id }
    }

    #[must_use]
    pub fn scope_id(&self) -> &WorkScopeId {
        &self.scope_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkScopeRetirementOutcome {
    Retired,
    AlreadyRetired,
    Blocked(WorkScopeRetirementBlocker),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkScopeRetirementBlocker {
    CurrentUserOwner,
    UserSuccessor,
    ActiveSubAgent,
    PendingWakeOrWorkflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkScopeIdError {
    Empty,
}

impl fmt::Display for WorkScopeIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("work scope id must not be empty"),
        }
    }
}

impl std::error::Error for WorkScopeIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_namespaces_are_structurally_disjoint() {
        let work = ResourceScopeKey::Work(WorkScopeId::parse("opaque").unwrap());
        assert_eq!(work.stable_key(), "work:opaque");
        assert_eq!(ResourceScopeKey::from_stable_key("work:opaque"), Some(work));
        assert_eq!(
            ResourceScopeKey::from_stable_key("global_terminal"),
            Some(ResourceScopeKey::GlobalTerminal)
        );
        assert_eq!(
            ResourceScopeKey::from_stable_key("conversation:opaque"),
            None
        );
    }

    #[test]
    fn generated_ids_are_non_empty_and_distinct() {
        let a = WorkScopeId::new();
        let b = WorkScopeId::new();
        assert!(!a.as_str().is_empty());
        assert_ne!(a, b);
    }

    #[test]
    fn parse_rejects_empty_or_blank() {
        assert_eq!(WorkScopeId::parse(""), Err(WorkScopeIdError::Empty));
        assert_eq!(WorkScopeId::parse("  \t"), Err(WorkScopeIdError::Empty));
    }

    #[test]
    fn parse_accepts_opaque_non_empty_text() {
        let id = WorkScopeId::parse("scope-opaque").unwrap();
        assert_eq!(id.as_str(), "scope-opaque");
    }
}
