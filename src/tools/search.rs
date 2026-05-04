//! Search tool - regex/text search across files
//!
//! REQ-PROJ-002, REQ-PROJ-013: Explore mode search without bash

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_RESULTS: usize = 50;
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ENTRIES_VISITED: usize = 100_000;
const CANCEL_CHECK_LINE_INTERVAL: usize = 1024;

/// Directories to prune from the walk (applied via `WalkBuilder::filter_entry`,
/// so descent into these subtrees is skipped entirely). `.git` is in here
/// because hidden-file filtering is now off — see `hidden(false)` below.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".next",
    "__pycache__",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    ".tox",
];

/// File extensions that are unambiguously binary. Files matching these are
/// skipped without opening, avoiding the per-file open+read syscalls used by
/// the byte-sniffing fallback.
const BINARY_EXTS: &[&str] = &[
    "wasm", "so", "dylib", "dll", "a", "o", "obj", "exe", "bin", "rlib", "lib", "zip", "gz", "tar",
    "tgz", "bz2", "xz", "7z", "rar", "zst", "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp",
    "tiff", "psd", "heic", "mp3", "mp4", "wav", "ogg", "avi", "mov", "mkv", "webm", "flac", "m4a",
    "pdf", "ttf", "otf", "woff", "woff2", "eot", "class", "jar", "war", "pyc", "pyo", "db",
    "sqlite", "sqlite3",
];

/// Search for text patterns across files.
pub struct SearchTool;

#[derive(Debug, Deserialize)]
struct SearchInput {
    pattern: String,
    path: Option<String>,
    include: Option<String>,
    exclude: Option<String>,
    max_results: Option<usize>,
}

/// Resolve a search path relative to `working_dir`. Absolute paths are used
/// as-is; relative paths are joined to `working_dir`. No containment check —
/// see the note on `read_file::resolve_and_validate` for rationale.
fn resolve_and_validate(path_str: &str, working_dir: &Path) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path_str);
    let resolved = if raw.is_absolute() {
        raw
    } else {
        working_dir.join(&raw)
    };

    resolved
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path '{}': {e}", resolved.display()))
}

/// Decide if a file should be treated as binary. Fast path: well-known binary
/// extensions skip the open+read entirely. Fallback for unknown extensions:
/// open the file and look for a null byte in the first 8KB.
fn is_binary(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let lower = ext.to_ascii_lowercase();
        if BINARY_EXTS.contains(&lower.as_str()) {
            return true;
        }
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::with_capacity(8192, file);
    let Ok(buf) = reader.fill_buf() else {
        return false;
    };
    buf.contains(&0)
}

/// Glob filter with gitignore-style semantics: a pattern with no `/` matches
/// against the file name only (so `*.rs` matches all Rust files at any depth);
/// a pattern with `/` matches the path relative to the search root.
struct GlobFilter {
    matcher: GlobMatcher,
    pattern_has_slash: bool,
}

impl GlobFilter {
    fn new(pattern: &str) -> Result<Self, String> {
        let glob = Glob::new(pattern).map_err(|e| format!("Invalid glob '{pattern}': {e}"))?;
        Ok(Self {
            matcher: glob.compile_matcher(),
            pattern_has_slash: pattern.contains('/'),
        })
    }

    fn matches(&self, file_name: &str, rel_path: &Path) -> bool {
        if self.pattern_has_slash {
            self.matcher.is_match(rel_path)
        } else {
            self.matcher.is_match(file_name)
        }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &'static str {
        "search"
    }

