use std::process::Command;

fn main() {
    // Embed a short git SHA + dirty marker into the binary so the UI can
    // surface exactly which build is running. Falls back to "unknown" when
    // git isn't available (e.g. tarball builds).
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

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|o| !o.stdout.is_empty());

    let full = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo:rustc-env=PHOENIX_GIT_SHA={full}");

    // Re-run when the git HEAD or index changes so the embedded sha doesn't
    // stale across rebuilds.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
}
