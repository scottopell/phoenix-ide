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

#[test]
fn startup_llm_toml_probe() {
    let Ok(_) = std::env::var("PHOENIX_TEST_LLM_TOML_PROBE") else {
        return;
    };
    let config = LlmConfig::from_env(Arc::new(PhoenixRuntimeEnvironment::detect()));
    assert!(config.credential_helper.is_some());
    assert_eq!(config.auth_style, phoenix_llm::AuthStyle::PlainBearer);
    assert_eq!(
        config.openai_chat_completions_base_url.as_deref(),
        Some("https://gateway.example.test/v1/chat/completions")
    );
    assert_eq!(
        config.default_model.as_deref(),
        std::env::var("EXPECTED_DEFAULT_MODEL").ok().as_deref()
    );
    assert_eq!(
        config.custom_headers,
        vec![("X-Gateway".to_string(), "phoenix".to_string())]
    );
    assert_eq!(
        config.request_tags.get("team").map(String::as_str),
        Some("ide")
    );
    let registry = ModelRegistry::new(&config);
    assert!(registry.get("google/gemini-4-neon").is_some());
}

#[test]
fn llm_toml_populates_from_env_and_env_overrides_per_field() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("phoenix-ide");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    std::fs::write(
        &path,
        r#"
version = 1

[llm]
api_key_helper = "echo test-token"
auth_header = "bearer"
default_model = "google/gemini-4-neon"
openai_chat_completions_base_url = "https://gateway.example.test/v1/chat/completions"

[llm.custom_headers]
X-Gateway = "phoenix"

[llm.request_tags]
team = "ide"
"#,
    )
    .unwrap();

    // Pure TOML config (simulates Finder-launched Phoenix.app with no shell env)
    let toml_only = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "startup_llm_toml_probe", "--nocapture"])
        .env_clear()
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path())
        .env("PHOENIX_TEST_LLM_TOML_PROBE", "1")
        .env("EXPECTED_DEFAULT_MODEL", "google/gemini-4-neon")
        .output()
        .unwrap();
    assert!(
        toml_only.status.success(),
        "{}",
        String::from_utf8_lossy(&toml_only.stderr)
    );

    // Env overrides DEFAULT_MODEL while inheriting helper, URL, headers, tags from TOML
    let env_override = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "startup_llm_toml_probe", "--nocapture"])
        .env_clear()
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path())
        .env("PHOENIX_TEST_LLM_TOML_PROBE", "1")
        .env("DEFAULT_MODEL", "claude-sonnet-4-6")
        .env("EXPECTED_DEFAULT_MODEL", "claude-sonnet-4-6")
        .output()
        .unwrap();
    assert!(
        env_override.status.success(),
        "{}",
        String::from_utf8_lossy(&env_override.stderr)
    );
}
