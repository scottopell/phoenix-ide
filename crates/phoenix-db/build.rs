fn git(args: &[&str]) -> Option<String> {
    let output = phoenix_core::git::command()
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_string())
    })?
}

fn is_valid_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.chars().all(|character| character.is_ascii_hexdigit())
}

fn dev_check_git_identity() -> Option<(String, &'static str)> {
    // Release builds must always probe live git state directly.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        return None;
    }
    let sha = std::env::var("PHOENIX_BUILD_GIT_SHA").ok()?;
    let dirty = std::env::var("PHOENIX_BUILD_GIT_DIRTY").ok()?;
    let valid_sha = sha == "unknown" || is_valid_sha(&sha);
    let normalized_dirty = match dirty.as_str() {
        "clean" => "clean",
        "dirty" => "dirty",
        "unknown" => "unknown",
        _ => return None,
    };
    valid_sha.then_some((sha, normalized_dirty))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PHOENIX_BUILD_GIT_SHA");
    println!("cargo:rerun-if-env-changed=PHOENIX_BUILD_GIT_DIRTY");

    let (sha, dirty) = if let Some((sha, dirty)) = dev_check_git_identity() {
        (sha, dirty)
    } else {
        // Standalone and release builds probe live git state on every build so
        // unstaged edits anywhere in the repository (including UI assets or
        // untracked files) can never leave a stale dirty/clean marker.
        println!("cargo:rerun-if-changed=__phoenix_db_git_identity_force_rerun__");
        let sha = git(&["rev-parse", "--verify", "HEAD^{commit}"])
            .filter(|sha| is_valid_sha(sha))
            .unwrap_or_else(|| "unknown".to_string());
        let dirty = match git(&["status", "--porcelain=v1", "--untracked-files=normal"])
            .map(|status| !status.is_empty())
        {
            Some(true) => "dirty",
            Some(false) => "clean",
            None => "unknown",
        };
        (sha, dirty)
    };

    println!("cargo:rustc-env=PHOENIX_DB_GIT_SHA={sha}");
    println!("cargo:rustc-env=PHOENIX_DB_GIT_DIRTY={dirty}");
    println!(
        "cargo:rustc-env=PHOENIX_DB_PACKAGE_VERSION={}",
        env!("CARGO_PKG_VERSION")
    );
}
