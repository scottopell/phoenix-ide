use super::agent_execution::SpawnCatalog;
use phoenix_agents::{AgentConfig, AgentDefinition, ExecutionCandidate};
use phoenix_core::llm_language::LlmLanguage;
use serde_json::json;

#[tokio::test]
#[ignore = "opt-in synthetic inventory; set PHOENIX_TOOL_INVENTORY_OUTPUT to a JSON output path"]
async fn export_coordinator_tool_surface_inventory() {
    let output = std::env::var_os("PHOENIX_TOOL_INVENTORY_OUTPUT")
        .expect("set PHOENIX_TOOL_INVENTORY_OUTPUT to an output path");
    let (runtime, _, coordinator) = crate::conversation_action_tools::tests::native_runtime().await;
    let skill_dir = tempfile::tempdir().unwrap();
    phoenix_skills::builtin::extract_to(skill_dir.path()).unwrap();
    let catalog =
        phoenix_skills::AuthenticatedCoordinatorSkillCatalog::discover(Some(skill_dir.path()))
            .expect("authenticated builtin catalog");
    let routes = runtime.llm_registry().available_execution_routes();
    let route = routes
        .first()
        .expect("offline fixture has a configured route");
    let configured = AgentConfig {
        agents: vec![AgentDefinition {
            name: "inventory-worker".into(),
            description: "Synthetic inventory worker".into(),
            body: "Return bounded evidence.".into(),
            execution: None,
        }],
        tiers: std::collections::BTreeMap::from([(
            "inventory-tier".into(),
            vec![ExecutionCandidate {
                model: route.model.clone(),
                connection: route.connection.clone(),
                reasoning_effort: None,
            }],
        )]),
    };
    let model = runtime.llm_registry().default_model_id();
    let mut variants = Vec::new();
    for authenticated_catalog in [false, true] {
        let registry = runtime
            .coordinator_tool_registry(&coordinator, authenticated_catalog.then(|| catalog.clone()))
            .await
            .unwrap();
        for language in LlmLanguage::ALL {
            for (configuration, config) in [
                ("generic", AgentConfig::default()),
                ("synthetic-worker-and-tier", configured.clone()),
            ] {
                let mut definitions = registry.definitions_for_language(*language);
                let spawn = definitions
                    .iter_mut()
                    .find(|tool| tool.name == "spawn_agents")
                    .expect("production Coordinator has spawn_agents");
                *spawn = SpawnCatalog::resolve(&config, routes.clone()).tool_definition(
                    &model,
                    runtime.llm_registry().is_builtin_model(&model),
                    true,
                );
                let definition_metrics = definitions
                    .iter()
                    .map(|definition| {
                        let schema = serde_json::to_string(&definition.input_schema).unwrap();
                        json!({
                            "name": definition.name,
                            "description_bytes": definition.description.len(),
                            "description_words": definition.description.split_whitespace().count(),
                            "schema_bytes": schema.len(),
                            "schema_words": schema.split_whitespace().count(),
                        })
                    })
                    .collect::<Vec<_>>();
                variants.push(json!({
                    "language": language.as_str(),
                    "authenticated_catalog": authenticated_catalog,
                    "spawn_configuration": configuration,
                    "tool_count": definitions.len(),
                    "serialized_definitions_bytes": serde_json::to_vec(&definitions).unwrap().len(),
                    "metrics": definition_metrics,
                    "definitions": definitions,
                }));
            }
        }
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "scope": "synthetic offline runtime registry plus production SpawnCatalog resolution; not deployed configuration or provider token accounting",
            "variants": variants,
        }))
        .unwrap(),
    )
    .unwrap();
}
