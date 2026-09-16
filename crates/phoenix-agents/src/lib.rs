//! User-configured named workers and execution tiers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use phoenix_core::domain::llm_types::ModelEffort;
pub use phoenix_core::domain::sm_state::ExecutionSelection;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCandidate {
    pub model: String,
    pub connection: String,
    pub reasoning_effort: Option<ModelEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub body: String,
    pub execution: Option<Vec<ExecutionCandidate>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub agents: Vec<AgentDefinition>,
    pub tiers: BTreeMap<String, Vec<ExecutionCandidate>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    version: u32,
    #[serde(default)]
    agents: BTreeMap<String, AgentEntry>,
    #[serde(default)]
    tiers: BTreeMap<String, TierEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentEntry {
    description: String,
    instructions: String,
    execution: Option<Vec<ExecutionCandidate>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TierEntry {
    execution: Vec<ExecutionCandidate>,
}

fn nonblank(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be blank"))
    } else {
        Ok(())
    }
}

fn validate_candidates(candidates: &[ExecutionCandidate], field: &str) -> Result<(), String> {
    if candidates.is_empty() {
        return Err(format!("{field} must contain at least one candidate"));
    }
    for (index, candidate) in candidates.iter().enumerate() {
        nonblank(&candidate.model, &format!("{field}[{index}].model"))?;
        nonblank(
            &candidate.connection,
            &format!("{field}[{index}].connection"),
        )?;
    }
    Ok(())
}

fn label(value: &str) -> String {
    value
        .chars()
        .take(80)
        .flat_map(char::escape_default)
        .collect()
}

/// Parse one complete config file; malformed definitions never become callable.
///
/// # Errors
/// Returns a bounded diagnostic for invalid TOML, unsupported versions, or invalid fields.
pub fn parse_config(content: &str) -> Result<AgentConfig, String> {
    let raw: ConfigFile = toml::from_str(content).map_err(|error: toml::de::Error| {
        let detail: String = error.message().chars().take(240).collect();
        format!("Invalid Phoenix config: {detail}")
    })?;
    if raw.version != 1 {
        return Err(format!(
            "Unsupported Phoenix config version {}; expected 1",
            raw.version
        ));
    }
    let mut agents = Vec::with_capacity(raw.agents.len());
    for (name, entry) in raw.agents {
        nonblank(&name, "Agent name")?;
        let field = format!("agents.{}", label(&name));
        nonblank(&entry.description, &format!("{field}.description"))?;
        nonblank(&entry.instructions, &format!("{field}.instructions"))?;
        if let Some(candidates) = &entry.execution {
            validate_candidates(candidates, &format!("{field}.execution"))?;
        }
        agents.push(AgentDefinition {
            name,
            description: entry.description,
            body: entry.instructions,
            execution: entry.execution,
        });
    }
    let mut tiers = BTreeMap::new();
    for (name, entry) in raw.tiers {
        nonblank(&name, "Tier name")?;
        validate_candidates(
            &entry.execution,
            &format!("tiers.{}.execution", label(&name)),
        )?;
        tiers.insert(name, entry.execution);
    }
    Ok(AgentConfig { agents, tiers })
}

/// Read exactly the supplied config file. A missing file means no named workers or tiers.
///
/// # Errors
/// Returns an error for unreadable files or invalid configuration.
pub fn load_config(path: &Path) -> Result<AgentConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_config(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AgentConfig::default()),
        Err(error) => Err(format!("Cannot read Phoenix config: {}", error.kind())),
    }
}

/// Resolve the XDG location, falling back to `$HOME/.config`.
///
/// # Errors
/// Returns an error when the configured location is not absolute.
pub fn config_path() -> Result<PathBuf, String> {
    let environment = phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect();
    config_path_from(
        std::env::var_os("XDG_CONFIG_HOME")
            .as_deref()
            .map(Path::new),
        environment.home(),
    )
}

fn config_path_from(xdg: Option<&Path>, home: &Path) -> Result<PathBuf, String> {
    let root = xdg
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(|| home.join(".config"), Path::to_path_buf);
    if !root.is_absolute() {
        return Err("Phoenix config directory must be absolute".to_string());
    }
    Ok(root.join("phoenix-ide/config.toml"))
}

/// Load the user configuration; no project or legacy agent files are consulted.
///
/// # Errors
/// Returns a config-location, file-read, or parse diagnostic.
pub fn load_user_config() -> Result<AgentConfig, String> {
    load_config(&config_path()?)
}

