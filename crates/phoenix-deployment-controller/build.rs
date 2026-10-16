use std::{env, path::Path, process::Command};

fn git(manifest: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    let manifest = Path::new(&manifest);
    println!("cargo:rerun-if-changed={}", manifest.display());
    for workspace_file in ["../../Cargo.toml", "../../Cargo.lock"] {
        println!(
            "cargo:rerun-if-changed={}",
            manifest.join(workspace_file).display()
        );
    }
    for git_path in ["HEAD", "index"] {
        if let Some(path) = git(
            manifest,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                git_path,
            ],
        ) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(
            manifest,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                &reference,
            ],
        ) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let identity = (|| {
        let root = manifest.join("../..").canonicalize().ok()?;
        let git_root = Path::new(&git(manifest, &["rev-parse", "--show-toplevel"])?)
            .canonicalize()
            .ok()?;
        if git_root != root {
            return None;
        }
        let status = git(
            manifest,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--",
                ".",
                "../../Cargo.toml",
                "../../Cargo.lock",
            ],
        )?;
        if !status.is_empty() {
            return None;
        }
        let sha = git(manifest, &["rev-parse", "--verify", "HEAD"])?;
        (sha.len() == 40
            && sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then_some(sha)
    })()
    .unwrap_or_default();
    // An empty value is deliberately not an admissible identity (including in rsynced source).
    println!("cargo:rustc-env=PHOENIX_SDC_BUILD_ID={identity}");
}
