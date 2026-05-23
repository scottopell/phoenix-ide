use std::process::Command;

fn main() {
    // Embed a short git SHA into the binary so the UI can surface exactly
    // which build is running. Falls back to "unknown" when git isn't
    // available (e.g. tarball builds, where .git/ is absent).
    //
    // No `cargo:rerun-if-changed=.git/HEAD` directive: those paths may not
    // exist in tarball builds. By emitting no rerun directives we let
    // cargo's default (rerun when any source in the package changes)
    // handle invalidation. The script is cheap and its output is stable
    // across runs at a given commit, so any cascade is bounded.
    let sha = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string());

    println!("cargo:rustc-env=PHOENIX_GIT_SHA={sha}");
}
