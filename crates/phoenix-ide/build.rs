use std::process::Command;

/// Run `git` with `args` and return trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        None
    }
}

fn is_valid_lowercase_sha(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn dev_check_git_sha() -> Option<String> {
    // Release builds must always probe live git state directly.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        return None;
    }
    let sha = std::env::var("PHOENIX_BUILD_GIT_SHA").ok()?;
    let dirty = std::env::var("PHOENIX_BUILD_GIT_DIRTY").ok()?;
    let valid_sha = sha == "unknown" || is_valid_lowercase_sha(&sha);
    if !valid_sha {
        return None;
    }
    let suffix = match dirty.as_str() {
        "dirty" if sha != "unknown" => "-dirty",
        "clean" | "unknown" | "dirty" => "",
        _ => return None,
    };
    Some(format!("{sha}{suffix}"))
}

fn main() {
    // Embed the full git SHA into the binary so the UI can surface exactly
    // which build is running. Falls back to "unknown" when git isn't
    // available (e.g. tarball builds, where .git/ is absent).
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PHOENIX_BUILD_GIT_SHA");
    println!("cargo:rerun-if-env-changed=PHOENIX_BUILD_GIT_DIRTY");

    let value = if let Some(value) = dev_check_git_sha() {
        value
    } else {
        // Standalone and release builds probe live git state on every build so
        // unstaged edits anywhere in the repository (including embedded ui/dist
        // assets or untracked files) can never freeze a stale SHA or clean flag.
        println!("cargo:rerun-if-changed=__phoenix_git_sha_force_rerun__");
        let sha = git(&["rev-parse", "HEAD"])
            .filter(|sha| is_valid_lowercase_sha(sha))
            .unwrap_or_else(|| "unknown".to_string());
        let dirty = sha != "unknown"
            && git(&["status", "--porcelain=v1", "--untracked-files=normal"])
                .is_some_and(|s| !s.is_empty());
        let suffix = if dirty { "-dirty" } else { "" };
        format!("{sha}{suffix}")
    };

    println!("cargo:rustc-env=PHOENIX_GIT_SHA={value}");
}
