//! Named sub-agent discovery and metadata (REQ-AG-001 through REQ-AG-003).
//!
//! This crate owns the named-agent machinery: walking the working-directory
//! tree for agent definitions (`.claude/agents/`, `.agents/agents/`), parsing
//! their YAML frontmatter, and producing a deterministically ordered catalog
//! of [`AgentDefinition`]s.
//!
//! It mirrors the discovery shape of `phoenix-skills` (walk-up + child
//! directories + `$HOME`, with symlink/content/name dedup) but differs in two
//! ways that match the ecosystem layout for agents: a named agent is a single
//! Markdown file (not a directory with a manifest), and agents are not
//! namespaced into sub-directories.
//!
//! A selected agent supplies the spawned sub-agent's persona (its `body`),
//! plus its default `model` and `mode`. Resolution precedence and persona
//! composition live in the spawn layer (see `specs/subagents/` and
//! `specs/agents/`); this crate only discovers and parses.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub use phoenix_core::domain::sm_state::SubAgentMode;

/// A named agent discovered from the filesystem.
///
/// The `body` is the persona instructions (frontmatter stripped); `model` and
/// `mode` are optional defaults the spawn layer resolves between the LLM's
/// explicit task fields and the mode-based defaults.
#[derive(Debug, Clone)]
pub struct AgentDefinition {
    /// Frontmatter `name`; the `agent_type` value used to select this agent.
    pub name: String,
    /// Frontmatter `description`; shown on the `agent_type` enum choice.
    pub description: String,
    /// File body with frontmatter stripped — the persona system prompt.
    pub body: String,
    /// Absolute path to the agent `.md` file.
    pub path: PathBuf,
    /// Discovery directory, e.g. ".claude/agents" or ".agents/agents".
    pub source_dir: String,
    /// Optional default model id from frontmatter (`model`).
    pub model: Option<String>,
    /// Optional default sub-agent mode from frontmatter (`mode`).
    pub mode: Option<SubAgentMode>,
    /// Optional `tools` allowlist, parsed and preserved but inert (REQ-AG-009).
    pub tools: Option<Vec<String>>,
}

/// Parsed frontmatter fields from an agent `.md` file.
struct AgentFrontmatter {
    name: String,
    description: String,
    model: Option<String>,
    mode: Option<SubAgentMode>,
    tools: Option<Vec<String>>,
}

/// Parse `name`, `description`, and optional `model`/`mode`/`tools` from an
/// agent file's YAML frontmatter.
///
/// Expects the file to start with `---\n`, followed by `key: value` lines,
/// closed by `\n---\n`. Returns `None` if either required field
/// (`name`, `description`) is missing or the frontmatter is malformed.
fn parse_agent_frontmatter(content: &str) -> Option<AgentFrontmatter> {
    let body = content.strip_prefix("---\n")?;
    let end = body
        .find("\n---\n")
        .or_else(|| body.find("\n---").filter(|&i| i + 4 == body.len()))?;
    // Safety: `end` is from `find()` on `body`.
    #[allow(clippy::string_slice)]
    let frontmatter = &body[..end];

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut model: Option<String> = None;
    let mut mode: Option<SubAgentMode> = None;
    let mut tools: Option<Vec<String>> = None;

    for line in frontmatter.lines() {
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(unquote_scalar(val));
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(unquote_scalar(val));
        } else if let Some(val) = line.strip_prefix("model:") {
            let m = unquote_scalar(val);
            if !m.is_empty() {
                model = Some(m);
            }
        } else if let Some(val) = line.strip_prefix("mode:") {
            mode = parse_mode(&unquote_scalar(val));
        } else if let Some(val) = line.strip_prefix("tools:") {
            tools = parse_tools(val.trim());
        }
    }

    // Required fields must be present and non-empty.
    let name = name.filter(|s| !s.is_empty())?;
    let description = description.filter(|s| !s.is_empty())?;

    Some(AgentFrontmatter {
        name,
        description,
        model,
        mode,
        tools,
    })
}

