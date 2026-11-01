//! Human names and allocated route projections share one transaction.

use sqlx::{Sqlite, Transaction};

use crate::DbResult;

pub(crate) fn slug_base(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "conversation".to_string()
    } else {
        slug.to_string()
    }
}

pub(crate) async fn allocate_slug_tx(
    tx: &mut Transaction<'_, Sqlite>,
    name: &str,
    transcript_id: &str,
) -> DbResult<String> {
    let base = slug_base(name);
    let mut candidate = base.clone();
    let mut suffix = 2_u64;
    loop {
        let occupied: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE slug = ?1 AND id <> ?2)
                 OR EXISTS(SELECT 1 FROM conversations WHERE id = ?1)
                 OR EXISTS(SELECT 1 FROM product_conversations WHERE id = ?1)
                 OR EXISTS(SELECT 1 FROM product_creation_jobs WHERE product_conversation_id = ?1)",
        )
        .bind(&candidate)
        .bind(transcript_id)
        .fetch_one(&mut **tx)
        .await?;
        if !occupied && candidate != transcript_id {
            return Ok(candidate);
        }
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
}

pub(crate) async fn migrate_name_authority(tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
    sqlx::query("ALTER TABLE conversations ADD COLUMN name_revision INTEGER NOT NULL DEFAULT 0 CHECK (typeof(name_revision) = 'integer' AND name_revision >= 0)")
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE conversations AS root
         SET title = COALESCE(NULLIF(root.chain_name, ''), NULLIF(root.title, ''),
                              NULLIF(root.slug, ''), 'Conversation')
         WHERE root.parent_conversation_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM conversations predecessor
               WHERE predecessor.product_conversation_id = root.product_conversation_id
                 AND predecessor.continued_in_conv_id = root.id)",
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query("ALTER TABLE conversations DROP COLUMN chain_name")
        .execute(&mut **tx)
        .await?;
    let roots: Vec<(String, String)> = sqlx::query_as(
        "SELECT root.id, root.title FROM conversations root
         JOIN product_conversations product ON product.id = root.product_conversation_id
         WHERE product.kind = 'ordinary' AND root.user_initiated = 1
           AND root.runtime_role = 'user' AND root.parent_conversation_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM conversations predecessor
               WHERE predecessor.product_conversation_id = root.product_conversation_id
                 AND predecessor.continued_in_conv_id = root.id)
         ORDER BY root.created_at, root.id",
    )
    .fetch_all(&mut **tx)
    .await?;
    // NULL releases every migrating root's former slug before deterministic allocation.
    for (id, _) in &roots {
        sqlx::query("UPDATE conversations SET slug = NULL WHERE id = ?1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    for (id, name) in roots {
        let slug = allocate_slug_tx(tx, &name, &id).await?;
        sqlx::query("UPDATE conversations SET slug = ?1 WHERE id = ?2")
            .bind(slug)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::slug_base;

    #[test]
    fn projection_preserves_unicode_words_and_has_a_punctuation_fallback() {
        assert_eq!(slug_base("Fix Full Words"), "fix-full-words");
        assert_eq!(slug_base("Über 東京 / Café"), "über-東京-café");
        assert_eq!(slug_base("! / ?"), "conversation");
        assert_eq!(slug_base("  Hello---World  "), "hello-world");
    }
    #[tokio::test]
    async fn concurrent_duplicate_names_allocate_atomically_and_release_old_routes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("names.db");
        let db = crate::Database::open(path.to_str().unwrap()).await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let other = crate::Database::open(path.to_str().unwrap()).await.unwrap();
        let first = db
            .create_conversation("first-id", "first-name", "/tmp", true, None, None)
            .await
            .unwrap();
        let second = db
            .create_conversation("second-id", "second-name", "/tmp", true, None, None)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            db.set_ordinary_product_conversation_title(&first.id, "Shared Human Name"),
            other.set_ordinary_product_conversation_title(&second.id, "Shared Human Name"),
        );
        a.unwrap();
        b.unwrap();
        let first_after = db.get_conversation(&first.id).await.unwrap();
        let second_after = db.get_conversation(&second.id).await.unwrap();
        assert_eq!(first_after.title.as_deref(), Some("Shared Human Name"));
        assert_eq!(first_after.title, second_after.title);
        let mut slugs = vec![first_after.slug.unwrap(), second_after.slug.unwrap()];
        slugs.sort();
        assert_eq!(slugs, ["shared-human-name", "shared-human-name-2"]);
        assert_eq!(
            first_after.product_conversation_id,
            first.product_conversation_id
        );
        assert!(db
            .resolve_ordinary_product_conversation("first-name")
            .await
            .is_err());
        let reused = db
            .create_conversation("third-id", "first-name", "/tmp", true, None, None)
            .await
            .unwrap();
        assert_eq!(reused.slug.as_deref(), Some("first-name"));
        assert_eq!(
            db.resolve_ordinary_product_conversation("first-name")
                .await
                .unwrap()
                .product_conversation_id,
            reused.product_conversation_id
        );
        assert_eq!(
            db.resolve_ordinary_product_conversation(first.product_conversation_id.as_str())
                .await
                .unwrap()
                .product_conversation_id,
            first.product_conversation_id
        );
    }

    #[tokio::test]
    async fn unicode_punctuation_and_actual_id_collisions_preserve_human_names() {
        let db = crate::Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("root-id", "starting-name", "/tmp", true, None, None)
            .await
            .unwrap();
        db.set_ordinary_product_conversation_title(&root.id, "  Über 東京 / Café  ")
            .await
            .unwrap();
        let renamed = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(renamed.title.as_deref(), Some("Über 東京 / Café"));
        assert_eq!(renamed.slug.as_deref(), Some("über-東京-café"));
        for (name, expected) in [
            ("! / ?", "conversation".to_string()),
            ("root-id", "root-id-2".to_string()),
            (
                root.product_conversation_id.as_str(),
                format!("{}-2", root.product_conversation_id),
            ),
            (
                "00000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000001".to_string(),
            ),
        ] {
            db.set_ordinary_product_conversation_title(&root.id, name)
                .await
                .unwrap();
            let renamed = db.get_conversation(&root.id).await.unwrap();
            assert_eq!(renamed.title.as_deref(), Some(name));
            assert_eq!(renamed.slug.as_deref(), Some(expected.as_str()));
        }
        let before = db.get_conversation(&root.id).await.unwrap();
        assert!(db
            .set_ordinary_product_conversation_title(&root.id, "   ")
            .await
            .is_err());
        let after = db.get_conversation(&root.id).await.unwrap();
        assert_eq!((before.title, before.slug), (after.title, after.slug));
    }

    #[tokio::test]
    async fn forward_migration_folds_effective_names_before_reallocating_without_history_loss() {
        use sqlx::Row;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE product_conversations(id TEXT PRIMARY KEY, kind TEXT NOT NULL);
             CREATE TABLE product_creation_jobs(product_conversation_id TEXT PRIMARY KEY);
             CREATE TABLE conversations(id TEXT PRIMARY KEY, product_conversation_id TEXT, slug TEXT UNIQUE,
                 title TEXT, chain_name TEXT, parent_conversation_id TEXT, continued_in_conv_id TEXT,
                 user_initiated INTEGER, runtime_role TEXT, created_at TEXT);
             CREATE TABLE messages(message_id TEXT PRIMARY KEY, conversation_id TEXT REFERENCES conversations(id), content TEXT);
             INSERT INTO product_conversations VALUES ('p-a','ordinary'),('p-b','ordinary'),('p-c','ordinary'),('p-d','ordinary');
             INSERT INTO product_creation_jobs VALUES ('slug-fallback');
             INSERT INTO conversations VALUES
                 ('a','p-a','old-a','Hidden','Shared Name',NULL,'a-next',1,'user','1'),
                 ('a-next','p-a','part-2','Part 2',NULL,NULL,NULL,1,'user','2'),
                 ('b','p-b','shared-name','Shared Name','',NULL,NULL,1,'user','2'),
                 ('c','p-c','slug-fallback',NULL,NULL,NULL,NULL,1,'user','3'),
                 ('d','p-d',NULL,NULL,NULL,NULL,NULL,1,'user','4');
             INSERT INTO messages VALUES ('exact-message','a','unchanged historical content');"
        ).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        super::migrate_name_authority(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id,title,slug FROM conversations WHERE id <> 'a-next' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![
                ("a".into(), "Shared Name".into(), "shared-name".into()),
                ("b".into(), "Shared Name".into(), "shared-name-2".into()),
                ("c".into(), "slug-fallback".into(), "slug-fallback-2".into()),
                ("d".into(), "Conversation".into(), "conversation".into()),
            ]
        );
        let evidence: (String, String, String, String) = sqlx::query_as("SELECT c.product_conversation_id,c.continued_in_conv_id,m.message_id,m.content FROM conversations c JOIN messages m ON m.conversation_id=c.id WHERE c.id='a'").fetch_one(&pool).await.unwrap();
        assert_eq!(
            evidence,
            (
                "p-a".into(),
                "a-next".into(),
                "exact-message".into(),
                "unchanged historical content".into()
            )
        );
        let columns = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(!columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "chain_name"));
        let successor: (String, String) =
            sqlx::query_as("SELECT title,slug FROM conversations WHERE id='a-next'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(successor, ("Part 2".into(), "part-2".into()));
    }

    #[tokio::test]
    async fn generated_name_cannot_overwrite_a_newer_name() {
        let db = crate::Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("generation-id", "original-name", "/tmp", true, None, None)
            .await
            .unwrap();
        let baseline = db
            .capture_conversation_name_baseline(&root.id)
            .await
            .unwrap();
        db.set_ordinary_product_conversation_title(&root.id, "Human Selected Name")
            .await
            .unwrap();
        let stale = db
            .set_ordinary_product_conversation_title_if_unchanged(
                root.product_conversation_id.as_str(),
                "Stale Generated Name",
                &baseline,
            )
            .await
            .unwrap();
        assert!(stale.is_none());
        let current = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(current.title.as_deref(), Some("Human Selected Name"));
        assert_eq!(current.slug.as_deref(), Some("human-selected-name"));
        db.set_ordinary_product_conversation_title(&root.id, root.title.as_deref().unwrap())
            .await
            .unwrap();
        let restored = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(restored.title, root.title);
        assert_eq!(restored.slug, root.slug);
        assert!(db
            .set_ordinary_product_conversation_title_if_unchanged(
                root.product_conversation_id.as_str(),
                "Stale ABA Name",
                &baseline,
            )
            .await
            .unwrap()
            .is_none());
        let fresh_baseline = db
            .capture_conversation_name_baseline(&root.id)
            .await
            .unwrap();
        assert!(db
            .set_ordinary_product_conversation_title_if_unchanged(
                root.product_conversation_id.as_str(),
                "Fresh Generated Name",
                &fresh_baseline,
            )
            .await
            .unwrap()
            .is_some());
        let generated = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(generated.title.as_deref(), Some("Fresh Generated Name"));
        assert_eq!(generated.slug.as_deref(), Some("fresh-generated-name"));
    }

    #[tokio::test]
    async fn active_close_fences_name_and_projection_together() {
        use phoenix_core::domain::close::TranscriptConversationId;
        let db = crate::Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("closing-id", "before-close", "/tmp", true, None, None)
            .await
            .unwrap();
        db.begin_close_foundation(
            &root.product_conversation_id,
            &TranscriptConversationId::parse(root.id.clone()).unwrap(),
            "name-close-attempt",
        )
        .await
        .unwrap();
        assert!(db
            .set_ordinary_product_conversation_title(&root.id, "Not Admitted")
            .await
            .is_err());
        let stored = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(stored.title, root.title);
        assert_eq!(stored.slug, root.slug);
    }
}
