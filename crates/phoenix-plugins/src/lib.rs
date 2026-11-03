//! Agent Plugins specification 1.1.0 loading (REQ-AP-001 through REQ-AP-012).
//!
//! This crate is the pure loading core: manifest validation with the
//! specification's report-and-ignore semantics, component discovery under
//! scoped failure boundaries, the stdio runtime environment contract
//! (placeholder expansion, env overlay ordering, cwd resolution), and
//! resolved-path containment. It performs no I/O beyond reading plugin files
//! and never writes, spawns, or connects anything — runtime wiring lives in
//! `phoenix-mcp` and `phoenix-skills` integration.
//!
//! The two canonical schema identifiers recognized (REQ-AP-012):
//!
//! - manifest: `https://agent-plugins.org/schemas/1.1.0/plugin.schema.json`
//! - MCP: `https://agent-plugins.org/schemas/1.1.0/mcp.schema.json`

pub mod manifest;
pub mod mcp_config;
pub mod resolve;

pub use manifest::Manifest;
pub use mcp_config::{McpDocument, ServerSpec, ServerValidation};
pub use resolve::{expand_placeholders, resolved_environment, within_boundary};

use std::path::{Path, PathBuf};

/// The default user-level plugins root: `~/.agents/plugins` (ADR-216).
/// Data directories live beside it as `~/.agents/plugins/data/<name>`.
#[must_use]
pub fn default_plugins_root(home: &Path) -> PathBuf {
    home.join(".agents/plugins")
}

/// The dedicated `PLUGIN_DATA` directory for a plugin, keyed by manifest
/// name alone so an update never moves or resets it (REQ-AP-006).
#[must_use]
pub fn data_dir_for(plugins_root: &Path, plugin_name: &str) -> PathBuf {
    plugins_root.join("data").join(plugin_name)
}

/// A parsed plugin package. Skill semantics and server execution eligibility
/// are validated by the skills and MCP adapters, respectively.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPlugin {
    pub manifest: Manifest,
    /// Directory entry through which the plugin was discovered.
    pub entry_dir: PathBuf,
    /// Resolved plugin root directory (symlinks followed).
    pub root: PathBuf,
    /// Contained skill-file candidates; frontmatter is not yet validated.
    pub skills: Vec<SkillCandidate>,
    /// Parsed server candidates; runtime paths and data access are not yet validated.
    pub servers: Vec<ServerSpec>,
    pub skipped_servers: Vec<mcp_config::ServerFailure>,
    pub skipped_skills: Vec<SkillFailure>,
    pub skills_disabled_reason: Option<String>,
    /// Why the MCP component type is absent despite a present location.
    pub mcp_disabled_reason: Option<String>,
}

/// A contained skill-file candidate, not a semantically validated skill.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillCandidate {
    /// Absolute directory path containing SKILL.md.
    pub dir: PathBuf,
    /// Absolute SKILL.md path.
    pub skill_md: PathBuf,
}

/// A skill rejected during filesystem discovery.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFailure {
    pub skill: String,
    pub reason: String,
}

