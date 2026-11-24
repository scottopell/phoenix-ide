use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_llm::{LlmConfig, ModelRegistry};

fn write_model(path: &Path, id: &str) {
    std::fs::write(
        path,
        format!(
            "version=1\n[[models]]\nid='{id}'\nbackend='openai_chat_completions'\n\
             description='Startup test model'\ncontext_window=1000\nmax_output_tokens=100\n\
             recommended=false\nsupports_tool_search=false\n"
        ),
    )
    .unwrap();
}

fn startup(root: &Path, environment_models: Option<&str>) -> Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "startup_probe", "--nocapture"])
        .env_clear()
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env("PHOENIX_TEST_MODEL_CONFIG_PROBE", "1")
        .env("OPENAI_API_KEY", "test-only-not-a-credential");
    if let Some(models) = environment_models {
        command.env("PHOENIX_LLM_MODELS", models);
    }
    command.output().unwrap()
}

#[test]
fn startup_probe() {
    let Ok(_) = std::env::var("PHOENIX_TEST_MODEL_CONFIG_PROBE") else {
        return;
    };
    let config = LlmConfig::from_env(Arc::new(PhoenixRuntimeEnvironment::detect()));
    assert_eq!(config.external_models.len(), 1);
    let model = &config.external_models[0].id;
    assert!(ModelRegistry::new(&config).get(model).is_some());
    println!("REGISTERED_MODEL={model}");
}

#[test]
fn restart_loads_edited_toml_with_the_same_process_environment() {
    // REQ-LLM-003c: registry construction, not only the file parser, reads edits.
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("phoenix-ide");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    write_model(&path, "first-model");
    let first = startup(root.path(), None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("REGISTERED_MODEL=first-model"));
    write_model(&path, "second-model");
    let second = startup(root.path(), None);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(String::from_utf8_lossy(&second.stdout).contains("REGISTERED_MODEL=second-model"));

    let conflict = startup(root.path(), Some("[]"));
    assert!(!conflict.status.success());
    let error = String::from_utf8_lossy(&conflict.stderr);
    assert!(error.contains("PHOENIX_LLM_MODELS"));
    assert!(error.contains(path.to_str().unwrap()));
}
