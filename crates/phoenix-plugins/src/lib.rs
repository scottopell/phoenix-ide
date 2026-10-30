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

pub mod filter;

pub use manifest::Manifest;
pub use mcp_config::{McpDocument, ServerSpec, ServerValidation};
pub use resolve::{expand_placeholders, resolved_environment, within_boundary};

use std::path::{Path, PathBuf};

/// The default user-level plugins root: `~/.agents/plugins` (ADR-191).
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

/// A loaded plugin: the validated manifest plus per-component outcomes.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedPlugin {
    pub manifest: Manifest,
    /// Resolved plugin root directory (symlinks followed).
    pub root: PathBuf,
    /// Skill directories that loaded (each contains a valid SKILL.md).
    pub skills: Vec<SkillDir>,
    /// Servers that validated, in document order.
    pub servers: Vec<ServerSpec>,
    /// Why the MCP component type is absent despite a present location.
    pub mcp_disabled_reason: Option<String>,
}

/// A loaded plugin skill: the directory and its SKILL.md file.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDir {
    /// Absolute directory path containing SKILL.md.
    pub dir: PathBuf,
    /// Absolute SKILL.md path.
    pub skill_md: PathBuf,
}

impl SkillDir {
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
pub fn scan_plugins_root(root: &Path) -> Vec<LoadedPlugin> {
    scan_plugins_root_filtered(root, &[])
}

/// [`scan_plugins_root`] restricted to enabled plugins: names in the
/// process-wide disabled filter are omitted entirely (REQ-AP-008).
#[must_use]
pub fn scan_enabled_plugins(root: &Path) -> Vec<LoadedPlugin> {
    let disabled: Vec<String> = filter::disabled_set().into_iter().collect();
    scan_plugins_root_filtered(root, &disabled)
}

/// The scan core: `disabled` names plugin roots omitted entirely.
#[must_use]
pub fn scan_plugins_root_filtered(root: &Path, disabled: &[String]) -> Vec<LoadedPlugin> {
    let mut plugins: Vec<LoadedPlugin> = Vec::new();
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
        if let Some(name) = plugin_name_if_valid(&candidate) {
            if disabled.iter().any(|d| d == &name) {
                tracing::debug!(
                    plugin = %name,
                    path = %candidate.display(),
                    "plugin disabled; contributing nothing"
                );
                continue;
            }
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

/// The manifest name of a candidate root, when its manifest is valid.
fn plugin_name_if_valid(root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(root.join("plugin.json")).ok()?;
    Manifest::parse(&content).ok().map(|m| m.name)
}

/// Load one plugin root: validate the manifest, then load both component
/// types under the scoped failure boundaries (REQ-AP-003, REQ-AP-004).
///
/// # Errors
/// Returns a rejection when the manifest is missing or invalid — the only
/// condition that rejects the whole plugin.
pub fn load_plugin(root: &Path) -> Result<LoadedPlugin, String> {
    let manifest_path = root.join("plugin.json");
    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("plugin.json unreadable: {e}"))?;
    let manifest = Manifest::parse(&content)?;

    // Resolve the real root so containment checks share one boundary.
    let resolved_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    let mut loaded = LoadedPlugin {
        manifest,
        root: resolved_root,
        skills: Vec::new(),
        servers: Vec::new(),
        mcp_disabled_reason: None,
    };

    load_skills(&mut loaded);
    load_mcp(&mut loaded);
    Ok(loaded)
}

/// Skills component: immediate children of `skills/` with a regular
/// `SKILL.md`, non-recursively (REQ-AP-004, REQ-AP-005). A present regular
/// file named `skills` invalidates the component type only.
fn load_skills(loaded: &mut LoadedPlugin) {
    let skills_path = loaded.root.join("skills");
    if skills_path.is_file() {
        tracing::warn!(
            plugin = %loaded.manifest.name,
            "skills location is a regular file; invalidating skills component"
        );
        return;
    }
    if !skills_path.is_dir() {
        return; // absent: valid
    }
    let Ok(entries) = std::fs::read_dir(&skills_path) else {
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
        // Containment: SKILL.md must resolve inside the plugin root
        // (REQ-AP-007).
        let resolved = std::fs::canonicalize(&skill_md).unwrap_or_else(|_| skill_md.clone());
        if !within_boundary(&resolved, &loaded.root) {
            tracing::warn!(
                plugin = %loaded.manifest.name,
                skill = %dir.display(),
                "SKILL.md escapes plugin root; skipping skill"
            );
            continue;
        }
        loaded.skills.push(SkillDir {
            dir: dir.clone(),
            skill_md: resolved,
        });
    }
}

/// MCP component: `mcp.json`, disabled wholesale on document-level failure;
/// per-server failures skip that server only (REQ-AP-004, REQ-AP-012).
fn load_mcp(loaded: &mut LoadedPlugin) {
    let mcp_path = loaded.root.join("mcp.json");
    if mcp_path.is_dir() {
        loaded.mcp_disabled_reason = Some("mcp.json location is a directory".to_string());
        tracing::warn!(
            plugin = %loaded.manifest.name,
            "mcp.json location is a directory; disabling plugin MCP"
        );
        return;
    }
    if !mcp_path.is_file() {
        return; // absent: valid
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
            }
        }
    }
}
