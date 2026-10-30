//! The stdio runtime environment contract (REQ-AP-006) and resolved-path
//! containment (REQ-AP-007).
//!
//! Pure functions only: given a validated `ServerSpec::Stdio`, the plugin
//! root, and the data directory, produce the fully resolved spawn
//! configuration — or the reason the server must be skipped.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::mcp_config::ServerSpec;

/// Single-pass `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` replacement (REQ-AP-006).
/// Exactly the two exact placeholders, one textual pass, no recursion:
/// replacement text is never rescanned, and other placeholder-like text
/// stays literal.
#[must_use]
pub fn expand_placeholders(text: &str, plugin_root: &Path, data_dir: &Path) -> String {
    text.replace("${PLUGIN_ROOT}", &plugin_root.to_string_lossy())
        .replace("${PLUGIN_DATA}", &data_dir.to_string_lossy())
}

/// The fully resolved stdio spawn configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStdio {
    pub command: String,
    pub args: Vec<String>,
    /// The complete environment: base, configured overlay, client-owned
    /// `PLUGIN_ROOT`/`PLUGIN_DATA` set last.
    pub env: BTreeMap<String, String>,
    /// Absolute working directory, guaranteed inside its boundary.
    pub cwd: PathBuf,
}

/// Resolve one stdio server against its plugin root and data directory
/// (REQ-AP-006, REQ-AP-007).
///
/// # Errors
/// Returns the skip reason when the expanded `cwd` escapes its anchor
/// boundary (e.g. `./`-relative with `..`, or a `${PLUGIN_ROOT}` path that
/// resolves outside the root).
pub fn resolved_environment(
    spec: &ServerSpec,
    plugin_root: &Path,
    data_dir: &Path,
    base_env: &BTreeMap<String, String>,
) -> Result<ResolvedStdio, String> {
    let crate::mcp_config::ServerSpec::Stdio {
        server,
        command,
        args,
        env: configured,
        cwd,
    } = spec
    else {
        return Err(format!(
            "{server} is not a stdio server",
            server = spec.server()
        ));
    };

    // Expansion applies to each args element, each env value, and cwd —
    // never to command or env keys.
    let expanded_args: Vec<String> = args
        .iter()
        .map(|arg| expand_placeholders(arg, plugin_root, data_dir))
        .collect();
    let mut expanded_env: BTreeMap<String, String> = base_env.clone();
    for (key, value) in configured {
        // Overlay replaces equivalent names under platform naming rules;
        // BTreeMap keys are the canonical form here.
        expanded_env.insert(
            key.clone(),
            expand_placeholders(value, plugin_root, data_dir),
        );
    }
    // Client-owned variables are set last and override any equivalent
    // names (validation already rejected configured PLUGIN_* declarations).
    expanded_env.insert(
        "PLUGIN_ROOT".to_string(),
        plugin_root.to_string_lossy().into_owned(),
    );
    expanded_env.insert(
        "PLUGIN_DATA".to_string(),
        data_dir.to_string_lossy().into_owned(),
    );

    // Working directory: default plugin root; explicit values must resolve
    // inside their anchor boundary (REQ-AP-006, REQ-AP-007).
    let resolved_cwd = match cwd {
        None => plugin_root.to_path_buf(),
        Some(value) => {
            let expanded = expand_placeholders(value, plugin_root, data_dir);
            let candidate: PathBuf =
                if value == "${PLUGIN_ROOT}" || value.starts_with("${PLUGIN_ROOT}/") {
                    plugin_root.join(trim_anchor(&expanded, plugin_root))
                } else if value == "${PLUGIN_DATA}" || value.starts_with("${PLUGIN_DATA}/") {
                    data_dir.join(trim_anchor(&expanded, data_dir))
                } else {
                    // `./`-relative: joined onto the plugin root.
                    plugin_root.join(expanded.strip_prefix("./").unwrap_or(expanded.as_str()))
                };
            if !within_boundary(&candidate, plugin_root) && !within_boundary(&candidate, data_dir) {
                return Err(format!(
                    "{server}: cwd {value} resolves outside its anchor boundary"
                ));
            }
            candidate
        }
    };

    Ok(ResolvedStdio {
        command: command.clone(),
        args: expanded_args,
        env: expanded_env,
        cwd: resolved_cwd,
    })
}

