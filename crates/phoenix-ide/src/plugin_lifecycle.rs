//! Plugin filesystem and registry mutations, serialized through activation refresh.

use std::path::{Path, PathBuf};

use phoenix_core::plugin_activation::PluginActivation;
use phoenix_db::Database;

#[derive(Default)]
pub(crate) struct PluginLifecycle {
    mutation: tokio::sync::Mutex<()>,
}

pub(crate) enum Mutation<'a> {
    Add(&'a Path),
    SetEnabled(&'a str, bool),
    Remove(&'a str),
    Purge(&'a str),
}

#[derive(Debug)]
pub(crate) struct LifecycleError {
    pub status: axum::http::StatusCode,
    pub message: String,
}

type Result<T> = std::result::Result<T, LifecycleError>;

impl LifecycleError {
    fn bad(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
    fn internal(message: &impl std::fmt::Display) -> Self {
        Self {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            message: message.to_string(),
        }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: message.into(),
        }
    }
}

struct PluginName<'a>(&'a str);
impl<'a> PluginName<'a> {
    fn parse(name: &'a str) -> Result<Self> {
        if phoenix_plugins::manifest::name_is_valid(name) {
            Ok(Self(name))
        } else {
            Err(LifecycleError::bad("invalid plugin name"))
        }
    }
}

enum Reconciliation<'a> {
    Live(&'a crate::tools::mcp::McpClientManager),
    #[cfg(test)]
    Offline,
    #[cfg(test)]
    Refuse,
}

impl Reconciliation<'_> {
    async fn apply(&self, name: &str) -> Result<()> {
        match self {
            Self::Live(manager) => {
                let result = manager.reload().await;
                let prefix = format!("{name}.");
                let errors: Vec<_> = result
                    .failed
                    .into_iter()
                    .filter(|failure| failure.server.starts_with(&prefix))
                    .map(|failure| format!("{}: {}", failure.server, failure.error))
                    .collect();
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(LifecycleError::internal(&format!(
                        "MCP reconciliation failed: {}",
                        errors.join("; ")
                    )))
                }
            }
            #[cfg(test)]
            Self::Offline => Ok(()),
            #[cfg(test)]
            Self::Refuse => Err(LifecycleError::internal(&"reconciliation refused")),
        }
    }
}

impl PluginLifecycle {
    pub(crate) async fn run(
        &self,
        db: &Database,
        manager: &crate::tools::mcp::McpClientManager,
        root: &Path,
        operation: Mutation<'_>,
    ) -> Result<String> {
        let _guard = self.mutation.lock().await;
        Self::mutate(
            db,
            manager.plugin_activation(),
            root,
            operation,
            &Reconciliation::Live(manager),
        )
        .await
    }

    #[cfg(test)]
    async fn mutate_and_refresh(
        db: &Database,
        activation: &PluginActivation,
        root: &Path,
        operation: Mutation<'_>,
    ) -> Result<String> {
        Self::mutate(db, activation, root, operation, &Reconciliation::Offline).await
    }