    fn description(&self) -> String {
        "Search for a text pattern across files in the project. Returns matching lines with file paths and line numbers. Default scope is the conversation's working directory; only pass `path` to narrow further or to search outside the project (rare)."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex)."
                },
                "path": {
                    "type": "string",
                    "description": "File or subdirectory to search. Defaults to the project root; only specify when narrowing scope."
                },
                "include": {
                    "type": "string",
                    "description": "Include glob. Patterns without `/` match the file name (e.g. \"*.rs\" matches all Rust files at any depth). Patterns with `/` match the path relative to the search root (e.g. \"src/**/*.ts\")."
                },
                "exclude": {
                    "type": "string",
                    "description": "Exclude glob, same matching rules as `include` (e.g. \"*.test.ts\" excludes test files; \"vendor/**\" excludes a subtree)."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return. Default: 50."
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: SearchInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let re = match Regex::new(&input.pattern) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Invalid regex pattern: {e}")),
        };

        let search_path = match &input.path {
            Some(p) => match resolve_and_validate(p, &ctx.working_dir) {
                Ok(resolved) => resolved,
                Err(e) => return ToolOutput::error(e),
            },
            None => match ctx.working_dir.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    return ToolOutput::error(format!("Cannot resolve working directory: {e}"))
                }
            },
        };

        let canonical_wd = match ctx.working_dir.canonicalize() {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(format!("Cannot resolve working directory: {e}")),
        };

        let include = match input.include.as_deref().map(GlobFilter::new).transpose() {
            Ok(f) => f,
            Err(e) => return ToolOutput::error(e),
        };
        let exclude = match input.exclude.as_deref().map(GlobFilter::new).transpose() {
            Ok(f) => f,
            Err(e) => return ToolOutput::error(e),
        };

        let max_results = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        let pattern_str = input.pattern;
        let cancel = ctx.cancel.clone();
        let conv_id = ctx.conversation_id.clone();

        match tokio::task::spawn_blocking(move || {
            run_search(SearchArgs {
                re,
                search_path,
                canonical_wd,
                include,
                exclude,
                max_results,
                cancel,
                conv_id,
                pattern_str,
            })
        })
        .await
        {
            Ok(out) => out,
            Err(e) => ToolOutput::error(format!("Search task failed: {e}")),
        }
    }
}

struct SearchArgs {
    re: Regex,
    search_path: PathBuf,
    canonical_wd: PathBuf,
    include: Option<GlobFilter>,
    exclude: Option<GlobFilter>,
    max_results: usize,
    cancel: CancellationToken,
    conv_id: String,
    pattern_str: String,
}

