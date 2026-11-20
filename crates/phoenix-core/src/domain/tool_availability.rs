//! Conversation policy and provider-specific request snapshots.
use std::collections::BTreeSet;

use super::llm_types::{MessageRole, ToolDefinition};
use super::tool_discovery::{ToolDiscoveryMode, ToolDiscoveryResult};

#[derive(Debug, Clone)]
pub struct ToolPolicyMessage {
    pub source_message_id: Option<String>,
    pub role: MessageRole,
}

#[derive(Debug, Clone)]
pub enum ToolChange {
    Addition(ToolDefinition),
    Removal { name: String },
}

#[derive(Debug, Clone)]
pub struct PositionedToolChange {
    pub after_message_id: String,
    pub change: ToolChange,
}

#[derive(Debug, Clone)]
pub struct ToolAvailability {
    declarations: Vec<ToolDefinition>,
    callable_names: BTreeSet<String>,
    anthropic_initial_declarations: Vec<ToolDefinition>,
    anthropic_changes: Vec<PositionedToolChange>,
    continuation_id: Option<String>,
    discovery_mode: ToolDiscoveryMode,
    discovery_results: Vec<ToolDiscoveryResult>,
}

impl ToolAvailability {
    /// # Errors
    /// Rejects duplicate declarations or callable names without a declaration.
    pub fn new(
        declarations: Vec<ToolDefinition>,
        callable_names: BTreeSet<String>,
    ) -> Result<Self, String> {
        let names: BTreeSet<_> = declarations.iter().map(|tool| tool.name.clone()).collect();
        if names.len() != declarations.len() {
            return Err("tool declarations contain duplicate names".into());
        }
        if !callable_names.is_subset(&names) {
            return Err("callable tool has no retained declaration".into());
        }
        Ok(Self {
            anthropic_initial_declarations: declarations.clone(),
            declarations,
            callable_names,
            anthropic_changes: Vec::new(),
            continuation_id: None,
            discovery_mode: ToolDiscoveryMode::Eager,
            discovery_results: Vec::new(),
        })
    }

    /// # Panics
    /// Panics when declarations contain duplicate tool names.
    #[must_use]
    pub fn all(declarations: Vec<ToolDefinition>) -> Self {
        let callable_names = declarations.iter().map(|tool| tool.name.clone()).collect();
        Self::new(declarations, callable_names).expect("unique tool declarations")
    }

    /// # Errors
    /// Rejects malformed initial definitions, unknown tool changes, or empty anchors.
    pub fn with_anthropic_context(
        mut self,
        initial: Vec<ToolDefinition>,
        changes: Vec<PositionedToolChange>,
    ) -> Result<Self, String> {
        let retained: BTreeSet<_> = self
            .declarations
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        let mut known: BTreeSet<_> = initial.iter().map(|tool| tool.name.as_str()).collect();
        if known.len() != initial.len() || !known.is_subset(&retained) {
            return Err("invalid initial tool declarations".into());
        }
        for event in &changes {
            if event.after_message_id.is_empty() {
                return Err("tool change has no source anchor".into());
            }
            match &event.change {
                ToolChange::Addition(tool) => {
                    if !retained.contains(tool.name.as_str()) {
                        return Err("tool addition has no retained declaration".into());
                    }
                    known.insert(tool.name.as_str());
                }
                ToolChange::Removal { name } => {
                    if !known.contains(name.as_str()) {
                        return Err("tool removal has no initial or added declaration".into());
                    }
                }
            }
        }
        self.anthropic_initial_declarations = initial;
        self.anthropic_changes = changes;
        Ok(self)
    }

    /// # Errors
    /// Rejects an empty continuation identity.
    pub fn with_continuation_id(mut self, continuation_id: String) -> Result<Self, String> {
        if continuation_id.is_empty() {
            return Err("empty continuation identity".into());
        }
        self.continuation_id = Some(continuation_id);
        Ok(self)
    }

    /// # Errors
    /// Rejects ambiguous discovery owners, results, or duplicate selected schemas.
    pub fn with_discovery(
        mut self,
        mode: ToolDiscoveryMode,
        results: Vec<ToolDiscoveryResult>,
    ) -> Result<Self, String> {
        let mut invocations = BTreeSet::new();
        let mut result_ids = BTreeSet::new();
        for result in &results {
            if result.owner_message_id.is_empty()
                || result.tool_call_id.is_empty()
                || result.result_message_id.is_empty()
                || !invocations.insert((&result.owner_message_id, &result.tool_call_id))
                || !result_ids.insert(&result.result_message_id)
            {
                return Err("ambiguous tool discovery identity".into());
            }
            let mut names = BTreeSet::new();
            if result
                .tools
                .iter()
                .any(|tool| tool.name.is_empty() || !names.insert(&tool.name))
            {
                return Err("ambiguous tool discovery schemas".into());
            }
        }
        self.discovery_mode = mode;
        self.discovery_results = results;
        Ok(self)
    }