/// Strip the anchor prefix from an already-expanded placeholder path so the
/// remainder can be re-joined onto the resolved anchor (the expansion used
/// the same string, so this is a pure prefix trim).
fn trim_anchor<'a>(expanded: &'a str, anchor: &Path) -> &'a str {
    let anchor_str = anchor.to_string_lossy();
    expanded
        .strip_prefix(anchor_str.as_ref())
        .unwrap_or(expanded)
}

/// Resolved-path containment (REQ-AP-007): `path` must stay inside
/// `boundary` after resolving symlinks. Links inside the boundary are
/// allowed; links escaping it are rejected. Lexical `..` above the anchor
/// also escapes.
///
/// Both sides use the same resolution mode: when either fails to
/// canonicalize (not yet on disk), both fall back to lexical
/// normalization, so a symlinked temp root can never make a plain
/// inside-path comparison spuriously fail.
#[must_use]
pub fn within_boundary(path: &Path, boundary: &Path) -> bool {
    if let (Ok(resolved_path), Ok(resolved_boundary)) =
        (std::fs::canonicalize(path), std::fs::canonicalize(boundary))
    {
        resolved_path.starts_with(&resolved_boundary)
    } else {
        let resolved_path = normalize_lexically(path);
        let resolved_boundary = normalize_lexically(boundary);
        resolved_path.starts_with(&resolved_boundary)
    }
}