#[must_use]
pub fn find_agent<'a>(agents: &'a [AgentDefinition], name: &str) -> Option<&'a AgentDefinition> {
    agents.iter().find(|agent| agent.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_selection_cannot_mix_tier_and_model_fields() {
        let tier: ExecutionSelection = toml::from_str("type='tier'\nname='fast'").unwrap();
        assert_eq!(
            tier,
            ExecutionSelection::Tier {
                name: "fast".into()
            }
        );
        let model: ExecutionSelection =
            toml::from_str("type='model'\nmodel='gpt-5.6-sol'\nconnection='codex'").unwrap();
        assert_eq!(
            model,
            ExecutionSelection::Model {
                model: "gpt-5.6-sol".into(),
                connection: "codex".into(),
                reasoning_effort: None,
            }
        );
        for invalid in [
            "type='tier'\nname='fast'\nmodel='gpt-5.6-sol'",
            "type='tier'\nname='fast'\nreasoning_effort='high'",
            "type='model'\nmodel='gpt-5.6-sol'",
            "type='default'",
        ] {
            assert!(toml::from_str::<ExecutionSelection>(invalid).is_err());
        }
    }

    #[test]
    fn parses_ordered_atomic_candidates_and_inline_instructions() {
        let config = parse_config(
            r#"
version = 1
[agents.reviewer]
description = "Reviews changes"
instructions = "Read carefully.\nReport findings."
execution = [
  { model = "sonnet", connection = "anthropic", reasoning_effort = "high" },
  { model = "gpt-5.6-sol", connection = "codex", reasoning_effort = "medium" }
]
[tiers.fast]
execution = [{model = "gpt-5.6-luna", connection = "codex"}]
"#,
        )
        .unwrap();
        let agent = find_agent(&config.agents, "reviewer").unwrap();
        assert_eq!(agent.body, "Read carefully.\nReport findings.");
        let candidates = agent.execution.as_ref().unwrap();
        assert_eq!(candidates[0].model, "sonnet");
        assert_eq!(candidates[0].connection, "anthropic");
        assert_eq!(candidates[0].reasoning_effort, Some(ModelEffort::High));
        assert_eq!(candidates[1].model, "gpt-5.6-sol");
        assert_eq!(config.tiers["fast"][0].reasoning_effort, None);
    }

    #[test]
    fn agents_are_sorted_and_omitted_execution_is_preserved() {
        let config = parse_config("version = 1\n[agents.z]\ndescription='z'\ninstructions='z'\n[agents.a]\ndescription='a'\ninstructions='a'").unwrap();
        assert_eq!(
            config
                .agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert!(config.agents.iter().all(|a| a.execution.is_none()));
        assert!(find_agent(&config.agents, "missing").is_none());
    }

    #[test]
    fn rejects_invalid_and_legacy_fields() {
        for source in [
            "",
            "version=2",
            "version=1\nunknown=true",
            "version=1\n[agents.a]\ndescription='d'\ninstructions='i'\nmode='work'",
            "version=1\n[agents.a]\ndescription='d'\ninstructions='i'\ntools=['bash']",
            "version=1\n[agents.a]\ndescription='d'\ninstructions='i'\nmodel='opus'",
            "version=1\n[agents.a]\ndescription='d'",
            "version=1\n[agents.a]\ndescription=' '\ninstructions='i'",
            "version=1\n[agents.a]\ndescription='d'\ninstructions=' '\n",
            "version=1\n[agents.a]\ndescription='d'\ninstructions='i'\nexecution=[]",
            "version=1\n[agents.' ']\ndescription='d'\ninstructions='i'",
            "version=1\n[tiers.fast]\nexecution=[]",
            "version=1\n[tiers.fast]\nexecution=[{model='x',connection=' '}]",
            "version=1\n[tiers.fast]\nexecution=[{model=' ',connection='codex'}]",
            "version=1\n[tiers.fast]\nexecution=[{model='x',connection='codex',reasoning_effort='invalid'}]",
            "version=1\n[tiers.fast]\nexecution=[{model='x',connection='codex',typo=true}]",
            "version=1\n[tiers.fast]\nexecution=[{model='x'}]",
        ] {
            assert!(parse_config(source).is_err(), "accepted invalid config: {source}");
        }
    }

    #[test]
    fn missing_config_does_not_load_legacy_files() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join(".claude/agents");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("reviewer.md"),
            "---\nname: reviewer\ndescription: old\nmodel: opus\n---\nReview.",
        )
        .unwrap();
        assert_eq!(
            load_config(&temp.path().join("config.toml")).unwrap(),
            AgentConfig::default()
        );
        assert!(load_config(temp.path()).is_err());
    }

    #[test]
    fn config_location_uses_only_xdg_or_home() {
        assert_eq!(
            config_path_from(Some(Path::new("/xdg")), Path::new("/home/u")).unwrap(),
            Path::new("/xdg/phoenix-ide/config.toml")
        );
        assert_eq!(
            config_path_from(None, Path::new("/home/u")).unwrap(),
            Path::new("/home/u/.config/phoenix-ide/config.toml")
        );
        assert_eq!(
            config_path_from(Some(Path::new("")), Path::new("/home/u")).unwrap(),
            Path::new("/home/u/.config/phoenix-ide/config.toml")
        );
        assert!(config_path_from(Some(Path::new("relative")), Path::new("/home/u")).is_err());
    }

    #[test]
    fn diagnostic_is_bounded_and_does_not_echo_source_document() {
        let source = format!(
            "version=1\n{}=true\n# PRIVATE INSTRUCTIONS",
            "x".repeat(20_000)
        );
        let error = parse_config(&source).unwrap_err();
        assert!(error.len() < 1_000);
        assert!(!error.contains("PRIVATE INSTRUCTIONS"));
    }
}
