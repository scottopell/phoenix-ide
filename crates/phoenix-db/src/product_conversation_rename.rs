use phoenix_core::domain::product_conversation::ProductConversationId;
use serde::Serialize;
use sqlx::{Connection, Row};

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedConversationTitle(String);

impl NormalizedConversationTitle {
    /// # Errors
    /// Returns an error when the trimmed title is empty or exceeds 200 characters.
    pub fn parse(value: &str) -> DbResult<Self> {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > 200 {
            return Err(DbError::ConversationTitleInvalid);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProductConversationRenameSource {
    pub product_conversation_id: ProductConversationId,
    pub transcript_id: String,
    pub message_id: String,
    pub tool_use_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductConversationRenameCommand {
    pub operation_id: String,
    pub product_conversation_id: ProductConversationId,
    pub expected_root_transcript_id: String,
    pub expected_name_revision: i64,
    pub title: NormalizedConversationTitle,
    pub source: ProductConversationRenameSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProductConversationRenameReceipt {
    pub operation_id: String,
    pub product_conversation_id: ProductConversationId,
    pub title: String,
    pub slug_at_commit: String,
    pub naming_root_transcript_id: String,
    pub name_revision_before: i64,
    pub committed_at_unix_micros: i64,
    pub source: ProductConversationRenameSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductConversationRenameResult {
    Applied(ProductConversationRenameReceipt),
    Replayed(ProductConversationRenameReceipt),
    Stale,
    Conflict,
    NotFound,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProductConversationRenameDetail {
    pub can_rename: bool,
    pub product_conversation_id: ProductConversationId,
    pub root_transcript_id: String,
    pub name_revision: i64,
    pub title: Option<String>,
    pub slug: Option<String>,
}

impl Database {
    /// # Errors
    /// Returns an error if Coordinator authority, conversation resolution, or database access fails.
    pub async fn read_product_conversation_rename_detail(
        &self,
        transcript_id: &str,
        reference: &str,
    ) -> DbResult<ProductConversationRenameDetail> {
        let mut tx = self.pool.begin().await?;
        require_current_gc_tx(&mut tx, transcript_id).await?;
        let resolved = Self::resolve_ordinary_product_conversation_on(&mut tx, reference).await?;
        let row = sqlx::query("SELECT id, title, slug, name_revision, (archived = 0 AND name_revision < 9223372036854775807 AND EXISTS (SELECT 1 FROM product_conversations p WHERE p.id = root.product_conversation_id AND p.ordinary_lifecycle = 'open') AND NOT EXISTS (SELECT 1 FROM close_obligations o WHERE o.product_conversation_id = root.product_conversation_id AND (o.phase <> 'completed' OR o.close_outcome = 'close_incomplete'))) AS can_rename FROM conversations root WHERE product_conversation_id = ?1 AND runtime_role = 'user' AND user_initiated = 1 AND parent_conversation_id IS NULL AND NOT EXISTS (SELECT 1 FROM conversations predecessor WHERE predecessor.product_conversation_id = root.product_conversation_id AND predecessor.continued_in_conv_id = root.id) ORDER BY id LIMIT 1")
            .bind(resolved.product_conversation_id.as_str()).fetch_optional(&mut *tx).await?
            .ok_or_else(|| DbError::ConversationNotFound(reference.to_string()))?;
        let detail = ProductConversationRenameDetail {
            can_rename: row.try_get("can_rename")?,
            product_conversation_id: resolved.product_conversation_id,
            root_transcript_id: row.try_get("id")?,
            name_revision: row.try_get("name_revision")?,
            title: row.try_get("title")?,
            slug: row.try_get("slug")?,
        };
        tx.commit().await?;
        Ok(detail)
    }

    /// Checks persisted role and singleton-head authority for a caller-selected transcript.
    /// # Errors
    /// Returns an error when the transcript is not the current Coordinator or the query fails.
    pub async fn require_current_coordinator_transcript(
        &self,
        transcript_id: &str,
    ) -> DbResult<()> {
        let allowed: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1 AND runtime_role = 'coordinator' AND coordinator_head = 1)",
        ).bind(transcript_id).fetch_one(&self.pool).await?;
        if allowed == 1 {
            Ok(())
        } else {
            Err(DbError::CurrentCoordinatorRequired)
        }
    }

    /// Reads one receipt only after validating the current persisted Coordinator authority.
    /// # Errors
    /// Returns an error if Coordinator authority, operation ID validation, or database access fails.
    pub async fn read_product_conversation_rename_receipt(
        &self,
        transcript_id: &str,
        operation_id: &str,
    ) -> DbResult<Option<ProductConversationRenameReceipt>> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin().await?;
        let result = async {
            require_current_gc_tx(&mut tx, transcript_id).await?;
            validate_operation_id(operation_id)?;
            let row = sqlx::query(
                "SELECT * FROM product_conversation_rename_receipts WHERE operation_id = ?1",
            )
            .bind(operation_id)
            .fetch_optional(&mut *tx)
            .await?;
            row.as_ref().map(decode_receipt).transpose()
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    /// Atomically performs a root-identity/revision-fenced rename and records its durable receipt.
    /// # Errors
    /// Returns an error if command validation, Coordinator authority, or database access fails.
    pub async fn rename_product_conversation(
        &self,
        current_gc_transcript_id: &str,
        command: &ProductConversationRenameCommand,
    ) -> DbResult<ProductConversationRenameResult> {
        validate_command(command)?;
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        let result = async {
            require_current_gc_tx(&mut tx, current_gc_transcript_id).await?;
            if command.source.transcript_id != current_gc_transcript_id {
                return Err(DbError::CurrentCoordinatorRequired);
            }
            let source_owner: Option<String> = sqlx::query_scalar(
                "SELECT product_conversation_id FROM conversations WHERE id = ?1",
            )
            .bind(&command.source.transcript_id)
            .fetch_optional(&mut *tx)
            .await?;
            if source_owner.as_deref() != Some(command.source.product_conversation_id.as_str()) {
                return Ok(ProductConversationRenameResult::Conflict);
            }
            if let Some(row) = sqlx::query("SELECT * FROM product_conversation_rename_receipts WHERE operation_id = ?1")
                .bind(&command.operation_id).fetch_optional(&mut *tx).await? {
                let receipt = decode_receipt(&row)?;
                return Ok(if same_intent(&receipt, command) { ProductConversationRenameResult::Replayed(receipt) } else { ProductConversationRenameResult::Conflict });
            }
            let resolved = Self::resolve_ordinary_product_conversation_on(&mut tx, command.product_conversation_id.as_str()).await;
            let resolved = match resolved { Ok(value) => value, Err(DbError::ConversationNotFound(_)) => return Ok(ProductConversationRenameResult::NotFound), Err(DbError::ProductConversationUnavailable(_)) => return Ok(ProductConversationRenameResult::Unavailable), Err(error) => return Err(error) };
            crate::close_foundation::require_product_conversation_admission_tx(&mut tx, &resolved.requested_transcript_row_id).await?;
            let root = sqlx::query("SELECT id, name_revision FROM conversations WHERE product_conversation_id = ?1 AND runtime_role = 'user' AND user_initiated = 1 AND parent_conversation_id IS NULL AND NOT EXISTS (SELECT 1 FROM conversations p WHERE p.product_conversation_id = conversations.product_conversation_id AND p.continued_in_conv_id = conversations.id) ORDER BY id LIMIT 1")
                .bind(command.product_conversation_id.as_str()).fetch_optional(&mut *tx).await?;
            let Some(root) = root else { return Ok(ProductConversationRenameResult::NotFound); };
            let root_id: String = root.try_get("id")?;
            let revision: i64 = root.try_get("name_revision")?;
            if root_id != command.expected_root_transcript_id || revision != command.expected_name_revision {
                return Ok(ProductConversationRenameResult::Stale);
            }
            let slug = crate::product_conversation_rename::apply_conversation_name_tx(
                &mut tx,
                &command.product_conversation_id,
                &root_id,
                Some(revision),
                command.title.as_str(),
                &chrono::Utc::now().to_rfc3339(),
            )
            .await?;
            let Some(slug) = slug else { return Ok(ProductConversationRenameResult::Stale); };
            let now = chrono::Utc::now().timestamp_micros();
            sqlx::query("INSERT INTO product_conversation_rename_receipts (operation_id, product_conversation_id, name_revision_before, title, source_product_conversation_id, source_transcript_id, source_message_id, source_tool_use_id, naming_root_transcript_id, slug_at_commit, committed_at_unix_micros) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)")
                .bind(&command.operation_id).bind(command.product_conversation_id.as_str()).bind(revision).bind(command.title.as_str()).bind(command.source.product_conversation_id.as_str()).bind(&command.source.transcript_id).bind(&command.source.message_id).bind(&command.source.tool_use_id).bind(&root_id).bind(&slug).bind(now).execute(&mut *tx).await?;
            Ok(ProductConversationRenameResult::Applied(ProductConversationRenameReceipt { operation_id: command.operation_id.clone(), product_conversation_id: command.product_conversation_id.clone(), title: command.title.as_str().into(), slug_at_commit: slug, naming_root_transcript_id: root_id, name_revision_before: revision, committed_at_unix_micros: now, source: command.source.clone() }))
        }.await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
}

fn validate_operation_id(operation_id: &str) -> DbResult<()> {
    let parsed =
        uuid::Uuid::parse_str(operation_id).map_err(|_| DbError::ConversationTitleInvalid)?;
    if parsed.hyphenated().to_string() != operation_id {
        return Err(DbError::ConversationTitleInvalid);
    }
    Ok(())
}

fn validate_command(command: &ProductConversationRenameCommand) -> DbResult<()> {
    validate_operation_id(&command.operation_id)?;
    if command.expected_name_revision < 0
        || command.expected_name_revision == i64::MAX
        || command.expected_root_transcript_id.is_empty()
        || command.source.transcript_id.is_empty()
        || command.source.message_id.is_empty()
        || command.source.tool_use_id.is_empty()
    {
        return Err(DbError::ConversationTitleInvalid);
    }
    Ok(())
}

pub(crate) async fn require_current_gc_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transcript_id: &str,
) -> DbResult<()> {
    let allowed: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1 AND runtime_role = 'coordinator' AND coordinator_head = 1)")
        .bind(transcript_id).fetch_one(&mut **tx).await?;
    if allowed == 1 {
        Ok(())
    } else {
        Err(DbError::CurrentCoordinatorRequired)
    }
}

/// Applies a title, slug, and revision increment inside the caller's transaction.
/// Returns `None` when owner/root/revision no longer identifies the captured naming baseline.
pub(crate) async fn apply_conversation_name_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner: &ProductConversationId,
    root_id: &str,
    expected_revision: Option<i64>,
    title: &str,
    updated_at: &str,
) -> DbResult<Option<String>> {
    let row = sqlx::query(
        "SELECT product_conversation_id, name_revision FROM conversations WHERE id = ?1",
    )
    .bind(root_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let actual_owner: String = row.try_get("product_conversation_id")?;
    let revision: i64 = row.try_get("name_revision")?;
    if actual_owner != owner.as_str()
        || expected_revision.is_some_and(|expected| expected != revision)
        || revision == i64::MAX
    {
        return Ok(None);
    }
    let slug = crate::conversation_name::allocate_slug_tx(tx, title, root_id).await?;
    let updated = sqlx::query("UPDATE conversations SET title = ?1, slug = ?2, updated_at = ?3, name_revision = name_revision + 1 WHERE id = ?4 AND product_conversation_id = ?5 AND name_revision = ?6")
        .bind(title).bind(&slug).bind(updated_at).bind(root_id).bind(owner.as_str()).bind(revision)
        .execute(&mut **tx).await?;
    if updated.rows_affected() != 1 {
        return Ok(None);
    }
    Ok(Some(slug))
}

fn same_intent(
    receipt: &ProductConversationRenameReceipt,
    command: &ProductConversationRenameCommand,
) -> bool {
    receipt.product_conversation_id == command.product_conversation_id
        && receipt.naming_root_transcript_id == command.expected_root_transcript_id
        && receipt.name_revision_before == command.expected_name_revision
        && receipt.title == command.title.as_str()
        && receipt.source.product_conversation_id == command.source.product_conversation_id
        && receipt.source.transcript_id == command.source.transcript_id
}

fn decode_receipt(row: &sqlx::sqlite::SqliteRow) -> DbResult<ProductConversationRenameReceipt> {
    let product = |key: &str| -> DbResult<ProductConversationId> {
        row.try_get::<String, _>(key)?
            .parse::<ProductConversationId>()
            .map_err(|e| DbError::Serialization(e.to_string()))
    };
    Ok(ProductConversationRenameReceipt {
        operation_id: row.try_get("operation_id")?,
        product_conversation_id: product("product_conversation_id")?,
        title: row.try_get("title")?,
        slug_at_commit: row.try_get("slug_at_commit")?,
        naming_root_transcript_id: row.try_get("naming_root_transcript_id")?,
        name_revision_before: row.try_get("name_revision_before")?,
        committed_at_unix_micros: row.try_get("committed_at_unix_micros")?,
        source: ProductConversationRenameSource {
            product_conversation_id: product("source_product_conversation_id")?,
            transcript_id: row.try_get("source_transcript_id")?,
            message_id: row.try_get("source_message_id")?,
            tool_use_id: row.try_get("source_tool_use_id")?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_canonical_and_revision_cannot_overflow() {
        let id = uuid::Uuid::new_v4().hyphenated().to_string();
        let mut command = test_command(id.clone(), 0);
        assert!(validate_command(&command).is_ok());
        command.operation_id = uuid::Uuid::parse_str(&id)
            .unwrap()
            .to_string()
            .to_uppercase();
        assert!(validate_command(&command).is_err());
        command.operation_id = id;
        command.expected_name_revision = i64::MAX;
        assert!(validate_command(&command).is_err());
    }

    #[tokio::test]
    async fn unauthorized_read_does_not_require_receipt_table() {
        let db = Database::open_in_memory().await.unwrap();
        let result = db
            .read_product_conversation_rename_receipt("not-a-coordinator", "op")
            .await;
        assert!(matches!(result, Err(DbError::CurrentCoordinatorRequired)));
    }

    #[tokio::test]
    async fn rename_uses_title_shared_revision_and_historical_receipt() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("rename-root", "rename-root", "/tmp", true, None, None)
            .await
            .unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let detail = db
            .read_product_conversation_rename_detail(&gc.id, root.product_conversation_id.as_str())
            .await
            .unwrap();
        assert_eq!(detail.name_revision, 0);
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.title = NormalizedConversationTitle::parse("A").unwrap();
        command.source.product_conversation_id = gc.product_conversation_id;
        command.source.transcript_id = gc.id.clone();
        let ProductConversationRenameResult::Applied(first) = db
            .rename_product_conversation(&gc.id, &command)
            .await
            .unwrap()
        else {
            panic!()
        };
        let updated = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(updated.title.as_deref(), Some("A"));
        assert_eq!(updated.slug.as_deref(), Some(first.slug_at_commit.as_str()));
        // Ordinary rename still allocates a slug and advances the very same root revision.
        db.set_ordinary_product_conversation_title(&root.id, "B")
            .await
            .unwrap();
        db.set_ordinary_product_conversation_title(&root.id, "A")
            .await
            .unwrap();
        assert_eq!(
            db.read_product_conversation_rename_detail(
                &gc.id,
                root.product_conversation_id.as_str()
            )
            .await
            .unwrap()
            .name_revision,
            3
        );
        let mut stale = command.clone();
        stale.operation_id = uuid::Uuid::new_v4().to_string();
        stale.expected_name_revision = 1;
        assert_eq!(
            db.rename_product_conversation(&gc.id, &stale)
                .await
                .unwrap(),
            ProductConversationRenameResult::Stale
        );
        command.source.message_id = "retry-message".into();
        command.source.tool_use_id = "retry-call".into();
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .unwrap(),
            ProductConversationRenameResult::Replayed(first.clone())
        );
        assert_eq!(
            db.read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
                .await
                .unwrap(),
            Some(first)
        );
        assert!(matches!(
            db.read_product_conversation_rename_receipt(&root.id, &command.operation_id)
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }

    fn test_command(operation_id: String, revision: i64) -> ProductConversationRenameCommand {
        let owner: ProductConversationId = "pc_test".parse().unwrap();
        ProductConversationRenameCommand {
            operation_id,
            product_conversation_id: owner.clone(),
            expected_root_transcript_id: "root".into(),
            expected_name_revision: revision,
            title: NormalizedConversationTitle::parse("title").unwrap(),
            source: ProductConversationRenameSource {
                product_conversation_id: owner,
                transcript_id: "source".into(),
                message_id: "message".into(),
                tool_use_id: "tool".into(),
            },
        }
    }

    #[test]
    fn title_is_trimmed_and_limited_by_unicode_scalar_count() {
        assert_eq!(
            NormalizedConversationTitle::parse("  hello  ")
                .unwrap()
                .as_str(),
            "hello"
        );
        assert!(NormalizedConversationTitle::parse(" \n ").is_err());
        assert!(NormalizedConversationTitle::parse(&"x".repeat(201)).is_err());
        assert!(NormalizedConversationTitle::parse(&"🙂".repeat(200)).is_ok());
    }
}
