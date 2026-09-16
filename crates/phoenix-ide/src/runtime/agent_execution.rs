use std::collections::BTreeMap;

use phoenix_agents::{AgentConfig, AgentDefinition, ExecutionCandidate, ExecutionSelection};
use phoenix_core::domain::llm_types::ModelEffort;
use phoenix_llm::{EffortCapabilities, ExecutionRoute};

#[derive(Clone)]
struct ResolvedAgent {
    definition: AgentDefinition,
    execution: Option<ExecutionCandidate>,
}

#[derive(Clone, Default)]
pub(super) struct SpawnCatalog {
    agents: BTreeMap<String, ResolvedAgent>,
    tiers: BTreeMap<String, ExecutionCandidate>,
    routes: Vec<ExecutionRoute>,
}

pub(super) struct SelectedWorker {
    pub execution: ExecutionCandidate,
    pub name: Option<String>,
    pub persona: Option<String>,
}

impl SpawnCatalog {
    pub fn resolve(config: &AgentConfig, routes: Vec<ExecutionRoute>) -> Self {
        let mut catalog = Self {
            routes,
            ..Self::default()
        };
        for agent in &config.agents {
            let execution = match agent.execution.as_deref() {
                Some(candidates) => match catalog.first_available(candidates) {
                    Ok(candidate) => Some(candidate),
                    Err(error) => {
                        tracing::warn!(agent = %agent.name, %error, "named worker unavailable; excluded from spawn choices");
                        continue;
                    }
                },
                None => None,
            };
            catalog.agents.insert(
                agent.name.clone(),
                ResolvedAgent {
                    definition: agent.clone(),
                    execution,
                },
            );
        }
        for (name, candidates) in &config.tiers {
            match catalog.first_available(candidates) {
                Ok(candidate) => {
                    catalog.tiers.insert(name.clone(), candidate);
                }
                Err(error) => {
                    tracing::warn!(tier = %name, %error, "execution tier unavailable; excluded from spawn choices");
                }
            }
        }
        catalog
    }

    fn first_available(
        &self,
        candidates: &[ExecutionCandidate],
    ) -> Result<ExecutionCandidate, String> {
        for (index, candidate) in candidates.iter().enumerate() {
            if self
                .route(&candidate.model, &candidate.connection)
                .is_some()
            {
                self.validate(candidate)?;
                if index > 0 {
                    tracing::info!(model = %candidate.model, connection = %candidate.connection, skipped = index, "selected configured execution fallback");
                }
                return Ok(candidate.clone());
            }
        }
        Err("No configured model/connection candidate is available".to_string())
    }

    fn route(&self, model: &str, connection: &str) -> Option<&ExecutionRoute> {
        self.routes
            .iter()
            .find(|route| route.model == model && route.connection == connection)
    }

    fn validate(&self, candidate: &ExecutionCandidate) -> Result<(), String> {
        let route = self.route(&candidate.model, &candidate.connection).ok_or_else(|| {
            format!("Model '{}' through '{}' was not advertised; select an available execution choice", candidate.model, candidate.connection)
        })?;
        if let Some(effort) = candidate.reasoning_effort {
            if !route.supported_efforts.supports(effort) {
                return Err(format!(
                    "Model '{}' through '{}' does not support effort '{effort}'",
                    candidate.model, candidate.connection
                ));
            }
        }
        Ok(())
    }