/// Strip a single layer of matching surrounding quotes from a YAML scalar,
/// after trimming whitespace. `name: "security-reviewer"` and
/// `name: security-reviewer` both yield `security-reviewer`. Without this the
/// quotes become part of the `agent_type` enum value and the LLM's unquoted
/// selection is rejected as unknown.
fn unquote_scalar(value: &str) -> String {
    let trimmed = value.trim();
    for q in ['"', '\''] {
        if let Some(inner) = trimmed.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner.to_string();
        }
    }
    trimmed.to_string()
}

/// Map a frontmatter `mode` string to a [`SubAgentMode`]; unknown values map
/// to `None` so an invalid mode falls through to the mode-based default rather
/// than silently picking one.
fn parse_mode(value: &str) -> Option<SubAgentMode> {
    match value {
        "explore" => Some(SubAgentMode::Explore),
        "work" => Some(SubAgentMode::Work),
        _ => None,
    }
}

/// Parse a `tools` frontmatter value. Accepts inline-list form
/// (`[read_file, bash]`) and bare comma/space-separated names. The field is
/// inert in v1 (REQ-AG-009); parsing it keeps the on-disk format
/// forward-compatible. Returns `None` for an empty value.
fn parse_tools(value: &str) -> Option<Vec<String>> {
    let inner = value
        .strip_prefix('[')
        .map_or(value, |rest| rest.strip_suffix(']').unwrap_or(rest));
    let names: Vec<String> = inner
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

/// Subdirectories scanned for agent `.md` files at each level of the tree.
const AGENT_DIRS: &[&str] = &[".claude/agents", ".agents/agents"];

/// Collect agents from a single agents directory (e.g. `.claude/agents/`).
///
/// Scans immediate child files ending in `.md`. Each file with valid
/// frontmatter becomes one [`AgentDefinition`]. Agents are not namespaced —
/// there is one flat level per agents directory.
fn collect_agents_from_dir(
    agents_dir: &Path,
    source: &str,
    agents: &mut Vec<AgentDefinition>,
    seen_names: &mut HashSet<String>,
    seen_paths: &mut HashSet<PathBuf>,
    seen_content: &mut HashSet<u64>,
) {
    let Ok(entries) = std::fs::read_dir(agents_dir) else {
        return;
    };
    // Sort by path before processing: `read_dir` order is filesystem-dependent,
    // and name dedup is first-wins, so two same-`name` files in one directory
    // would otherwise resolve nondeterministically across machines — breaking
    // the cache-stable catalog guarantee (REQ-AG-008). Lexicographic path order
    // makes the winner deterministic.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Symlink dedup: canonicalize to detect duplicates.
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen_paths.insert(canonical) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Content dedup: hash file content to catch copies.
        let content_hash = {
            let mut hasher = std::hash::DefaultHasher::new();
            content.hash(&mut hasher);
            hasher.finish()
        };
        if !seen_content.insert(content_hash) {
            continue;
        }
        let Some(fm) = parse_agent_frontmatter(&content) else {
            // Missing a required field: skip without aborting discovery (REQ-AG-002).
            continue;
        };
        if seen_names.insert(fm.name.clone()) {
            agents.push(AgentDefinition {
                name: fm.name,
                description: fm.description,
                body: strip_frontmatter(&content),
                path,
                source_dir: source.to_string(),
                model: fm.model,
                mode: fm.mode,
                tools: fm.tools,
            });
        }
    }
}

/// Discover agents for `working_dir` by walking up to the filesystem root,
/// scanning immediate child directories, and finally `$HOME`.
///
/// When the same agent name appears at multiple levels, the one closer to
/// `working_dir` wins (more specific overrides parent). Returns agents sorted
/// by name — the sort is contractual: it is what makes the `agent_type` enum
/// byte-stable across a conversation's turns (REQ-AG-008).
#[must_use]
pub fn discover_agents(working_dir: &Path) -> Vec<AgentDefinition> {
    discover_agents_with_home(working_dir, None)
}

