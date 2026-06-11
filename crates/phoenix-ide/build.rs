use std::path::PathBuf;
use std::process::Command;

/// Run `git` with `args` and return trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        None
    }
}

/// Emit `cargo:rerun-if-changed` for a repo-relative git path, but only when
/// the file exists: a directive for a missing path makes cargo rerun the
/// script on every build. `--git-path` resolves worktree/commondir layouts.
fn rerun_on_git_path(name: &str) {
    if let Some(p) = git(&["rev-parse", "--git-path", name]).map(PathBuf::from) {
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
}

fn main() {
    // Embed a short git SHA into the binary so the UI can surface exactly
    // which build is running. Falls back to "unknown" when git isn't
    // available (e.g. tarball builds, where .git/ is absent).
    //
    // Once any rerun-if directive is emitted, cargo's default (rerun when
    // any source in the package changes) is replaced entirely — so the git
    // state that feeds the output must be tracked explicitly, or the SHA and
    // dirty marker go stale when HEAD moves without touching this package:
    //   HEAD          — checkout / detach
    //   resolved ref  — commit on the current branch
    //   packed-refs   — the ref may be packed instead of loose
    //   index         — stage / commit / revert (dirty-state edges)
    // In tarball builds none of these exist, no directive is emitted, and
    // the script reruns on package changes as before (output is a constant
    // "unknown" there anyway). Residual staleness: working-tree edits that
    // touch neither this package nor the index won't rerun the script, so
    // the dirty marker reflects the last build-script run — acceptable for
    // a best-effort marker.
    rerun_on_git_path("HEAD");
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        rerun_on_git_path(&r);
    }
    rerun_on_git_path("packed-refs");
    rerun_on_git_path("index");

    let sha = git(&["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());

    // "-dirty" suffix when the working tree has uncommitted changes, so a
    // binary built from modified sources can't masquerade as the commit it
    // was branched from. Only meaningful when the SHA itself resolved.
    let dirty = sha != "unknown"
        && git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());

    let suffix = if dirty { "-dirty" } else { "" };
    println!("cargo:rustc-env=PHOENIX_GIT_SHA={sha}{suffix}");
}
