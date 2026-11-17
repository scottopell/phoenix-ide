use super::*;
use crate::{EffortCapabilities, LlmConfig, ModelBackend, ModelRegistry};

const MODEL: &str = r#"
[[models]]
id = "local-open-weight"
api_name = "gateway/open-weight"
backend = "openai_chat_completions"
family = "Open weight"
description = "A configured coding model"
context_window = 1048576
max_output_tokens = 32768
recommended = false
supports_tool_search = false
[models.effort_capabilities]
support = "supported"
levels = ["low", "high"]
native_default = "low"
"#;

fn config() -> String {
    format!("version = 1\n{MODEL}")
}

#[test]
fn toml_models_register_with_the_declared_metadata_and_request_name() {
    let models = parse_models(&config()).unwrap().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].backend, ModelBackend::OpenAIChatCompletions);
    assert_eq!(
        models[0].default_request_name.as_str(),
        "gateway/open-weight"
    );
    assert_eq!(models[0].context_window, 1_048_576);
    assert_eq!(models[0].max_output_tokens, 32_768);
    assert!(matches!(
        models[0].effort_capabilities,
        EffortCapabilities::Supported(_)
    ));
    let registry = ModelRegistry::new(&LlmConfig {
        external_models: models,
        openai_api_key: Some("test-only-not-a-credential".into()),
        ..LlmConfig::default()
    });
    assert!(registry.get("local-open-weight").is_some());
}

#[test]
fn model_and_worker_readers_accept_each_others_sections() {
    let content = format!(
        "{}\n[agents.reviewer]\ndescription='Review'\ninstructions='Find defects'\n\
         [tiers.fast]\nexecution=[{{model='local-open-weight',connection='openai_chat_completions'}}]",
        config()
    );
    let workers = phoenix_agents::parse_config(&content).unwrap();
    assert_eq!(workers.agents[0].name, "reviewer");
    assert_eq!(workers.tiers["fast"][0].model, "local-open-weight");
    assert_eq!(
        parse_models(&content).unwrap().unwrap()[0].id,
        "local-open-weight"
    );
}

#[test]
fn missing_file_and_worker_only_config_allow_environment_models() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    assert!(load_external_models(&path, None).unwrap().is_empty());
    let raw = r#"[{"id":"env-model","backend":"anthropic","description":"From env","context_window":1000,"max_output_tokens":100,"recommended":false,"supports_tool_search":false}]"#;
    assert_eq!(
        load_external_models(&path, Some(raw)).unwrap()[0].id,
        "env-model"
    );
    std::fs::write(
        &path,
        "version=1\n[agents.a]\ndescription='d'\ninstructions='i'",
    )
    .unwrap();
    assert_eq!(
        load_external_models(&path, Some(raw)).unwrap()[0].id,
        "env-model"
    );
}

#[test]
fn explicit_models_even_empty_reject_a_second_configuration_source() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    for content in [config(), "version=1\nmodels=[]".into()] {
        std::fs::write(&path, content).unwrap();
        let error = load_external_models(&path, Some("[]")).unwrap_err();
        assert!(error.contains("PHOENIX_LLM_MODELS"));
        assert!(error.contains(path.to_str().unwrap()));
        assert!(load_external_models(&path, Some("  ")).is_ok());
    }
}

#[test]
fn invalid_toml_entries_reject_the_whole_section() {
    let valid = config();
    for invalid in [
        valid.replace("version = 1", "version = 2"),
        valid.replace("max_output_tokens = 32768", "max_output_tokens = 0"),
        valid.replace("max_output_tokens = 32768", "max_output_tokens = 1048576"),
        valid.replace("max_output_tokens = 32768", ""),
        valid.replace("recommended = false", "recomended = false"),
        valid.replace(
            "backend = \"openai_chat_completions\"",
            "backend = \"mock\"",
        ),
        valid.replace("levels = [\"low\", \"high\"]", "levels = []"),
        valid.replace("native_default = \"low\"", "native_default = \"medium\""),
        valid.replace(
            "levels = [\"low\", \"high\"]",
            "levels = [\"low\", \"low\"]",
        ),
        format!("{valid}\n{MODEL}"),
        format!("{valid}\n[[models]]\nid='broken'"),
        "version=1\nmodels='not an array'".into(),
        "version=1\nmodles=[]".into(),
    ] {
        assert!(
            parse_models(&invalid).is_err(),
            "Accepted invalid configuration: {invalid}"
        );
    }
}

#[test]
fn unit_effort_forms_reject_extra_metadata() {
    let base = config();
    let base = base.split("[models.effort_capabilities]").next().unwrap();
    for support in ["unknown", "unsupported"] {
        let valid = format!("{base}[models.effort_capabilities]\nsupport='{support}'\n");
        assert!(parse_models(&valid).is_ok());
        for extra in [
            "unknown_field=true",
            "levels=['high']",
            "native_default='high'",
        ] {
            let invalid = format!("{valid}{extra}\n");
            assert!(
                parse_models(&invalid).is_err(),
                "accepted {support} with {extra}"
            );
        }
    }
}

#[test]
fn fresh_load_reads_edits_without_reusing_a_cached_model_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, config()).unwrap();
    let first = load_external_models(&path, None).unwrap();
    std::fs::write(
        &path,
        config().replace("local-open-weight", "new-open-weight"),
    )
    .unwrap();
    let second = load_external_models(&path, None).unwrap();
    assert_eq!(first[0].id, "local-open-weight");
    assert_eq!(second[0].id, "new-open-weight");
}

#[test]
fn invalid_or_unreadable_file_reports_its_location_without_env_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "version = [\n# PRIVATE INSTRUCTIONS").unwrap();
    let error = load_external_models(&path, Some("[]")).unwrap_err();
    assert!(error.contains(path.to_str().unwrap()));
    assert!(!error.contains("PRIVATE INSTRUCTIONS"));
    assert!(load_external_models(temp.path(), None)
        .unwrap_err()
        .contains("Cannot read"));
}

#[test]
fn toml_cannot_override_a_builtin_model() {
    let builtin = crate::all_models().remove(0);
    let content = config().replace("local-open-weight", &builtin.id);
    let merged = crate::merge_model_specs(
        crate::all_models(),
        &parse_models(&content).unwrap().unwrap(),
    );
    let kept = merged.iter().find(|model| model.id == builtin.id).unwrap();
    assert_eq!(kept.source, crate::ModelSource::BuiltIn);
    assert_eq!(kept.default_request_name, builtin.default_request_name);
}