/// Lexical normalization when canonicalize fails (path not yet on disk):
/// resolve `.` and `..` components without touching the filesystem.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                match out.last() {
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    _ => out.push(component), // leading .. or above root: keep
                }
            }
            other @ (Component::Prefix(_) | Component::RootDir | Component::Normal(_)) => {
                out.push(other);
            }
        }
    }
    out.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_env() -> BTreeMap<String, String> {
        BTreeMap::from([("PATH".to_string(), "/usr/bin".to_string())])
    }

    fn stdio_spec(args: Vec<&str>, env: &[(&str, &str)], cwd: Option<&str>) -> ServerSpec {
        ServerSpec::Stdio {
            server: "database".to_string(),
            command: "npx".to_string(),
            args: args.into_iter().map(String::from).collect(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            cwd: cwd.map(String::from),
        }
    }

    #[test]
    fn expands_the_specification_environment_example() {
        let spec = stdio_spec(
            vec!["--config", "${PLUGIN_ROOT}/config/db.json"],
            &[("DATA_DIR", "${PLUGIN_DATA}/database")],
            Some("${PLUGIN_ROOT}"),
        );
        let root = Path::new("/plugins/database-plugin");
        let data = Path::new("/plugins/data/database-plugin");
        let resolved = resolved_environment(&spec, root, data, &base_env()).unwrap();
        assert_eq!(
            resolved.args,
            vec![
                "--config".to_string(),
                "/plugins/database-plugin/config/db.json".to_string(),
            ]
        );
        assert_eq!(
            resolved.env.get("DATA_DIR").map(String::as_str),
            Some("/plugins/data/database-plugin/database")
        );
        assert_eq!(resolved.cwd, root);
        assert_eq!(
            resolved.env.get("PLUGIN_ROOT").map(String::as_str),
            Some("/plugins/database-plugin")
        );
        assert_eq!(
            resolved.env.get("PLUGIN_DATA").map(String::as_str),
            Some("/plugins/data/database-plugin")
        );
    }

    #[test]
    fn expansion_is_single_pass_and_not_recursive() {
        let expanded = expand_placeholders(
            "${PLUGIN_ROOT}/x${PLUGIN_DATA}",
            Path::new("/r"),
            Path::new("/d"),
        );
        assert_eq!(expanded, "/r/x/d");
        // Replacement text is never rescanned: a data dir that itself
        // contains the placeholder spelling stays literal.
        let tricky = expand_placeholders(
            "${PLUGIN_DATA}",
            Path::new("/r"),
            Path::new("/d/${PLUGIN_ROOT}"),
        );
        assert_eq!(tricky, "/d/${PLUGIN_ROOT}");
    }

    #[test]
    fn client_owned_variables_override_last() {
        let spec = stdio_spec(vec![], &[("OTHER", "v")], None);
        let mut hostile_base = base_env();
        hostile_base.insert("PLUGIN_ROOT".to_string(), "/spoofed".to_string());
        let resolved = resolved_environment(
            &spec,
            Path::new("/real/root"),
            Path::new("/real/data"),
            &hostile_base,
        )
        .unwrap();
        assert_eq!(
            resolved.env.get("PLUGIN_ROOT").map(String::as_str),
            Some("/real/root")
        );
    }

    #[test]
    fn default_cwd_is_the_plugin_root() {
        let spec = stdio_spec(vec![], &[], None);
        let resolved =
            resolved_environment(&spec, Path::new("/root"), Path::new("/data"), &base_env())
                .unwrap();
        assert_eq!(resolved.cwd, Path::new("/root"));
    }

    #[test]
    fn anchored_cwd_must_resolve_inside_its_boundary() {
        let data_escape = stdio_spec(vec![], &[], Some("${PLUGIN_DATA}/../.."));
        assert!(resolved_environment(
            &data_escape,
            Path::new("/root"),
            Path::new("/root/data/p"),
            &base_env(),
        )
        .is_err());

        let root_escape = stdio_spec(vec![], &[], Some("./../../etc"));
        assert!(resolved_environment(
            &root_escape,
            Path::new("/root"),
            Path::new("/root/data/p"),
            &base_env(),
        )
        .is_err());

        let ok = stdio_spec(vec![], &[], Some("./work"));
        assert!(resolved_environment(
            &ok,
            Path::new("/root"),
            Path::new("/root/data/p"),
            &base_env(),
        )
        .is_ok());
    }

    #[test]
    fn boundary_permits_inside_links_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugin");
        std::fs::create_dir_all(root.join("inside")).unwrap();

        // Inside link: allowed.
        let inside_link = root.join("link-to-inside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("inside"), &inside_link).unwrap();
        assert!(within_boundary(&inside_link, &root));

        // Escaping link: rejected.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let escape_link = root.join("escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &escape_link).unwrap();
        assert!(!within_boundary(&escape_link, &root));

        // Lexical escape without any symlink: rejected.
        assert!(!within_boundary(&root.join("../../etc"), &root));

        // Plain containment: allowed.
        assert!(within_boundary(&root.join("skills/deploy/SKILL.md"), &root));
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    use crate::scan_plugins_root;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    const MANIFEST: &str = "{\"$schema\": \"https://agent-plugins.org/schemas/1.1.0/plugin.schema.json\", \"name\": \"devtools\"}";

    /// The full scan path on a real plugin tree: manifest, skills, and
    /// mcp.json load; the data directory is not a plugin candidate; a
    /// broken sibling is skipped without failing the scan.
    #[test]
    fn scans_a_real_plugins_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");

        write(&root.join("devtools/plugin.json"), MANIFEST);
        write(
            &root.join("devtools/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploys.\n---\nBody.",
        );
        write(
            &root.join("devtools/skills/nested/hidden/SKILL.md"),
            "---\nname: hidden\ndescription: Nested.\n---\nBody.",
        );
        write(
            &root.join("devtools/mcp.json"),
            "{\"$schema\": \"https://agent-plugins.org/schemas/1.1.0/mcp.schema.json\", \"mcpServers\": {\"validator\": {\"type\": \"stdio\", \"command\": \"./bin/validator\"}}}",
        );

        // A broken sibling: no manifest, still scanned past.
        write(&root.join("not-a-plugin/readme.txt"), "hello");

        // Data directories are not plugin candidates.
        write(&root.join("data/devtools/state.db"), "x");

        let plugins = scan_plugins_root(&root);
        assert_eq!(plugins.len(), 1, "broken sibling skipped, data ignored");
        let plugin = &plugins[0];
        assert_eq!(plugin.manifest.name, "devtools");
        assert_eq!(plugin.skills.len(), 1, "nested skill tree is not recursive");
        assert_eq!(plugin.skills[0].name(), "deploy");
        assert_eq!(plugin.servers.len(), 1);
        assert_eq!(plugin.servers[0].server(), "validator");
        assert!(plugin.mcp_disabled_reason.is_none());
    }

    /// Failure boundary matrix on a real tree: each condition invalidates
    /// only its own scope (REQ-AP-004).
    #[test]
    fn failure_boundaries_are_scoped_on_a_real_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");

        // Bad mcp.json: skills still load, MCP disabled for the plugin.
        write(
            &root.join("skillful/plugin.json"),
            &MANIFEST.replace("devtools", "skillful"),
        );
        write(
            &root.join("skillful/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploys.\n---\nBody.",
        );
        write(&root.join("skillful/mcp.json"), "not json");

        // skills as a regular file: skills invalid, mcp still loads.
        write(
            &root.join("toolful/plugin.json"),
            &MANIFEST.replace("devtools", "toolful"),
        );
        write(&root.join("toolful/skills"), "i am a file");
        write(
            &root.join("toolful/mcp.json"),
            "{\"$schema\": \"https://agent-plugins.org/schemas/1.1.0/mcp.schema.json\", \"mcpServers\": {\"api\": {\"type\": \"streamable-http\", \"url\": \"https://api.example.com/mcp\"}}}",
        );

        let plugins = scan_plugins_root(&root);
        assert_eq!(plugins.len(), 2);

        let skillful = plugins
            .iter()
            .find(|p| p.manifest.name == "skillful")
            .unwrap();
        assert_eq!(skillful.skills.len(), 1, "skills survive bad mcp.json");
        assert!(skillful.servers.is_empty());
        assert!(skillful.mcp_disabled_reason.is_some());

        let toolful = plugins
            .iter()
            .find(|p| p.manifest.name == "toolful")
            .unwrap();
        assert!(
            toolful.skills.is_empty(),
            "skills file invalidates skills only"
        );
        assert_eq!(
            toolful.servers.len(),
            1,
            "mcp survives invalid skills location"
        );
        assert!(toolful.mcp_disabled_reason.is_none());
    }

    /// A missing plugins root is an empty plugin set, not an error
    /// (REQ-AP-002).
    #[test]
    fn missing_root_is_empty() {
        let plugins = scan_plugins_root(Path::new("/nonexistent/plugins/root"));
        assert!(plugins.is_empty());
    }

    /// Duplicate manifest names: first discovered (alphabetical) wins
    /// (REQ-AP-002).
    #[test]
    fn duplicate_manifest_name_first_wins() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");
        write(&root.join("a-copy/plugin.json"), MANIFEST);
        write(&root.join("b-copy/plugin.json"), MANIFEST);
        let plugins = scan_plugins_root(&root);
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].root.ends_with("a-copy"));
    }

    /// An escaping SKILL.md symlink is skipped; siblings continue
    /// (REQ-AP-007).
    #[test]
    fn escaping_skill_symlink_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");
        write(
            &root.join("escaping/plugin.json"),
            &MANIFEST.replace("devtools", "escaping"),
        );
        write(
            &root.join("escaping/skills/real/SKILL.md"),
            "---\nname: real\ndescription: Stays.\n---\nBody.",
        );
        let outside_skill = dir.path().join("outside-skill");
        write(
            &outside_skill,
            "---\nname: outside\ndescription: Escapes.\n---\nBody.",
        );
        let link = root.join("escaping/skills/escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_skill, &link).unwrap();

        let plugins = scan_plugins_root(&root);
        assert_eq!(plugins.len(), 1);
        let plugin = &plugins[0];
        assert_eq!(
            plugin.skills.len(),
            1,
            "escape link skipped, real skill kept"
        );
        assert_eq!(plugin.skills[0].name(), "real");
    }
}