/// Discovery with an explicit `$HOME` override. Production goes through
/// [`discover_agents`]; tests use this to inject a deterministic `$HOME`
/// without mutating process-global env vars.
#[must_use]
pub fn discover_agents_with_home(
    working_dir: &Path,
    home_override: Option<&Path>,
) -> Vec<AgentDefinition> {
    let mut agents: Vec<AgentDefinition> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut seen_content: HashSet<u64> = HashSet::new();
    let mut scanned_dirs: HashSet<PathBuf> = HashSet::new();

    // 1. Walk from working_dir to root; working_dir first so it wins.
    let mut current = Some(working_dir.to_path_buf());
    while let Some(dir) = current {
        scan_level(
            &dir,
            &mut agents,
            &mut seen_names,
            &mut seen_paths,
            &mut seen_content,
            &mut scanned_dirs,
        );
        current = dir.parent().map(Path::to_path_buf);
    }

    // 2. Immediate child directories (projects-directory case). Sort by path
    //    so that when two sibling projects declare the same agent name the
    //    first-wins dedup is deterministic across filesystems (REQ-AG-008),
    //    mirroring the within-directory sort in collect_agents_from_dir.
    if let Ok(children) = std::fs::read_dir(working_dir) {
        let mut child_dirs: Vec<PathBuf> = children
            .flatten()
            .map(|c| c.path())
            .filter(|p| p.is_dir())
            .collect();
        child_dirs.sort();
        for dir in &child_dirs {
            scan_level(
                dir,
                &mut agents,
                &mut seen_names,
                &mut seen_paths,
                &mut seen_content,
                &mut scanned_dirs,
            );
        }
    }

    // 3. $HOME, if the walk-up didn't already pass through it.
    let resolved_home = match home_override {
        Some(h) => Some(h.to_path_buf()),
        None => Some(
            phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()
                .home()
                .to_path_buf(),
        ),
    };
    if let Some(home) = resolved_home {
        scan_level(
            &home,
            &mut agents,
            &mut seen_names,
            &mut seen_paths,
            &mut seen_content,
            &mut scanned_dirs,
        );
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

/// Scan both `AGENT_DIRS` under a single tree level.
fn scan_level(
    dir: &Path,
    agents: &mut Vec<AgentDefinition>,
    seen_names: &mut HashSet<String>,
    seen_paths: &mut HashSet<PathBuf>,
    seen_content: &mut HashSet<u64>,
    scanned_dirs: &mut HashSet<PathBuf>,
) {
    for agent_subdir in AGENT_DIRS {
        let agents_dir = dir.join(agent_subdir);
        if !agents_dir.is_dir() {
            continue;
        }
        let canonical_dir =
            std::fs::canonicalize(&agents_dir).unwrap_or_else(|_| agents_dir.clone());
        if !scanned_dirs.insert(canonical_dir) {
            continue;
        }
        collect_agents_from_dir(
            &agents_dir,
            agent_subdir,
            agents,
            seen_names,
            seen_paths,
            seen_content,
        );
    }
}

/// Look up a discovered agent by exact name.
#[must_use]
pub fn find_agent<'a>(agents: &'a [AgentDefinition], name: &str) -> Option<&'a AgentDefinition> {
    agents.iter().find(|a| a.name == name)
}