    async fn mutate(
        db: &Database,
        activation: &PluginActivation,
        root: &Path,
        operation: Mutation<'_>,
        reconciliation: &Reconciliation<'_>,
    ) -> Result<String> {
        let name = match operation {
            Mutation::Add(source) => install(db, root, source).await?,
            Mutation::SetEnabled(name, enabled) => {
                let name = PluginName::parse(name)?;
                if let Some(plugin) = phoenix_plugins::scan_plugins_root(root)
                    .into_iter()
                    .find(|plugin| plugin.manifest.name == name.0)
                {
                    db.record_plugin_entry(name.0, &plugin.entry_dir, enabled)
                        .await
                        .map_err(|error| LifecycleError::internal(&error))?;
                } else {
                    db.set_plugin_enabled(name.0, enabled)
                        .await
                        .map_err(|error| LifecycleError::internal(&error))?;
                }
                name.0.to_owned()
            }
            Mutation::Remove(name) => {
                let name = PluginName::parse(name)?;
                let plugin = phoenix_plugins::scan_plugins_root(root)
                    .into_iter()
                    .find(|plugin| plugin.manifest.name == name.0)
                    .ok_or_else(|| LifecycleError::bad("plugin is not installed"))?;
                db.record_plugin_entry(name.0, &plugin.entry_dir, false)
                    .await
                    .map_err(|error| LifecycleError::internal(&error))?;
                crate::plugin_activation::refresh(db, activation)
                    .await
                    .map_err(|error| LifecycleError::internal(&error))?;
                reconciliation.apply(name.0).await?;
                remove_entry(&plugin.entry_dir)?;
                name.0.to_owned()
            }
            Mutation::Purge(name) => {
                let name = PluginName::parse(name)?;
                let records = db
                    .get_plugins()
                    .await
                    .map_err(|error| LifecycleError::internal(&error))?;
                let record = records
                    .iter()
                    .find(|record| record.name == name.0)
                    .ok_or_else(|| LifecycleError::bad("no retained plugin identity"))?;
                let entry = record.install_entry.as_deref().ok_or_else(|| {
                    LifecycleError::bad(
                        "installation entry is unknown; cannot prove plugin absence",
                    )
                })?;
                match std::fs::symlink_metadata(entry) {
                    Ok(_) => {
                        return Err(LifecycleError::bad(
                            "installation entry still exists; remove it before purging data",
                        ))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(LifecycleError::internal(&error)),
                }
                if phoenix_plugins::scan_plugins_root(root)
                    .iter()
                    .any(|plugin| plugin.manifest.name == name.0)
                {
                    return Err(LifecycleError::bad(
                        "remove the installed plugin before purging data",
                    ));
                }
                db.set_plugin_enabled(name.0, false)
                    .await
                    .map_err(|error| LifecycleError::internal(&error))?;
                crate::plugin_activation::refresh(db, activation)
                    .await
                    .map_err(|error| LifecycleError::internal(&error))?;
                reconciliation.apply(name.0).await?;
                let data = owned_data_dir(root, &name)?;
                match std::fs::remove_dir_all(&data) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(LifecycleError::internal(&error)),
                }
                db.unregister_plugin(name.0)
                    .await
                    .map_err(|error| LifecycleError::internal(&error))?;
                name.0.to_owned()
            }
        };
        crate::plugin_activation::refresh(db, activation)
            .await
            .map_err(|error| LifecycleError::internal(&error))?;
        reconciliation.apply(&name).await?;
        Ok(name)
    }
}

fn remove_entry(entry: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(entry).map_err(|error| LifecycleError::internal(&error))?;
    if metadata.file_type().is_symlink() {
        std::fs::remove_file(entry)
    } else {
        std::fs::remove_dir_all(entry)
    }
    .map_err(|error| LifecycleError::internal(&error))
}

fn owned_data_dir(root: &Path, name: &PluginName<'_>) -> Result<PathBuf> {
    let root = std::fs::canonicalize(root).map_err(|error| LifecycleError::internal(&error))?;
    let boundary = root.join("data");
    let data = boundary.join(name.0);
    for ancestor in [&boundary, &data] {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(LifecycleError::bad("plugin data path contains a symlink"))
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(LifecycleError::bad("plugin data path is not a directory"))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(LifecycleError::internal(&error)),
        }
    }
    Ok(data)
}

