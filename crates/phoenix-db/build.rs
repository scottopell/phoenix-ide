use std::path::{Path, PathBuf};

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
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");

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