/// Strip YAML frontmatter (`---` delimited block at the top of the file),
/// returning the body. Shares the skills approach: a file without a leading
/// `---` is returned unchanged (treated as all-body).
fn strip_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    // Safety: checked `trimmed` starts with "---" (3 bytes).
    #[allow(clippy::string_slice)]
    let after_open = &trimmed[3..];
    if let Some(end_pos) = after_open.find("\n---") {
        // Safety: `end_pos` from `find()`; +4 accounts for "\n---".
        #[allow(clippy::string_slice)]
        let body_start = &after_open[end_pos + 4..];
        body_start.trim_start_matches('\n').to_string()
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Write an agent file at `{base}/{agents_subdir}/{file_name}` with the
    /// given frontmatter and body.
    fn write_agent(
        base: &Path,
        agents_subdir: &str,
        file_name: &str,
        frontmatter: &str,
        body: &str,
    ) {
        let dir = base.join(agents_subdir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file_name),
            format!("---\n{frontmatter}---\n\n{body}"),
        )
        .unwrap();
    }

    // ---- frontmatter parsing -------------------------------------------------

    #[test]
    fn parses_required_fields() {
        let fm =
            parse_agent_frontmatter("---\nname: reviewer\ndescription: Reviews code\n---\n\nBody.")
                .unwrap();
        assert_eq!(fm.name, "reviewer");
        assert_eq!(fm.description, "Reviews code");
        assert_eq!(fm.model, None);
        assert_eq!(fm.mode, None);
        assert_eq!(fm.tools, None);
    }

    #[test]
    fn parses_optional_fields() {
        let fm = parse_agent_frontmatter(
            "---\nname: r\ndescription: d\nmodel: claude-sonnet-4-6\nmode: work\ntools: [read_file, bash]\n---\n\nBody.",
        )
        .unwrap();
        assert_eq!(fm.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(fm.mode, Some(SubAgentMode::Work));
        assert_eq!(
            fm.tools,
            Some(vec!["read_file".to_string(), "bash".to_string()])
        );
    }

    #[test]
    fn missing_name_is_none() {
        assert!(parse_agent_frontmatter("---\ndescription: d\n---\n").is_none());
    }

    #[test]
    fn missing_description_is_none() {
        assert!(parse_agent_frontmatter("---\nname: r\n---\n").is_none());
    }

    #[test]
    fn empty_required_field_is_none() {
        assert!(parse_agent_frontmatter("---\nname: \ndescription: d\n---\n").is_none());
    }

    #[test]
    fn invalid_mode_falls_through_to_none() {
        let fm =
            parse_agent_frontmatter("---\nname: r\ndescription: d\nmode: turbo\n---\n").unwrap();
        assert_eq!(fm.mode, None);
    }

    #[test]
    fn tools_bare_list_parsed() {
        assert_eq!(
            parse_tools("read_file, bash"),
            Some(vec!["read_file".to_string(), "bash".to_string()])
        );
        assert_eq!(parse_tools(""), None);
    }

    // ---- discovery -----------------------------------------------------------

    #[test]
    fn discovers_none_in_empty_tree() {
        let tmp = TempDir::new().unwrap();
        assert!(discover_agents_with_home(tmp.path(), Some(tmp.path())).is_empty());
    }

    #[test]
    fn discovers_from_claude_and_agents_dirs() {
        let tmp = TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "a.md",
            "name: a\ndescription: A\n",
            "Persona A",
        );
        write_agent(
            tmp.path(),
            ".agents/agents",
            "b.md",
            "name: b\ndescription: B\n",
            "Persona B",
        );
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].name, "a");
        assert_eq!(agents[1].name, "b");
        assert_eq!(agents[0].source_dir, ".claude/agents");
        assert_eq!(agents[1].source_dir, ".agents/agents");
    }

    #[test]
    fn body_has_frontmatter_stripped() {
        let tmp = TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "r.md",
            "name: r\ndescription: d\n",
            "You are a reviewer.",
        );
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        assert_eq!(agents[0].body, "You are a reviewer.");
        assert!(!agents[0].body.starts_with("---"));
    }

    #[test]
    fn sorted_by_name() {
        let tmp = TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "z.md",
            "name: zzz\ndescription: d\n",
            "z",
        );
        write_agent(
            tmp.path(),
            ".claude/agents",
            "a.md",
            "name: aaa\ndescription: d\n",
            "a",
        );
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        assert_eq!(agents[0].name, "aaa");
        assert_eq!(agents[1].name, "zzz");
    }

    #[test]
    fn closer_to_cwd_wins() {
        let tmp = TempDir::new().unwrap();
        let child = tmp.path().join("project");
        fs::create_dir(&child).unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "r.md",
            "name: r\ndescription: parent\n",
            "p",
        );
        write_agent(
            &child,
            ".claude/agents",
            "r.md",
            "name: r\ndescription: child\n",
            "c",
        );
        let agents = discover_agents_with_home(&child, Some(tmp.path()));
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description, "child");
    }

    #[test]
    fn discovery_is_byte_stable_across_runs() {
        // REQ-AG-008: repeated discovery over the same tree yields the same order.
        let tmp = TempDir::new().unwrap();
        for n in ["m", "a", "z", "k"] {
            write_agent(
                tmp.path(),
                ".claude/agents",
                &format!("{n}.md"),
                &format!("name: {n}\ndescription: d\n"),
                "body",
            );
        }
        let first: Vec<String> = discover_agents_with_home(tmp.path(), Some(tmp.path()))
            .into_iter()
            .map(|a| a.name)
            .collect();
        for _ in 0..5 {
            let again: Vec<String> = discover_agents_with_home(tmp.path(), Some(tmp.path()))
                .into_iter()
                .map(|a| a.name)
                .collect();
            assert_eq!(first, again);
        }
        assert_eq!(first, vec!["a", "k", "m", "z"]);
    }

    #[test]
    fn non_md_files_ignored() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".claude/agents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), "---\nname: x\ndescription: d\n---\n").unwrap();
        assert!(discover_agents_with_home(tmp.path(), Some(tmp.path())).is_empty());
    }

    #[test]
    fn malformed_file_skipped_others_survive() {
        let tmp = TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "good.md",
            "name: good\ndescription: d\n",
            "b",
        );
        write_agent(tmp.path(), ".claude/agents", "bad.md", "name: bad\n", "b"); // no description
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "good");
    }

    #[test]
    fn same_name_in_one_dir_resolves_deterministically() {
        // REQ-AG-008: when two files in the same directory declare the same
        // name, the lexicographically-first path wins, regardless of read_dir
        // order. "a-dup.md" sorts before "z-dup.md".
        let tmp = TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "z-dup.md",
            "name: dup\ndescription: from-z\n",
            "z",
        );
        write_agent(
            tmp.path(),
            ".claude/agents",
            "a-dup.md",
            "name: dup\ndescription: from-a\n",
            "a",
        );
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        let dup: Vec<&AgentDefinition> = agents.iter().filter(|a| a.name == "dup").collect();
        assert_eq!(dup.len(), 1, "name dedup keeps exactly one");
        assert_eq!(
            dup[0].description, "from-a",
            "lexicographically-first path wins"
        );
    }

    #[test]
    fn quoted_yaml_scalars_are_unquoted() {
        // Valid YAML quoted scalars must not leak quotes into the agent_type
        // value, or the LLM's unquoted selection would be rejected.
        let fm = parse_agent_frontmatter(
            "---\nname: \"security-reviewer\"\ndescription: 'Finds vulns'\nmodel: \"claude-sonnet-4-6\"\n---\n",
        )
        .unwrap();
        assert_eq!(fm.name, "security-reviewer");
        assert_eq!(fm.description, "Finds vulns");
        assert_eq!(fm.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn same_name_across_sibling_children_resolves_deterministically() {
        // REQ-AG-008: in the projects-directory case, two sibling projects
        // declaring the same agent name resolve to the lexicographically-first
        // child path, regardless of read_dir order.
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a-proj");
        let z = tmp.path().join("z-proj");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&z).unwrap();
        write_agent(
            &z,
            ".claude/agents",
            "r.md",
            "name: dup\ndescription: from-z\n",
            "z",
        );
        write_agent(
            &a,
            ".claude/agents",
            "r.md",
            "name: dup\ndescription: from-a\n",
            "a",
        );
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        let dup: Vec<&AgentDefinition> = agents.iter().filter(|x| x.name == "dup").collect();
        assert_eq!(dup.len(), 1);
        assert_eq!(dup[0].description, "from-a", "first child path wins");
    }

    #[test]
    fn find_agent_by_name() {
        let tmp = TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            ".claude/agents",
            "r.md",
            "name: r\ndescription: d\n",
            "b",
        );
        let agents = discover_agents_with_home(tmp.path(), Some(tmp.path()));
        assert!(find_agent(&agents, "r").is_some());
        assert!(find_agent(&agents, "nope").is_none());
    }
}