fn run_search(args: SearchArgs) -> ToolOutput {
    let SearchArgs {
        re,
        search_path,
        canonical_wd,
        include,
        exclude,
        max_results,
        cancel,
        conv_id,
        pattern_str,
    } = args;

    let walker = WalkBuilder::new(&search_path)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if SKIP_DIRS.contains(&name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    let mut results = Vec::new();
    let mut entries_visited: usize = 0;
    let mut walk_truncated = false;

    for entry in walker {
        if cancel.is_cancelled() {
            return ToolOutput::error("Cancelled");
        }

        entries_visited += 1;
        if entries_visited > MAX_ENTRIES_VISITED {
            walk_truncated = true;
            tracing::warn!(
                conv_id = %conv_id,
                cap = MAX_ENTRIES_VISITED,
                path = %search_path.display(),
                pattern = %pattern_str,
                "search walk truncated at entry cap; agent should narrow path or include"
            );
            break;
        }

        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let rel_to_search = path.strip_prefix(&search_path).unwrap_or(path);

        if let Some(ref f) = include {
            if !f.matches(file_name, rel_to_search) {
                continue;
            }
        }
        if let Some(ref f) = exclude {
            if f.matches(file_name, rel_to_search) {
                continue;
            }
        }

        if scan_file(path, &re, &canonical_wd, &mut results, max_results, &cancel) {
            break;
        }
    }

    format_output(&results, max_results, walk_truncated)
}

/// Scan one file for `re`, appending `display_path:lineno: line` to `results`.
/// Returns `true` if `max_results` was hit and the outer walk should stop.
/// Cancellation is checked between expensive sub-steps and every
/// `CANCEL_CHECK_LINE_INTERVAL` lines inside the read.
fn scan_file(
    path: &Path,
    re: &Regex,
    canonical_wd: &Path,
    results: &mut Vec<String>,
    max_results: usize,
    cancel: &CancellationToken,
) -> bool {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_FILE_BYTES {
            return false;
        }
    }
    if cancel.is_cancelled() {
        return false;
    }
    if is_binary(path) {
        return false;
    }
    if cancel.is_cancelled() {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let reader = std::io::BufReader::new(file.take(MAX_FILE_BYTES));
    let display_path = path
        .strip_prefix(canonical_wd)
        .unwrap_or(path)
        .display()
        .to_string();

    for (line_num, line_res) in reader.lines().enumerate() {
        if line_num % CANCEL_CHECK_LINE_INTERVAL == 0 && cancel.is_cancelled() {
            return false;
        }
        let Ok(line) = line_res else {
            break;
        };
        if re.is_match(&line) {
            results.push(format!("{}:{}: {}", display_path, line_num + 1, line));
            if results.len() >= max_results {
                return true;
            }
        }
    }
    false
}

fn format_output(results: &[String], max_results: usize, walk_truncated: bool) -> ToolOutput {
    if results.is_empty() && !walk_truncated {
        return ToolOutput::success("No matches found.");
    }

    let mut output = results.join("\n");
    if results.len() >= max_results {
        let _ = write!(
            output,
            "\n\n[Results limited to {max_results} matches. Use a more specific pattern or path to narrow results.]"
        );
    }
    if walk_truncated {
        let _ = write!(
            output,
            "\n\n[Walk truncated at {MAX_ENTRIES_VISITED} entries. Narrow `path` or `include`.]"
        );
    }

    ToolOutput::success(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::BrowserSessionManager;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            Arc::new(crate::tools::TmuxRegistry::new()),
        )
    }

    #[tokio::test]
    async fn test_search_basic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("hello.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("other.txt"), "nothing here\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "println"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("hello.rs"));
        assert!(result.output.contains("println"));
    }

    #[tokio::test]
    async fn test_search_with_include() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn hello()\n").unwrap();
        std::fs::write(dir.path().join("code.txt"), "fn hello()\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "hello", "include": "*.rs"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("code.rs"));
        assert!(!result.output.contains("code.txt"));
    }

    #[tokio::test]
    async fn test_search_max_results() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (1..=100).fold(String::new(), |mut s, i| {
            use std::fmt::Write;
            let _ = writeln!(s, "match line {i}");
            s
        });
        std::fs::write(dir.path().join("big.txt"), &content).unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "match", "max_results": 5}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("Results limited to 5"));
    }

    #[tokio::test]
    async fn test_search_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "zzzznotfound"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_search_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "[invalid"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_search_allows_paths_outside_working_dir() {
        // Consistency with bash/read_file: search can walk any directory the
        // process has permission to read. Guards against re-introducing the
        // old working-directory containment check.
        let outer = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir_in(outer.path()).unwrap();
        let outside_dir = tempfile::tempdir_in(outer.path()).unwrap();
        std::fs::write(outside_dir.path().join("hit.txt"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme", "path": outside_dir.path().to_str().unwrap()}),
                test_context(inner.path().to_path_buf()),
            )
            .await;
        assert!(
            result.success,
            "search should resolve paths outside the working dir: {}",
            result.output
        );
        assert!(result.output.contains("findme"));
    }

    #[test]
    fn test_glob_filter_filename_only() {
        let f = GlobFilter::new("*.rs").unwrap();
        assert!(f.matches("test.rs", Path::new("any/where/test.rs")));
        assert!(!f.matches("test.ts", Path::new("test.ts")));
        assert!(!f.pattern_has_slash);
    }

    #[test]
    fn test_glob_filter_path_with_double_star() {
        let f = GlobFilter::new("src/**/*.rs").unwrap();
        assert!(f.matches("foo.rs", Path::new("src/a/b/foo.rs")));
        assert!(!f.matches("foo.rs", Path::new("other/foo.rs")));
        assert!(f.pattern_has_slash);
    }

    #[test]
    fn test_glob_filter_invalid_pattern_errors() {
        let res = GlobFilter::new("[unclosed");
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_search_prunes_skip_dirs() {
        let dir = tempfile::tempdir().unwrap();
        for skipped in &["node_modules", "target", "vendor", "dist", ".git"] {
            let sub = dir.path().join(skipped);
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("inside.txt"), "findme\n").unwrap();
        }
        std::fs::write(dir.path().join("real.rs"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(
            result.output.contains("real.rs"),
            "should find real source: {}",
            result.output
        );
        for skipped in &["node_modules", "target", "vendor", "dist", ".git"] {
            assert!(
                !result.output.contains(skipped),
                "should not descend into {skipped}: {}",
                result.output
            );
        }
    }

    #[tokio::test]
    async fn test_search_skips_files_over_size_cap() {
        let dir = tempfile::tempdir().unwrap();

        let big_path = dir.path().join("big.log");
        let cap = usize::try_from(MAX_FILE_BYTES).unwrap();
        let mut big = vec![b'x'; cap + 1024];
        big.extend_from_slice(b"\nfindme matchhere\n");
        std::fs::write(&big_path, &big).unwrap();

        std::fs::write(dir.path().join("small.txt"), "findme matchhere\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("small.txt"), "{}", result.output);
        assert!(
            !result.output.contains("big.log"),
            "oversize file should be skipped: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_search_includes_dotfiles() {
        // hidden(false) so that .github/, .cargo/, etc. are searchable. .git
        // remains pruned via SKIP_DIRS.
        let dir = tempfile::tempdir().unwrap();
        let dotdir = dir.path().join(".github");
        std::fs::create_dir(&dotdir).unwrap();
        std::fs::write(dotdir.join("ci.yml"), "findme\n").unwrap();
        std::fs::write(dir.path().join(".env.example"), "findme\n").unwrap();
        let gitdir = dir.path().join(".git");
        std::fs::create_dir(&gitdir).unwrap();
        std::fs::write(gitdir.join("config"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains(".github"), "{}", result.output);
        assert!(result.output.contains(".env.example"), "{}", result.output);
        assert!(
            !result.output.contains(".git/config") && !result.output.contains(".git\\config"),
            ".git should still be pruned: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_search_skips_known_binary_extensions() {
        // .wasm files contain valid UTF-8 sequences early on; the byte-sniff
        // fallback would still skip them, but only after open+read. Extension
        // allowlist short-circuits that.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blob.wasm"), "findme just text\n").unwrap();
        std::fs::write(dir.path().join("real.rs"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("real.rs"), "{}", result.output);
        assert!(
            !result.output.contains("blob.wasm"),
            ".wasm should be skipped by extension: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_search_include_double_star_matches_subtree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/inner")).unwrap();
        std::fs::write(dir.path().join("src/inner/deep.rs"), "findme\n").unwrap();
        std::fs::write(dir.path().join("other.rs"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme", "include": "src/**/*.rs"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("deep.rs"), "{}", result.output);
        assert!(
            !result.output.contains("other.rs"),
            "outside src should be excluded by glob: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_search_exclude_param() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.ts"), "findme\n").unwrap();
        std::fs::write(dir.path().join("real.test.ts"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme", "exclude": "*.test.ts"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("real.ts"), "{}", result.output);
        assert!(
            !result.output.contains("real.test.ts"),
            "exclude should drop test files: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_search_streams_lines_without_full_load() {
        // A file just under the size cap should still be searched. This exercises
        // the streamed-read path (no read_to_string of the full content).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("medium.txt");
        let filler_line = "x".repeat(1024);
        let cap = usize::try_from(MAX_FILE_BYTES).unwrap();
        let mut content = String::with_capacity(cap);
        while content.len() + filler_line.len() + 1 < cap - 256 {
            content.push_str(&filler_line);
            content.push('\n');
        }
        content.push_str("findme matchhere\n");
        std::fs::write(&path, &content).unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("medium.txt"), "{}", result.output);
    }
}
