use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| {
        String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_string())
    })?
}

fn main() {
    // Git state includes refs and unstaged source edits, neither of which Cargo can
    // express as a stable watched path. An intentionally absent path makes Cargo
    // rerun this script without forcing rustc when the emitted values are unchanged.
    println!("cargo:rerun-if-changed=__phoenix_db_git_identity_force_rerun__");

    let sha = git(&["rev-parse", "--verify", "HEAD^{commit}"]).filter(|sha| {
        sha.len() == 40 && sha.chars().all(|character| character.is_ascii_hexdigit())
    });
    let dirty = git(&["status", "--porcelain=v1", "--untracked-files=normal"])
        .map(|status| !status.is_empty());

    println!(
        "cargo:rustc-env=PHOENIX_DB_GIT_SHA={}",
        sha.as_deref().unwrap_or("unknown")
    );
    println!(
        "cargo:rustc-env=PHOENIX_DB_GIT_DIRTY={}",
        match dirty {
            Some(true) => "dirty",
            Some(false) => "clean",
            None => "unknown",
        }
    );
    println!(
        "cargo:rustc-env=PHOENIX_DB_PACKAGE_VERSION={}",
        env!("CARGO_PKG_VERSION")
    );
}