    pub fn select(
        &self,
        agent_type: Option<&str>,
        selection: Option<&ExecutionSelection>,
        parent_model: &str,
        parent_effort: Option<ModelEffort>,
    ) -> Result<SelectedWorker, String> {
        let agent = agent_type
            .map(|name| {
                self.agents.get(name).ok_or_else(|| {
                    let alternatives = if self.agents.is_empty() {
                        "No named workers are callable. Omit agent_type for a generic worker."
                            .to_string()
                    } else {
                        format!(
                            "Callable agents: {}. Omit agent_type for a generic worker.",
                            self.agents.keys().cloned().collect::<Vec<_>>().join(", ")
                        )
                    };
                    format!("Unknown or unavailable agent_type '{name}'. {alternatives}")
                })
            })
            .transpose()?;
        let execution = match selection {
            Some(ExecutionSelection::Tier { name }) => self
                .tiers
                .get(name)
                .cloned()
                .ok_or_else(|| format!("Unknown or unavailable execution tier '{name}'"))?,
            Some(ExecutionSelection::Model {
                model,
                connection,
                reasoning_effort,
            }) => ExecutionCandidate {
                model: model.clone(),
                connection: connection.clone(),
                reasoning_effort: *reasoning_effort,
            },
            None => {
                if let Some(execution) = agent.and_then(|agent| agent.execution.clone()) {
                    execution
                } else {
                    let route = self
                        .routes
                        .iter()
                        .find(|route| route.model == parent_model)
                        .ok_or_else(|| format!("Parent model '{parent_model}' is unavailable"))?;
                    ExecutionCandidate {
                        model: route.model.clone(),
                        connection: route.connection.clone(),
                        reasoning_effort: parent_effort,
                    }
                }
            }
        };
        self.validate(&execution)?;
        Ok(SelectedWorker {
            execution,
            name: agent.map(|a| a.definition.name.clone()),
            persona: agent.map(|a| a.definition.body.clone()),
        })
    }

