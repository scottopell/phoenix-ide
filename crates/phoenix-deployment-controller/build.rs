use sha2::{Digest, Sha256};
use std::{env, fmt::Write as _, fs, path::Path};

fn collect_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.expect("read build-identity directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

fn main() {
    let manifest =
        std::path::PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let workspace = manifest
        .join("../..")
        .canonicalize()
        .expect("canonical workspace root");
    let watched = [
        manifest.join("Cargo.toml"),
        manifest.join("build.rs"),
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
    ];
    for path in &watched {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for directory in [manifest.join("src"), manifest.join("tests")] {
        println!("cargo:rerun-if-changed={}", directory.display());
    }

    let mut files = watched.to_vec();
    collect_files(&manifest.join("src"), &mut files);
    collect_files(&manifest.join("tests"), &mut files);
    files.sort();

    let mut digest = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(&workspace)
            .expect("build-identity input belongs to workspace");
        let bytes =
            fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(
            u64::try_from(bytes.len())
                .expect("input length fits u64")
                .to_be_bytes(),
        );
        digest.update(bytes);
    }
    let identity =
        digest
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut identity, byte| {
                write!(&mut identity, "{byte:02x}").expect("write digest to string");
                identity
            });
    println!("cargo:rustc-env=PHOENIX_SDC_BUILD_ID={identity}");
}