impl SkillCandidate {
    /// Skill name for namespacing: the parent directory name, which the
    /// Agent Skills specification requires to match frontmatter `name`
    /// (REQ-AP-005).
    #[must_use]
    pub fn name(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

/// Scan a plugins root: each immediate child directory with a valid
/// manifest loads; invalid siblings produce diagnostics and continue
/// (REQ-AP-002). No recursion.
#[must_use]
pub fn scan_plugins_root(root: &Path) -> Vec<ParsedPlugin> {
    let mut plugins: Vec<ParsedPlugin> = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();

    let Ok(entries) = std::fs::read_dir(root) else {
        return plugins; // missing root: empty plugin set, no error (REQ-AP-002)
    };

    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    candidates.sort(); // deterministic: first-discovered wins is stable

    for candidate in candidates {
        // `data` holds PLUGIN_DATA directories, not plugins.
        if candidate.file_name().is_some_and(|n| n == "data") {
            continue;
        }
        match load_plugin(&candidate) {
            Ok(plugin) => {
                if seen_names.contains(&plugin.manifest.name) {
                    tracing::warn!(
                        name = %plugin.manifest.name,
                        path = %candidate.display(),
                        "duplicate plugin name; first discovered wins"
                    );
                    continue;
                }
                seen_names.push(plugin.manifest.name.clone());
                plugins.push(plugin);
            }
            Err(rejection) => {
                tracing::debug!(
                    path = %candidate.display(),
                    reason = %rejection,
                    "no valid manifest; skipping directory"
                );
            }
        }
    }
    plugins
}

/// Load one plugin root: validate the manifest, then load both component
/// types under the scoped failure boundaries (REQ-AP-003, REQ-AP-004).
///
/// # Errors
/// Returns a rejection when the manifest is missing or invalid — the only
/// condition that rejects the whole plugin.
pub fn load_plugin(root: &Path) -> Result<ParsedPlugin, String> {
    let resolved_root =
        std::fs::canonicalize(root).map_err(|e| format!("plugin root unreadable: {e}"))?;
    let manifest_path = resolved_root.join("plugin.json");
    if !within_boundary(&manifest_path, &resolved_root) {
        return Err("plugin.json escapes plugin root".into());
    }
    match std::fs::metadata(&manifest_path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Err("plugin.json is not a regular file".into()),
        Err(e) => return Err(format!("plugin.json unreadable: {e}")),
    }
    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("plugin.json unreadable: {e}"))?;
    let manifest = Manifest::parse(&content)?;

    let mut loaded = ParsedPlugin {
        manifest,
        entry_dir: root.to_path_buf(),
        root: resolved_root,
        skills: Vec::new(),
        servers: Vec::new(),
        skipped_servers: Vec::new(),
        skipped_skills: Vec::new(),
        skills_disabled_reason: None,
        mcp_disabled_reason: None,
    };

    load_skills(&mut loaded);
    load_mcp(&mut loaded);
    Ok(loaded)
}

/// Skills component: immediate children of `skills/` with a regular
/// `SKILL.md`, non-recursively (REQ-AP-004, REQ-AP-005). A present regular
/// file named `skills` invalidates the component type only.
fn load_skills(loaded: &mut ParsedPlugin) {
    let skills_path = loaded.root.join("skills");
    if !within_boundary(&skills_path, &loaded.root) {
        loaded.skills_disabled_reason = Some("skills location escapes plugin root".into());
        tracing::warn!(plugin = %loaded.manifest.name, "skills location escapes plugin root");
        return;
    }
    match std::fs::metadata(&skills_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            loaded.skills_disabled_reason = Some("skills location is not a directory".into());
            tracing::warn!(plugin = %loaded.manifest.name, "skills location has invalid filesystem kind");
            return;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            loaded.skills_disabled_reason = Some(format!("skills metadata unreadable: {error}"));
            tracing::warn!(plugin = %loaded.manifest.name, %error, "skills metadata unreadable");
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(&skills_path) else {
        loaded.skills_disabled_reason = Some("skills location unreadable".into());
        tracing::warn!(
            plugin = %loaded.manifest.name,
            "skills location unreadable; skipping"
        );
        return;
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    dirs.sort();

    for dir in dirs {
        let skill_md = dir.join("SKILL.md");
        if !skill_md.is_file() {
            continue; // no SKILL.md: not a skill; nested trees are ignored (REQ-AP-005)
        }
        let resolved = std::fs::canonicalize(&skill_md).unwrap_or_else(|_| skill_md.clone());
        if !within_boundary(&resolved, &loaded.root) {
            loaded.skipped_skills.push(SkillFailure {
                skill: dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                reason: "SKILL.md escapes plugin root".into(),
            });
            tracing::warn!(
                plugin = %loaded.manifest.name,
                skill = %dir.display(),
                "SKILL.md escapes plugin root; skipping skill"
            );
            continue;
        }
        loaded.skills.push(SkillCandidate {
            dir: dir.clone(),
            skill_md: resolved,
        });
    }
}

/// MCP component: `mcp.json`, disabled wholesale on document-level failure;
/// per-server failures skip that server only (REQ-AP-004, REQ-AP-012).
fn load_mcp(loaded: &mut ParsedPlugin) {
    let mcp_path = loaded.root.join("mcp.json");
    if !within_boundary(&mcp_path, &loaded.root) {
        let reason = "mcp.json escapes plugin root".to_string();
        tracing::warn!(plugin = %loaded.manifest.name, "{reason}");
        loaded.mcp_disabled_reason = Some(reason);
        return;
    }
    match std::fs::metadata(&mcp_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            loaded.mcp_disabled_reason = Some("mcp.json location is not a regular file".into());
            tracing::warn!(plugin = %loaded.manifest.name, "mcp.json has invalid filesystem kind");
            return;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            loaded.mcp_disabled_reason = Some(format!("mcp.json metadata unreadable: {error}"));
            tracing::warn!(plugin = %loaded.manifest.name, %error, "mcp.json metadata unreadable");
            return;
        }
    }

    let content = match std::fs::read_to_string(&mcp_path) {
        Ok(c) => c,
        Err(e) => {
            let reason = format!("mcp.json unreadable: {e}");
            tracing::warn!(plugin = %loaded.manifest.name, "{reason}");
            loaded.mcp_disabled_reason = Some(reason);
            return;
        }
    };

    let document = match McpDocument::parse(&content, crate::manifest::MANIFEST_SCHEMA_URL) {
        Ok(doc) => doc,
        Err(reason) => {
            tracing::warn!(plugin = %loaded.manifest.name, "{reason}");
            loaded.mcp_disabled_reason = Some(reason);
            return;
        }
    };

    for (_name, validation) in document.validate_servers() {
        match validation {
            ServerValidation::Valid(spec) => loaded.servers.push(spec),
            ServerValidation::Skipped(failure) => {
                tracing::warn!(
                    plugin = %loaded.manifest.name,
                    server = %failure.server,
                    reason = %failure.reason,
                    "skipping plugin MCP server"
                );
                loaded.skipped_servers.push(failure);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn present_fifo_components_report_wrong_kind_instead_of_absence() {
        for location in ["skills", "mcp.json"] {
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(
                temp.path().join("plugin.json"),
                serde_json::json!({
                    "$schema": manifest::MANIFEST_SCHEMA_URL, "name": "demo"
                })
                .to_string(),
            )
            .unwrap();
            assert!(std::process::Command::new("mkfifo")
                .arg(temp.path().join(location))
                .status()
                .unwrap()
                .success());
            let plugin = load_plugin(temp.path()).unwrap();
            if location == "skills" {
                assert!(plugin
                    .skills_disabled_reason
                    .unwrap()
                    .contains("not a directory"));
                assert!(plugin.mcp_disabled_reason.is_none());
            } else {
                assert!(plugin
                    .mcp_disabled_reason
                    .unwrap()
                    .contains("not a regular file"));
                assert!(plugin.skills_disabled_reason.is_none());
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn fifo_manifest_is_rejected_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("fifo-plugin");
        std::fs::create_dir(&pkg).unwrap();
        let manifest = pkg.join("plugin.json");
        std::process::Command::new("mkfifo")
            .arg(&manifest)
            .status()
            .unwrap();
        // load_plugin must reject via metadata check, never attempting read.
        let result = load_plugin(&pkg);
        assert!(result.is_err(), "FIFO manifest must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "rejection reason should mention regular file: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn fifo_manifest_skipped_in_scan_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Package with a FIFO manifest.
        let bad_pkg = root.join("fifo-pkg");
        std::fs::create_dir(&bad_pkg).unwrap();
        std::process::Command::new("mkfifo")
            .arg(bad_pkg.join("plugin.json"))
            .status()
            .unwrap();
        // Valid sibling.
        let good_pkg = root.join("good-pkg");
        std::fs::create_dir(&good_pkg).unwrap();
        std::fs::write(
            good_pkg.join("plugin.json"),
            serde_json::json!({
                "$schema": crate::manifest::MANIFEST_SCHEMA_URL,
                "name": "good",
                "version": "1.0.0",
                "description": "valid"
            })
            .to_string(),
        )
        .unwrap();

        let plugins = scan_plugins_root(root);
        assert_eq!(plugins.len(), 1, "only the valid package loads");
        assert_eq!(plugins[0].manifest.name, "good");
    }
}