async fn install(db: &Database, root: &Path, source: &Path) -> Result<String> {
    let plugin = phoenix_plugins::load_plugin(source).map_err(LifecycleError::bad)?;
    let name = PluginName::parse(&plugin.manifest.name)?;
    if phoenix_plugins::scan_plugins_root(root)
        .iter()
        .any(|plugin| plugin.manifest.name == name.0)
    {
        return Err(LifecycleError::conflict("plugin is already installed"));
    }
    std::fs::create_dir_all(root).map_err(|error| LifecycleError::internal(&error))?;
    // `data` is reserved for retained state, but is a valid manifest identity.
    let entry = if name.0 == "data" {
        ".phoenix-plugin-data"
    } else {
        name.0
    };
    let destination = root.join(entry);
    let staging = tempfile::Builder::new()
        .prefix(".phoenix-install-")
        .tempdir_in(root)
        .map_err(|error| LifecycleError::internal(&error))?;
    let payload = staging.path().join("payload");
    let output = tokio::process::Command::new("cp")
        .arg("-a")
        .arg(&plugin.root)
        .arg(&payload)
        .output()
        .await
        .map_err(|error| LifecycleError::internal(&error))?;
    if !output.status.success() {
        return Err(LifecycleError::internal(&format!(
            "copy failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let staged = phoenix_plugins::load_plugin(&payload).map_err(LifecycleError::bad)?;
    if staged.manifest.name != name.0 {
        return Err(LifecycleError::bad("plugin identity changed while copying"));
    }
    publish_exclusive(&payload, &destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            LifecycleError::conflict("installation entry already exists")
        } else {
            LifecycleError::internal(&error)
        }
    })?;
    db.register_plugin(name.0, &destination)
        .await
        .map_err(|error| {
            LifecycleError::internal(&format!(
                "plugin files installed, but registration failed: {error}"
            ))
        })?;
    Ok(name.0.to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn publish_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    // Both C strings remain alive through the syscall. No replacement is permitted.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn publish_exclusive(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "exclusive plugin publication is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(path: &Path, name: &str) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("plugin.json"), format!(r#"{{"$schema":"https://agent-plugins.org/schemas/1.1.0/plugin.schema.json","name":"{name}"}}"#)).unwrap();
        std::fs::write(path.join("witness"), "preserve").unwrap();
    }

    async fn apply(
        db: &Database,
        activation: &PluginActivation,
        root: &Path,
        mutation: Mutation<'_>,
    ) -> Result<String> {
        PluginLifecycle::mutate_and_refresh(db, activation, root, mutation).await
    }

    #[tokio::test]
    async fn install_remove_purge_retains_identity_until_data_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("missing/plugins");
        let source = base.join("source");
        bundle(&source, "demo");
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        apply(&db, &activation, &root, Mutation::Add(&source))
            .await
            .unwrap();
        assert!(activation.snapshot().is_enabled("demo"));
        assert_eq!(phoenix_plugins::scan_plugins_root(&root).len(), 1);
        let data = root.join("data/demo");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("state"), "retained").unwrap();
        apply(&db, &activation, &root, Mutation::Remove("demo"))
            .await
            .unwrap();
        assert!(phoenix_plugins::scan_plugins_root(&root).is_empty());
        assert!(data.join("state").is_file());
        assert_eq!(db.get_plugins().await.unwrap()[0].name, "demo");
        apply(&db, &activation, &root, Mutation::Purge("demo"))
            .await
            .unwrap();
        assert!(!data.exists());
        assert!(db.get_plugins().await.unwrap().is_empty());
        assert!(source.join("witness").is_file());
    }

    #[tokio::test]
    async fn reconciliation_failure_preserves_entry_and_data_with_disabled_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "demo");
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        apply(&db, &activation, &root, Mutation::Add(&source))
            .await
            .unwrap();
        let data = root.join("data/demo");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("witness"), "safe").unwrap();
        let error = PluginLifecycle::mutate(
            &db,
            &activation,
            &root,
            Mutation::Remove("demo"),
            &Reconciliation::Refuse,
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("reconciliation"));
        assert!(root.join("demo/plugin.json").is_file());
        assert!(!activation.snapshot().is_enabled("demo"));
        assert!(!db.get_plugins().await.unwrap()[0].enabled);
        std::fs::remove_dir_all(root.join("demo")).unwrap();
        assert!(PluginLifecycle::mutate(
            &db,
            &activation,
            &root,
            Mutation::Purge("demo"),
            &Reconciliation::Refuse
        )
        .await
        .is_err());
        assert!(data.join("witness").is_file());
        assert_eq!(db.get_plugins().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reinstall_refreshes_the_shared_disabled_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "demo");
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        apply(&db, &activation, &root, Mutation::Add(&source))
            .await
            .unwrap();
        apply(&db, &activation, &root, Mutation::SetEnabled("demo", false))
            .await
            .unwrap();
        assert!(!activation.snapshot().is_enabled("demo"));
        std::fs::remove_dir_all(root.join("demo")).unwrap();
        apply(&db, &activation, &root, Mutation::Add(&source))
            .await
            .unwrap();
        assert!(activation.snapshot().is_enabled("demo"));
    }

    #[tokio::test]
    async fn invalid_identity_cannot_write_registry_or_delete_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("witness"), "safe").unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        for name in [
            outside.to_str().unwrap(),
            "../outside",
            "..",
            "x/y",
            "",
            "Bad",
        ] {
            for operation in [
                Mutation::Remove(name),
                Mutation::Purge(name),
                Mutation::SetEnabled(name, false),
                Mutation::SetEnabled(name, true),
            ] {
                let error = apply(&db, &activation, &root, operation).await.unwrap_err();
                assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
            }
        }
        assert!(outside.join("witness").is_file());
        assert!(!root.exists());
        assert!(db.get_plugins().await.unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn removing_unmanaged_symlink_preserves_its_source_and_records_retained_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "demo");
        std::fs::create_dir(&root).unwrap();
        let entry = root.join("renamed-entry");
        std::os::unix::fs::symlink(&source, &entry).unwrap();
        let db = Database::open_in_memory().await.unwrap();
        apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Remove("demo"),
        )
        .await
        .unwrap();
        assert!(std::fs::symlink_metadata(&entry).is_err());
        assert!(source.join("witness").is_file());
        let rows = db.get_plugins().await.unwrap();
        assert_eq!(rows[0].provenance, phoenix_db::PluginProvenance::Unmanaged);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn purge_requires_retained_identity_and_rejects_symlinked_data_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("plugins");
        let outside = base.join("outside");
        std::fs::create_dir_all(outside.join("demo")).unwrap();
        std::fs::write(outside.join("demo/witness"), "safe").unwrap();
        std::fs::create_dir(&root).unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        assert!(apply(&db, &activation, &root, Mutation::Purge("demo"))
            .await
            .is_err());
        db.record_plugin_entry("demo", &root.join("demo"), false)
            .await
            .unwrap();
        std::os::unix::fs::symlink(&outside, root.join("data")).unwrap();
        assert!(apply(&db, &activation, &root, Mutation::Purge("demo"))
            .await
            .is_err());
        std::fs::remove_file(root.join("data")).unwrap();
        std::fs::create_dir(root.join("data")).unwrap();
        std::os::unix::fs::symlink(outside.join("demo"), root.join("data/demo")).unwrap();
        assert!(apply(&db, &activation, &root, Mutation::Purge("demo"))
            .await
            .is_err());
        assert!(outside.join("demo/witness").is_file());
        assert_eq!(db.get_plugins().await.unwrap().len(), 1);
        std::fs::remove_file(root.join("data/demo")).unwrap();
        std::fs::create_dir_all(root.join("data/demo")).unwrap();
        std::fs::write(root.join("data/demo/state"), "retained").unwrap();
        std::os::unix::fs::symlink(&root, base.join("alias")).unwrap();
        apply(
            &db,
            &activation,
            &base.join("alias"),
            Mutation::Purge("demo"),
        )
        .await
        .unwrap();
        assert!(!root.join("data/demo").exists());
        assert!(db.get_plugins().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_manifest_and_unknown_entry_cannot_authorize_purge() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "demo");
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        apply(&db, &activation, &root, Mutation::Add(&source))
            .await
            .unwrap();
        let data = root.join("data/demo");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("state"), "retained").unwrap();
        std::fs::write(root.join("demo/plugin.json"), "malformed update").unwrap();
        assert!(phoenix_plugins::scan_plugins_root(&root).is_empty());
        let error = apply(&db, &activation, &root, Mutation::Purge("demo"))
            .await
            .unwrap_err();
        assert!(error.message.contains("entry still exists"));
        assert!(data.join("state").is_file());
        assert_eq!(db.get_plugins().await.unwrap().len(), 1);
        #[cfg(unix)]
        {
            std::fs::remove_dir_all(root.join("demo")).unwrap();
            std::os::unix::fs::symlink(root.join("absent-target"), root.join("demo")).unwrap();
            let error = apply(&db, &activation, &root, Mutation::Purge("demo"))
                .await
                .unwrap_err();
            assert!(error.message.contains("entry still exists"));
            assert!(data.join("state").is_file());
        }
        db.set_plugin_enabled("unknown", true).await.unwrap();
        let error = apply(&db, &activation, &root, Mutation::Purge("unknown"))
            .await
            .unwrap_err();
        assert!(error.message.contains("entry is unknown"));
        assert_eq!(db.get_plugins().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn removal_disables_duplicate_siblings_and_external_restore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        bundle(&root.join("first"), "demo");
        bundle(&root.join("second"), "demo");
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        let original = phoenix_plugins::scan_plugins_root(&root)
            .remove(0)
            .entry_dir;
        db.register_plugin("demo", &original).await.unwrap();
        apply(&db, &activation, &root, Mutation::Remove("demo"))
            .await
            .unwrap();
        assert!(!original.exists());
        assert_eq!(phoenix_plugins::scan_plugins_root(&root).len(), 1);
        assert!(!activation.snapshot().is_enabled("demo"));
        let row = &db.get_plugins().await.unwrap()[0];
        assert!(!row.enabled);
        assert_eq!(row.provenance, phoenix_db::PluginProvenance::Managed);
        assert_eq!(row.install_entry.as_deref(), original.to_str());
        assert!(apply(&db, &activation, &root, Mutation::Purge("demo"))
            .await
            .is_err());
        bundle(&original, "demo");
        crate::plugin_activation::refresh(&db, &activation)
            .await
            .unwrap();
        assert!(!activation.snapshot().is_enabled("demo"));
    }

    #[tokio::test]
    async fn failed_removal_registry_write_keeps_the_installation_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let entry = root.join("renamed");
        bundle(&entry, "demo");
        let db = Database::open_in_memory().await.unwrap();
        sqlx::query("CREATE TRIGGER reject_remove BEFORE INSERT ON plugins BEGIN SELECT RAISE(ABORT, 'reject removal'); END")
            .execute(db.pool()).await.unwrap();
        let error = apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Remove("demo"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(entry.join("witness").is_file());
        assert!(db.get_plugins().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn enablement_records_unmanaged_entry_and_remove_preserves_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let entry = root.join("renamed");
        bundle(&entry, "demo");
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        apply(&db, &activation, &root, Mutation::SetEnabled("demo", true))
            .await
            .unwrap();
        let row = &db.get_plugins().await.unwrap()[0];
        assert_eq!(row.install_entry.as_deref(), entry.to_str());
        assert_eq!(row.provenance, phoenix_db::PluginProvenance::Unmanaged);
        apply(&db, &activation, &root, Mutation::Remove("demo"))
            .await
            .unwrap();
        assert!(!activation.snapshot().is_enabled("demo"));
        assert!(!db.get_plugins().await.unwrap()[0].enabled);
    }

    #[tokio::test]
    async fn installation_never_replaces_or_nests_into_an_existing_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "demo");
        let destination = root.join("demo");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("unmanaged"), "safe").unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let error = apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Add(&source),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 1);
        assert!(destination.join("unmanaged").is_file());
        assert!(db.get_plugins().await.unwrap().is_empty());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn data_identity_installs_outside_the_reserved_data_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "data");
        let db = Database::open_in_memory().await.unwrap();
        apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Add(&source),
        )
        .await
        .unwrap();
        let plugins = phoenix_plugins::scan_plugins_root(&root);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].manifest.name, "data");
        assert_ne!(plugins[0].entry_dir, root.join("data"));
    }

    #[tokio::test]
    async fn failed_forget_keeps_the_registry_identity_after_data_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("plugins");
        let data = root.join("data/demo");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("state"), "retained").unwrap();
        let db = Database::open_in_memory().await.unwrap();
        db.register_plugin("demo", &root.join("demo"))
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER reject_forget BEFORE DELETE ON plugins BEGIN SELECT RAISE(ABORT, 'reject forget'); END")
            .execute(db.pool()).await.unwrap();
        let error = apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Purge("demo"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!data.exists());
        assert_eq!(db.get_plugins().await.unwrap().len(), 1);
        sqlx::query("DROP TRIGGER reject_forget")
            .execute(db.pool())
            .await
            .unwrap();
        apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Purge("demo"),
        )
        .await
        .unwrap();
        assert!(db.get_plugins().await.unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn existing_symlink_destination_is_not_followed_or_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        let outside = temp.path().join("outside");
        bundle(&source, "demo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("witness"), "safe").unwrap();
        let destination = root.join("demo");
        std::os::unix::fs::symlink(&outside, &destination).unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let error = apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Add(&source),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert!(std::fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        assert!(db.get_plugins().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn registration_failure_is_not_reported_as_install_success() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let source = temp.path().join("source");
        bundle(&source, "demo");
        let db = Database::open_in_memory().await.unwrap();
        sqlx::query("DROP TABLE plugins")
            .execute(db.pool())
            .await
            .unwrap();
        let error = apply(
            &db,
            &PluginActivation::default(),
            &root,
            Mutation::Add(&source),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(error
            .message
            .contains("files installed, but registration failed"));
        assert!(root.join("demo/plugin.json").is_file());
    }

    #[test]
    fn staging_is_not_discoverable_and_exclusive_publication_has_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let stage1 = tempfile::tempdir_in(root).unwrap();
        let stage2 = tempfile::tempdir_in(root).unwrap();
        bundle(&stage1.path().join("payload"), "demo");
        bundle(&stage2.path().join("payload"), "demo");
        assert!(phoenix_plugins::scan_plugins_root(root).is_empty());
        let destination = root.join("demo");
        std::thread::scope(|scope| {
            let first =
                scope.spawn(|| publish_exclusive(&stage1.path().join("payload"), &destination));
            let second =
                scope.spawn(|| publish_exclusive(&stage2.path().join("payload"), &destination));
            let results = [first.join().unwrap(), second.join().unwrap()];
            assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
            assert_eq!(
                results
                    .iter()
                    .filter(|result| result
                        .as_ref()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists))
                    .count(),
                1
            );
        });
        assert_eq!(phoenix_plugins::scan_plugins_root(root).len(), 1);
    }

    #[tokio::test]
    async fn instance_gate_is_held_before_mutation_and_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let owner = PluginLifecycle::default();
        let manager = crate::tools::mcp::McpClientManager::new();
        let guard = owner.mutation.lock().await;
        let mut operation = Box::pin(owner.run(
            &db,
            &manager,
            temp.path(),
            Mutation::SetEnabled("../bad", true),
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut operation)
                .await
                .is_err()
        );
        assert!(db.get_plugins().await.unwrap().is_empty());
        drop(guard);
        assert!(operation.await.is_err());
    }
}
