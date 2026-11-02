//! `SQLite` is the durable plugin activation authority.

use phoenix_core::plugin_activation::PluginActivation;
use phoenix_db::{Database, DbResult};

pub(crate) async fn refresh(db: &Database, activation: &PluginActivation) -> DbResult<()> {
    let records = db.get_plugins().await?;
    activation.publish(
        records
            .into_iter()
            .filter(|record| !record.enabled)
            .map(|record| record.name)
            .collect(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_refresh_covers_disable_reinstall_and_failed_reads() {
        let db = Database::open_in_memory().await.unwrap();
        let activation = PluginActivation::default();
        let skills = activation.clone();
        let mcp = activation.clone();
        assert!(!skills.snapshot().is_enabled("demo"));
        refresh(&db, &activation).await.unwrap();
        assert!(mcp.snapshot().is_enabled("demo"));
        db.set_plugin_enabled("demo", false).await.unwrap();
        refresh(&db, &activation).await.unwrap();
        assert!(!skills.snapshot().is_enabled("demo"));
        assert!(!mcp.snapshot().is_enabled("demo"));
        db.register_plugin("demo", std::path::Path::new("/plugins/demo"))
            .await
            .unwrap();
        refresh(&db, &activation).await.unwrap();
        assert!(skills.snapshot().is_enabled("demo"));
        assert!(mcp.snapshot().is_enabled("demo"));
        db.set_plugin_enabled("demo", false).await.unwrap();
        refresh(&db, &activation).await.unwrap();
        sqlx::query("DROP TABLE plugins")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(refresh(&db, &activation).await.is_err());
        assert!(!skills.snapshot().is_enabled("demo"));
        assert!(!mcp.snapshot().is_enabled("demo"));
        let unavailable = PluginActivation::default();
        assert!(refresh(&db, &unavailable).await.is_err());
        assert!(!unavailable.snapshot().is_enabled("demo"));
    }
}
