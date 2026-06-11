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

fn main() {
    // Embed a short git SHA into the binary so the UI can surface exactly
    // which build is running. Falls back to "unknown" when git isn't
    // available (e.g. tarball builds, where .git/ is absent).
    //
    // Rerun on every build: a rerun-if-changed path that never exists is
    // always considered out-of-date (documented cargo behavior). The git
    // state feeding the output cannot be enumerated as a fixed path list —
    // unstaged edits to tracked files touch neither git metadata nor (once
    // any directive is emitted) cargo's default package scan, and a packed
    // ref can become loose at a path that didn't exist at build time. Both
    // would freeze a stale SHA or, worse, a false-clean marker into the
    // binary. Rerunning unconditionally costs two git invocations; the
    // rustc-env value is fingerprinted, so an unchanged SHA does not
    // recompile the crate.
    println!("cargo:rerun-if-changed=__phoenix_git_sha_force_rerun__");

    let sha = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".to_string());

    // "-dirty" suffix when the working tree has uncommitted changes, so a
    // binary built from modified sources can't masquerade as the commit it
    // was branched from. Only meaningful when the SHA itself resolved.
    let dirty = sha != "unknown" && git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());

    let suffix = if dirty { "-dirty" } else { "" };
    println!("cargo:rustc-env=PHOENIX_GIT_SHA={sha}{suffix}");
}
