use std::path::{Path, PathBuf};
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

fn resolve_git_path(rel: &str) -> Option<PathBuf> {
    let raw = git(&["rev-parse", "--git-path", rel])?;
    let path = PathBuf::from(raw);
    let abs = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    abs.exists().then_some(abs)
}

fn emit_rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    // Embed the full git SHA into the binary so the UI can surface exactly
    // which build is running. Falls back to "unknown" when git isn't
    // available (e.g. tarball builds, where .git/ is absent).
    //
    // Watch existing git metadata paths and workspace Rust sources instead of
    // a nonexistent sentinel path: in Cargo, any missing rerun-if-changed path
    // marks RunCustomBuild dirty and forces downstream rustc recompiles on
    // every cargo invocation even when rustc-env output is unchanged.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    if let Ok(entries) = std::fs::read_dir("../../crates") {
        let mut crate_dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        crate_dirs.sort();
        for crate_dir in crate_dirs {
            emit_rerun_if_exists(&crate_dir.join("src"));
            emit_rerun_if_exists(&crate_dir.join("Cargo.toml"));
        }
    }

    for git_path in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = resolve_git_path(git_path) {
            emit_rerun_if_exists(&path);
        }
    }
    if let Some(sym_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = resolve_git_path(&sym_ref) {
            emit_rerun_if_exists(&path);
        } else if let Some(path) = resolve_git_path("refs/heads") {
            emit_rerun_if_exists(&path);
        }
    }

    let sha = git(&["rev-parse", "HEAD"])
        .filter(|sha| {
            sha.len() == 40
                && sha
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .unwrap_or_else(|| "unknown".to_string());

    // "-dirty" suffix when the working tree has uncommitted changes, so a
    // binary built from modified sources can't masquerade as the commit it
    // was branched from. Only meaningful when the SHA itself resolved.
    let dirty = sha != "unknown" && git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());

    let suffix = if dirty { "-dirty" } else { "" };
    println!("cargo:rustc-env=PHOENIX_GIT_SHA={sha}{suffix}");
}