    #[must_use]
    pub const fn discovery_mode(&self) -> ToolDiscoveryMode {
        self.discovery_mode
    }

    #[must_use]
    pub fn discovery_results(&self) -> &[ToolDiscoveryResult] {
        &self.discovery_results
    }

    #[must_use]
    pub fn discovery_result(
        &self,
        result_message_id: &str,
        tool_call_id: &str,
    ) -> Option<&ToolDiscoveryResult> {
        self.discovery_results.iter().find(|result| {
            result.result_message_id == result_message_id && result.tool_call_id == tool_call_id
        })
    }

    #[must_use]
    pub fn continuation_id(&self) -> Option<&str> {
        self.continuation_id.as_deref()
    }

    #[must_use]
    pub fn declarations(&self) -> &[ToolDefinition] {
        &self.declarations
    }
    #[must_use]
    pub fn callable_names(&self) -> &BTreeSet<String> {
        &self.callable_names
    }
    #[must_use]
    pub fn is_callable(&self, name: &str) -> bool {
        self.callable_names.contains(name)
    }
    #[must_use]
    pub fn anthropic_initial_declarations(&self) -> &[ToolDefinition] {
        &self.anthropic_initial_declarations
    }
    #[must_use]
    pub fn anthropic_changes(&self) -> &[PositionedToolChange] {
        &self.anthropic_changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: String::new(),
            input_schema: serde_json::json!({"type":"object"}),
            defer_loading: false,
        }
    }
    #[test]
    fn continuation_rejects_unknown_tools_and_empty_anchors() {
        let policy = ToolAvailability::all(vec![definition("a")]);
        assert!(policy
            .clone()
            .with_anthropic_context(vec![definition("missing")], vec![])
            .is_err());
        assert!(policy
            .clone()
            .with_anthropic_context(
                vec![definition("a")],
                vec![PositionedToolChange {
                    after_message_id: "anchor".into(),
                    change: ToolChange::Removal {
                        name: "missing".into()
                    },
                }]
            )
            .is_err());
        assert!(policy
            .with_anthropic_context(
                vec![definition("a")],
                vec![PositionedToolChange {
                    after_message_id: String::new(),
                    change: ToolChange::Removal { name: "a".into() },
                }]
            )
            .is_err());
    }
    #[test]
    fn admission_requires_declaration_and_policy() {
        assert!(ToolAvailability::new(vec![], BTreeSet::from(["missing".into()])).is_err());
        assert!(
            ToolAvailability::new(vec![definition("a"), definition("a")], BTreeSet::new()).is_err()
        );
        let policy = ToolAvailability::new(vec![definition("a")], BTreeSet::new()).unwrap();
        assert!(policy.clone().with_continuation_id(String::new()).is_err());
        assert_eq!(
            policy
                .clone()
                .with_continuation_id("context".into())
                .unwrap()
                .continuation_id(),
            Some("context")
        );
        assert!(!policy.is_callable("a"));
        assert_eq!(policy.declarations().len(), 1);
    }

    #[test]
    fn discovery_distinguishes_empty_result_from_missing_and_rejects_ambiguous_identity() {
        use crate::domain::tool_discovery::ToolDiscoveryProtocol;
        let policy = ToolAvailability::all(vec![definition("a")]);
        let result = ToolDiscoveryResult {
            owner_message_id: "owner".into(),
            tool_call_id: "call".into(),
            result_message_id: "result".into(),
            protocol: ToolDiscoveryProtocol::OpenAiClient,
            tools: vec![],
        };
        let loaded = policy
            .clone()
            .with_discovery(ToolDiscoveryMode::OpenAiClient, vec![result.clone()])
            .unwrap();
        assert!(loaded
            .discovery_result("result", "call")
            .unwrap()
            .tools
            .is_empty());
        assert!(loaded.discovery_result("another-result", "call").is_none());
        assert!(loaded.discovery_result("result", "another-call").is_none());
        assert!(policy
            .clone()
            .with_discovery(
                ToolDiscoveryMode::Local,
                vec![result.clone(), result.clone()]
            )
            .is_err());
        let mut duplicate_schema = result;
        duplicate_schema.tools = vec![definition("a"), definition("a")];
        assert!(policy
            .with_discovery(ToolDiscoveryMode::Local, vec![duplicate_schema])
            .is_err());
    }
}