    pub fn schema(&self) -> serde_json::Value {
        use phoenix_tools::Tool;
        use phoenix_tools::{SpawnAgentsTool, SpawnModelChoice};
        let choices = self
            .routes
            .iter()
            .map(|route| SpawnModelChoice {
                model: route.model.clone(),
                connection: route.connection.clone(),
                efforts: match &route.supported_efforts {
                    EffortCapabilities::Supported(capabilities) => capabilities.levels().to_vec(),
                    EffortCapabilities::Unknown | EffortCapabilities::Unsupported => Vec::new(),
                },
            })
            .collect();
        SpawnAgentsTool::with_execution_choices(
            self.agents.values().map(|a| a.definition.clone()).collect(),
            self.tiers.keys().cloned().collect(),
            choices,
        )
        .input_schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_llm::NativeDefault;

    fn route(model: &str, connection: &str) -> ExecutionRoute {
        ExecutionRoute {
            model: model.to_string(),
            connection: connection.to_string(),
            supported_efforts: EffortCapabilities::supported(
                &[ModelEffort::Low, ModelEffort::High],
                NativeDefault::Known(ModelEffort::Low),
            ),
        }
    }

    fn config() -> AgentConfig {
        phoenix_agents::parse_config(
            r#"
version = 1
[agents.reviewer]
description = "Find defects"
instructions = "Review the task carefully."
execution = [
 { model = "opus", connection = "anthropic", reasoning_effort = "high" },
 { model = "sol", connection = "codex", reasoning_effort = "high" }
]
[agents.unavailable]
description = "Unavailable worker"
instructions = "Review."
execution = [{model = "opus", connection = "anthropic"}]
[tiers.fast]
execution = [{model = "luna", connection = "codex", reasoning_effort = "low"}]
"#,
        )
        .unwrap()
    }

    #[test]
    fn codex_only_catalog_prevents_hidden_opus_failure() {
        let catalog = SpawnCatalog::resolve(
            &config(),
            vec![route("sol", "codex"), route("luna", "codex")],
        );
        let selected = catalog
            .select(Some("reviewer"), None, "luna", None)
            .unwrap();
        assert_eq!(selected.execution.model, "sol");
        assert_eq!(selected.execution.connection, "codex");
        assert_eq!(selected.execution.reasoning_effort, Some(ModelEffort::High));
        assert_eq!(
            selected.persona.as_deref(),
            Some("Review the task carefully.")
        );
        assert!(catalog
            .select(Some("unavailable"), None, "sol", None)
            .is_err());
        let schema = catalog.schema();
        let agents = &schema["properties"]["tasks"]["items"]["properties"]["agent_type"]["enum"];
        assert_eq!(agents, &serde_json::json!(["reviewer"]));
        assert!(!schema.to_string().contains("opus"));
    }

    #[test]
    fn generic_omission_inherits_parent_execution() {
        let catalog = SpawnCatalog::resolve(&AgentConfig::default(), vec![route("sol", "codex")]);
        let selected = catalog
            .select(None, None, "sol", Some(ModelEffort::High))
            .unwrap();
        assert_eq!(selected.execution.model, "sol");
        assert_eq!(selected.execution.connection, "codex");
        assert_eq!(selected.execution.reasoning_effort, Some(ModelEffort::High));
        assert!(selected.persona.is_none());
    }

    #[test]
    fn unknown_worker_lists_only_callable_alternatives() {
        let catalog = SpawnCatalog::resolve(&config(), vec![route("sol", "codex")]);
        let error = catalog
            .select(Some("ghost"), None, "sol", None)
            .err()
            .expect("unknown worker must be rejected");
        assert_eq!(
            error,
            "Unknown or unavailable agent_type 'ghost'. Callable agents: reviewer. Omit agent_type for a generic worker."
        );
        assert!(catalog.select(Some("reviewer"), None, "sol", None).is_ok());
    }

    #[test]
    fn unknown_worker_with_empty_catalog_explains_generic_selection() {
        let catalog = SpawnCatalog::resolve(&AgentConfig::default(), vec![route("sol", "codex")]);
        let error = catalog
            .select(Some("ghost"), None, "sol", None)
            .err()
            .expect("unknown worker must be rejected");
        assert_eq!(
            error,
            "Unknown or unavailable agent_type 'ghost'. No named workers are callable. Omit agent_type for a generic worker."
        );
        assert!(catalog.select(None, None, "sol", None).is_ok());
    }

    #[test]
    fn override_replaces_execution_and_keeps_persona() {
        let catalog = SpawnCatalog::resolve(
            &config(),
            vec![route("sol", "codex"), route("luna", "codex")],
        );
        let explicit = ExecutionSelection::Model {
            model: "luna".into(),
            connection: "codex".into(),
            reasoning_effort: None,
        };
        let selected = catalog
            .select(
                Some("reviewer"),
                Some(&explicit),
                "sol",
                Some(ModelEffort::High),
            )
            .unwrap();
        assert_eq!(selected.execution.model, "luna");
        assert_eq!(selected.execution.reasoning_effort, None);
        assert!(selected.persona.is_some());
        let tier = ExecutionSelection::Tier {
            name: "fast".into(),
        };
        let selected = catalog
            .select(
                Some("reviewer"),
                Some(&tier),
                "sol",
                Some(ModelEffort::High),
            )
            .unwrap();
        assert_eq!(selected.execution.reasoning_effort, Some(ModelEffort::Low));
        assert!(selected.persona.is_some());
    }

    #[test]
    fn invalid_reached_effort_does_not_fall_through() {
        let mut config = config();
        config.agents[0].execution.as_mut().unwrap()[0].reasoning_effort = Some(ModelEffort::Max);
        let catalog = SpawnCatalog::resolve(
            &config,
            vec![route("opus", "anthropic"), route("sol", "codex")],
        );
        assert!(catalog.select(Some("reviewer"), None, "sol", None).is_err());
    }

    #[test]
    fn explicit_connection_is_exact_and_never_falls_back() {
        let catalog = SpawnCatalog::resolve(&config(), vec![route("sol", "codex")]);
        let selection = ExecutionSelection::Model {
            model: "sol".into(),
            connection: "openai_responses".into(),
            reasoning_effort: None,
        };
        assert!(catalog
            .select(Some("reviewer"), Some(&selection), "sol", None)
            .is_err());
    }

    #[test]
    fn advertisement_snapshot_does_not_reresolve_a_worker() {
        let config = config();
        let first = SpawnCatalog::resolve(&config, vec![route("sol", "codex")]);
        let next = SpawnCatalog::resolve(
            &config,
            vec![route("opus", "anthropic"), route("sol", "codex")],
        );
        assert_eq!(
            first
                .select(Some("reviewer"), None, "sol", None)
                .unwrap()
                .execution
                .model,
            "sol"
        );
        assert_eq!(
            next.select(Some("reviewer"), None, "sol", None)
                .unwrap()
                .execution
                .model,
            "opus"
        );
    }
}
