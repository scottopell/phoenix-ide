use super::agent_execution::SpawnCatalog;
use phoenix_agents::{AgentConfig, AgentDefinition, ExecutionCandidate};
use phoenix_core::llm_language::LlmLanguage;
use serde_json::json;

async fn coordinator_tool_surface_variants() -> Vec<serde_json::Value> {
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
    variants
}

#[tokio::test]
async fn assembled_coordinator_inventory_preserves_catalog_and_spawn_specialization() {
    let variants = coordinator_tool_surface_variants().await;
    assert_eq!(variants.len(), LlmLanguage::ALL.len() * 4);
    for variant in &variants {
        let definitions = variant["definitions"].as_array().unwrap();
        let names = definitions
            .iter()
            .map(|definition| definition["name"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), definitions.len(), "duplicate installed tool");
        assert_eq!(
            names.contains("skill"),
            variant["authenticated_catalog"].as_bool().unwrap()
        );
        for definition in definitions {
            assert_eq!(definition["input_schema"]["type"], "object", "{definition}");
            for keyword in ["oneOf", "anyOf", "allOf"] {
                assert!(
                    definition["input_schema"].get(keyword).is_none(),
                    "{definition}"
                );
            }
            assert!(definition["defer_loading"].is_boolean());
        }
        let spawn = definitions
            .iter()
            .find(|definition| definition["name"] == "spawn_agents")
            .unwrap();
        let properties = &spawn["input_schema"]["properties"]["tasks"]["items"]["properties"];
        if variant["spawn_configuration"] == "generic" {
            assert!(properties.get("agent_type").is_none());
        } else {
            assert_eq!(
                properties["agent_type"]["enum"],
                json!(["inventory-worker"])
            );
            assert!(properties["execution"]
                .to_string()
                .contains("inventory-tier"));
        }
        assert!(names.contains("create_conversation"));
        let watch = definitions
            .iter()
            .find(|definition| definition["name"] == "watch_conversation")
            .unwrap();
        let description = watch["description"].as_str().unwrap();
        assert!(description.contains("question or task-approval waits"));
        assert!(description.contains("selected-peer watches report terminal facts only"));
        assert!(description.contains("supported Goal metadata changes"));
    }
}

#[tokio::test]
#[ignore = "opt-in synthetic inventory; set PHOENIX_TOOL_INVENTORY_OUTPUT to a JSON output path"]
async fn export_coordinator_tool_surface_inventory() {
    let output = std::env::var_os("PHOENIX_TOOL_INVENTORY_OUTPUT")
        .expect("set PHOENIX_TOOL_INVENTORY_OUTPUT to an output path");
    let variants = coordinator_tool_surface_variants().await;
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
