use phoenix_core::domain::product_conversation::ProductConversationId;
use serde::Serialize;
use sqlx::{Connection, Row};

use crate::{Database, DbError, DbResult, FederationConversationAuthority};
use phoenix_core::domain::instance_identity::InstanceId;

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
    pub source_instance_id: InstanceId,
    pub product_conversation_id: ProductConversationId,
    pub transcript_id: String,
    pub message_id: String,
    pub tool_use_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProductConversationRenameReceiptSource {
    pub source_instance_id: InstanceId,
    pub product_conversation_id: String,
    pub transcript_id: String,
    pub message_id: String,
    pub tool_use_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoricalProductConversationRenameReceiptSource {
    pub product_conversation_id: String,
    pub transcript_id: String,
    pub message_id: String,
    pub tool_use_id: String,
}

impl From<&ProductConversationRenameSource> for ProductConversationRenameReceiptSource {
    fn from(source: &ProductConversationRenameSource) -> Self {
        Self {
            source_instance_id: source.source_instance_id,
            product_conversation_id: source.product_conversation_id.as_str().to_owned(),
            transcript_id: source.transcript_id.clone(),
            message_id: source.message_id.clone(),
            tool_use_id: source.tool_use_id.clone(),
        }
    }
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
    pub source: ProductConversationRenameReceiptSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoricalProductConversationRenameReceipt {
    pub operation_id: String,
    pub product_conversation_id: ProductConversationId,
    pub title: String,
    pub slug_at_commit: String,
    pub naming_root_transcript_id: String,
    pub name_revision_before: i64,
    pub committed_at_unix_micros: i64,
    pub source: HistoricalProductConversationRenameReceiptSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ProductConversationRenameReceiptRecord {
    Qualified(ProductConversationRenameReceipt),
    HistoricalUnqualified(HistoricalProductConversationRenameReceipt),
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
        self.read_product_conversation_rename_detail_authorized(
            RenameAuthority::Local(transcript_id),
            reference,
        )
        .await
    }

    /// # Errors
    /// Returns an error if the peer enrollment is inactive, the ordinary target is missing, or database access fails.
    pub async fn read_product_conversation_rename_detail_as_federation(
        &self,
        authority: &FederationConversationAuthority,
        reference: &str,
    ) -> DbResult<ProductConversationRenameDetail> {
        self.read_product_conversation_rename_detail_authorized(
            RenameAuthority::Federated(authority),
            reference,
        )
        .await
    }

    async fn read_product_conversation_rename_detail_authorized(
        &self,
        authority: RenameAuthority<'_>,
        reference: &str,
    ) -> DbResult<ProductConversationRenameDetail> {
        let mut tx = self.pool.begin().await?;
        match authority {
            RenameAuthority::Local(transcript_id) => {
                require_current_gc_tx(&mut tx, transcript_id).await?;
            }
            RenameAuthority::Federated(peer) => {
                peer.require_tx(&mut tx).await?;
            }
        }
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
    ) -> DbResult<Option<ProductConversationRenameReceiptRecord>> {
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
    /// Returns established typed errors or an unclassified durable outcome.
    pub async fn rename_product_conversation(
        &self,
        current_gc_transcript_id: &str,
        command: &ProductConversationRenameCommand,
    ) -> crate::workflow::LocalAuthorityResult<DbResult<ProductConversationRenameResult>> {
        self.rename_product_conversation_with_commit(
            current_gc_transcript_id,
            command,
            RenameCommit::Normal,
        )
        .await
    }

    /// # Errors
    /// Returns an error for inactive enrollment, invalid commands, or database failure.
    pub async fn rename_product_conversation_as_federation(
        &self,
        authority: &FederationConversationAuthority,
        command: &ProductConversationRenameCommand,
    ) -> crate::workflow::LocalAuthorityResult<DbResult<ProductConversationRenameResult>> {
        self.rename_product_conversation_authorized(
            RenameAuthority::Federated(authority),
            command,
            RenameCommit::Normal,
        )
        .await
    }

    async fn rename_product_conversation_with_commit(
        &self,
        current_gc_transcript_id: &str,
        command: &ProductConversationRenameCommand,
        commit: RenameCommit,
    ) -> crate::workflow::LocalAuthorityResult<DbResult<ProductConversationRenameResult>> {
        self.rename_product_conversation_authorized(
            RenameAuthority::Local(current_gc_transcript_id),
            command,
            commit,
        )
        .await
    }

    async fn rename_product_conversation_authorized(
        &self,
        authority: RenameAuthority<'_>,
        command: &ProductConversationRenameCommand,
        commit: RenameCommit,
    ) -> crate::workflow::LocalAuthorityResult<DbResult<ProductConversationRenameResult>> {
        use crate::workflow::LocalAuthorityResult;

        if let Err(error) = validate_command(command) {
            return LocalAuthorityResult::DurableFactEstablished(Err(error));
        }
        let mut conn = match self.pool.acquire().await {
            Ok(conn) => conn,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let mut tx = match conn.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            if !rename_authorized_tx(&mut tx, authority, command.source.source_instance_id).await? {
                return Ok(ProductConversationRenameResult::Conflict);
            }
            if let Some(row) = sqlx::query("SELECT * FROM product_conversation_rename_receipts WHERE operation_id = ?1")
                .bind(&command.operation_id).fetch_optional(&mut *tx).await? {
                let ProductConversationRenameReceiptRecord::Qualified(receipt) = decode_receipt(&row)? else {
                    return Ok(ProductConversationRenameResult::Conflict);
                };
                return Ok(if same_intent(&receipt, command) { ProductConversationRenameResult::Replayed(receipt) } else { ProductConversationRenameResult::Conflict });
            }
            if let RenameAuthority::Local(current_gc_transcript_id) = authority {
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
            }
            let resolved = Self::resolve_ordinary_product_conversation_on(&mut tx, command.product_conversation_id.as_str()).await;
            let resolved = match resolved { Ok(value) => value, Err(DbError::ConversationNotFound(_)) => return Ok(ProductConversationRenameResult::NotFound), Err(DbError::ProductConversationUnavailable(_)) => return Ok(ProductConversationRenameResult::Unavailable), Err(error) => return Err(error) };
            match crate::close_foundation::require_product_conversation_admission_tx(
                &mut tx,
                &resolved.requested_transcript_row_id,
            )
            .await
            {
                Ok(_) => {}
                Err(DbError::CloseAdmissionFenced(_) | DbError::ProductConversationUnavailable(_)) => {
                    return Ok(ProductConversationRenameResult::Unavailable);
                }
                Err(error) => return Err(error),
            }
            let root = sqlx::query("SELECT id, name_revision, archived FROM conversations WHERE product_conversation_id = ?1 AND runtime_role = 'user' AND user_initiated = 1 AND parent_conversation_id IS NULL AND NOT EXISTS (SELECT 1 FROM conversations p WHERE p.product_conversation_id = conversations.product_conversation_id AND p.continued_in_conv_id = conversations.id) ORDER BY id LIMIT 1")
                .bind(command.product_conversation_id.as_str()).fetch_optional(&mut *tx).await?;
            let Some(root) = root else { return Ok(ProductConversationRenameResult::NotFound); };
            let root_id: String = root.try_get("id")?;
            let revision: i64 = root.try_get("name_revision")?;
            if root.try_get::<bool, _>("archived")? {
                return Ok(ProductConversationRenameResult::Unavailable);
            }
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
            sqlx::query("INSERT INTO product_conversation_rename_receipts (operation_id, product_conversation_id, name_revision_before, title, source_product_conversation_id, source_transcript_id, source_message_id, source_tool_use_id, naming_root_transcript_id, slug_at_commit, committed_at_unix_micros, source_instance_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)")
                .bind(&command.operation_id).bind(command.product_conversation_id.as_str()).bind(revision).bind(command.title.as_str()).bind(command.source.product_conversation_id.as_str()).bind(&command.source.transcript_id).bind(&command.source.message_id).bind(&command.source.tool_use_id).bind(&root_id).bind(&slug).bind(now).bind(command.source.source_instance_id.to_string()).execute(&mut *tx).await?;
            Ok(ProductConversationRenameResult::Applied(ProductConversationRenameReceipt { operation_id: command.operation_id.clone(), product_conversation_id: command.product_conversation_id.clone(), title: command.title.as_str().into(), slug_at_commit: slug, naming_root_transcript_id: root_id, name_revision_before: revision, committed_at_unix_micros: now, source: (&command.source).into() }))
        }.await;
        match result {
            Ok(value) => match commit.execute(tx).await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(value)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }
}

async fn rename_authorized_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    authority: RenameAuthority<'_>,
    source_instance_id: InstanceId,
) -> DbResult<bool> {
    let local: String =
        sqlx::query_scalar("SELECT instance_id FROM instance_identity WHERE singleton_key = 1")
            .fetch_one(&mut **tx)
            .await?;
    let local: InstanceId = local.parse().map_err(
        |e: phoenix_core::domain::instance_identity::InstanceIdParseError| {
            DbError::Serialization(e.to_string())
        },
    )?;
    match authority {
        RenameAuthority::Local(transcript_id) => {
            require_current_gc_tx(tx, transcript_id).await?;
            Ok(source_instance_id == local)
        }
        RenameAuthority::Federated(peer) => {
            peer.require_tx(tx).await?;
            Ok(source_instance_id == peer.caller_instance_id() && source_instance_id != local)
        }
    }
}

#[derive(Clone, Copy)]
enum RenameAuthority<'a> {
    Local(&'a str),
    Federated(&'a FederationConversationAuthority),
}

enum RenameCommit {
    Normal,
    #[cfg(test)]
    LoseAcknowledgement,
}

impl RenameCommit {
    async fn execute(self, tx: sqlx::Transaction<'_, sqlx::Sqlite>) -> DbResult<()> {
        tx.commit().await?;
        match self {
            Self::Normal => Ok(()),
            #[cfg(test)]
            Self::LoseAcknowledgement => Err(sqlx::Error::WorkerCrashed.into()),
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

fn valid_source_locator(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains('\0')
}

fn validate_command(command: &ProductConversationRenameCommand) -> DbResult<()> {
    validate_operation_id(&command.operation_id)?;
    if command.expected_name_revision < 0
        || command.expected_name_revision == i64::MAX
        || command.expected_root_transcript_id.trim().is_empty()
        || !valid_source_locator(command.source.product_conversation_id.as_str())
        || !valid_source_locator(&command.source.transcript_id)
        || !valid_source_locator(&command.source.message_id)
        || !valid_source_locator(&command.source.tool_use_id)
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
        && receipt.source.source_instance_id == command.source.source_instance_id
        && receipt.source.product_conversation_id == command.source.product_conversation_id.as_str()
        && receipt.source.transcript_id == command.source.transcript_id
}

fn decode_receipt(
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<ProductConversationRenameReceiptRecord> {
    let product = |key: &str| -> DbResult<ProductConversationId> {
        row.try_get::<String, _>(key)?
            .parse::<ProductConversationId>()
            .map_err(|e| DbError::Serialization(e.to_string()))
    };
    let source_instance_id: Option<String> = row.try_get("source_instance_id")?;
    let Some(source_instance_id) = source_instance_id else {
        return Ok(
            ProductConversationRenameReceiptRecord::HistoricalUnqualified(
                HistoricalProductConversationRenameReceipt {
                    operation_id: row.try_get("operation_id")?,
                    product_conversation_id: product("product_conversation_id")?,
                    title: row.try_get("title")?,
                    slug_at_commit: row.try_get("slug_at_commit")?,
                    naming_root_transcript_id: row.try_get("naming_root_transcript_id")?,
                    name_revision_before: row.try_get("name_revision_before")?,
                    committed_at_unix_micros: row.try_get("committed_at_unix_micros")?,
                    source: HistoricalProductConversationRenameReceiptSource {
                        product_conversation_id: row.try_get("source_product_conversation_id")?,
                        transcript_id: row.try_get("source_transcript_id")?,
                        message_id: row.try_get("source_message_id")?,
                        tool_use_id: row.try_get("source_tool_use_id")?,
                    },
                },
            ),
        );
    };
    Ok(ProductConversationRenameReceiptRecord::Qualified(
        ProductConversationRenameReceipt {
            operation_id: row.try_get("operation_id")?,
            product_conversation_id: product("product_conversation_id")?,
            title: row.try_get("title")?,
            slug_at_commit: row.try_get("slug_at_commit")?,
            naming_root_transcript_id: row.try_get("naming_root_transcript_id")?,
            name_revision_before: row.try_get("name_revision_before")?,
            committed_at_unix_micros: row.try_get("committed_at_unix_micros")?,
            source: ProductConversationRenameReceiptSource {
                source_instance_id: source_instance_id.parse().map_err(
                    |e: phoenix_core::domain::instance_identity::InstanceIdParseError| {
                        DbError::Serialization(e.to_string())
                    },
                )?,
                product_conversation_id: row.try_get("source_product_conversation_id")?,
                transcript_id: row.try_get("source_transcript_id")?,
                message_id: row.try_get("source_message_id")?,
                tool_use_id: row.try_get("source_tool_use_id")?,
            },
        },
    ))
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
    async fn federated_rename_detail_uses_peer_authority_for_stable_and_exact_references() {
        use crate::federation_conversation_authority::enroll_test_peer;

        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("option-target", "Option target", "/tmp", true, None, None)
            .await
            .unwrap();
        let caller = InstanceId::new();
        let authority = enroll_test_peer(&db, caller, b"option-peer").await;

        let stable = db
            .read_product_conversation_rename_detail_as_federation(
                &authority,
                root.product_conversation_id.as_str(),
            )
            .await
            .unwrap();
        let exact = db
            .read_product_conversation_rename_detail_as_federation(&authority, &root.id)
            .await
            .unwrap();
        assert_eq!(stable.product_conversation_id, root.product_conversation_id);
        assert_eq!(
            exact.product_conversation_id,
            stable.product_conversation_id
        );
        assert_eq!(stable.root_transcript_id, root.id);
        assert_eq!(exact.root_transcript_id, stable.root_transcript_id);
        assert_eq!(exact.name_revision, stable.name_revision);
        assert_eq!(exact.title, stable.title);
        assert_eq!(exact.slug, stable.slug);
        assert!(stable.can_rename);

        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        assert!(matches!(
            db.read_product_conversation_rename_detail(
                &root.id,
                root.product_conversation_id.as_str()
            )
            .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
        assert_eq!(
            db.read_product_conversation_rename_detail(
                &gc.id,
                root.product_conversation_id.as_str()
            )
            .await
            .unwrap()
            .root_transcript_id,
            root.id
        );
    }

    #[tokio::test]
    async fn federated_rename_detail_rejects_inactive_peer_before_target_resolution() {
        use crate::federation_conversation_authority::enroll_test_peer;
        use phoenix_core::domain::instance_identity::FederationCredentialVerifier;

        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("option-guard", "Option guard", "/tmp", true, None, None)
            .await
            .unwrap();
        let caller = InstanceId::new();
        let authority = enroll_test_peer(&db, caller, b"first-credential").await;
        let wrong: FederationConversationAuthority = crate::FederationSendAuthority::new(
            caller,
            FederationCredentialVerifier::from_bearer(b"wrong-credential"),
        )
        .into();
        for reference in [root.product_conversation_id.as_str(), "missing-option"] {
            assert!(matches!(
                db.read_product_conversation_rename_detail_as_federation(&wrong, reference)
                    .await,
                Err(DbError::FederationSendAuthorityInactive)
            ));
        }
        let replacement = enroll_test_peer(&db, caller, b"replacement-credential").await;
        assert!(matches!(
            db.read_product_conversation_rename_detail_as_federation(
                &authority,
                root.product_conversation_id.as_str()
            )
            .await,
            Err(DbError::FederationSendAuthorityInactive)
        ));
        assert_eq!(
            db.read_product_conversation_rename_detail_as_federation(&replacement, &root.id)
                .await
                .unwrap()
                .product_conversation_id,
            root.product_conversation_id
        );
    }

    #[tokio::test]
    async fn federated_rename_detail_rejects_non_ordinary_and_missing_targets() {
        use crate::federation_conversation_authority::enroll_test_peer;

        let db = Database::open_in_memory().await.unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let authority = enroll_test_peer(&db, InstanceId::new(), b"kind-guard").await;
        for reference in [
            gc.product_conversation_id.as_str(),
            gc.id.as_str(),
            "missing-option",
        ] {
            assert!(matches!(
                db.read_product_conversation_rename_detail_as_federation(&authority, reference)
                    .await,
                Err(DbError::ConversationNotFound(id)) if id == reference
            ));
            assert!(matches!(
                db.read_product_conversation_rename_detail(&gc.id, reference).await,
                Err(DbError::ConversationNotFound(id)) if id == reference
            ));
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn federated_rename_replays_before_live_lookup_and_preserves_first_foreign_source() {
        use crate::federation_conversation_authority::enroll_test_peer;
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("remote-rename-root", "before", "/tmp", true, None, None)
            .await
            .unwrap();
        let caller = InstanceId::new();
        let authority = enroll_test_peer(&db, caller, b"rename-peer").await;
        let other_caller = InstanceId::new();
        let other = enroll_test_peer(&db, other_caller, b"rename-other").await;
        let mut command = test_command(uuid::Uuid::new_v4().hyphenated().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = caller;
        command.source.transcript_id = "foreign-source-with-no-local-row".into();
        command.source.message_id = "foreign-message".into();
        command.source.tool_use_id = "foreign-tool".into();
        let ProductConversationRenameResult::Applied(first) = db
            .rename_product_conversation_as_federation(&authority, &command)
            .await
            .established()
            .unwrap()
            .unwrap()
        else {
            panic!("expected rename")
        };
        assert_eq!(
            first.source,
            ProductConversationRenameReceiptSource::from(&command.source)
        );
        assert_eq!(first.title, command.title.as_str());
        let mut changed_locator = command.clone();
        changed_locator.source.message_id = "different-message".into();
        changed_locator.source.tool_use_id = "different-call".into();
        assert_eq!(
            db.rename_product_conversation_as_federation(&authority, &changed_locator)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Replayed(first.clone())
        );
        let mut different_transcript_owner = command.clone();
        different_transcript_owner.source.transcript_id = "another-source-transcript".into();
        assert_eq!(
            db.rename_product_conversation_as_federation(&authority, &different_transcript_owner)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Conflict
        );
        let mut different_owner = command.clone();
        different_owner.source.source_instance_id = other_caller;
        assert_eq!(
            db.rename_product_conversation_as_federation(&other, &different_owner)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Conflict
        );
        let mut later = command.clone();
        later.operation_id = uuid::Uuid::new_v4().to_string();
        later.expected_name_revision = 1;
        later.title = NormalizedConversationTitle::parse("Later title").unwrap();
        assert!(matches!(
            db.rename_product_conversation_as_federation(&authority, &later)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Applied(_)
        ));
        assert_eq!(
            db.rename_product_conversation_as_federation(&authority, &changed_locator)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Replayed(first.clone())
        );
        sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(&root.id)
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            db.rename_product_conversation_as_federation(&authority, &changed_locator)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Replayed(first.clone())
        );
        let replacement = enroll_test_peer(&db, caller, b"rename-rotated").await;
        assert!(matches!(
            db.rename_product_conversation_as_federation(&authority, &command)
                .await
                .established()
                .unwrap(),
            Err(DbError::FederationSendAuthorityInactive)
        ));
        assert_eq!(
            db.rename_product_conversation_as_federation(&replacement, &command)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Replayed(first)
        );
        let mut changed = command.clone();
        changed.title = NormalizedConversationTitle::parse("Changed intent").unwrap();
        assert_eq!(
            db.rename_product_conversation_as_federation(&replacement, &changed)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Conflict
        );
    }

    #[tokio::test]
    async fn committed_rename_with_lost_acknowledgement_is_not_an_ordinary_failure() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("lost-ack-root", "lost-ack-root", "/tmp", true, None, None)
            .await
            .unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id;
        command.source.transcript_id = gc.id.clone();
        let outcome = db
            .rename_product_conversation_with_commit(
                &gc.id,
                &command,
                RenameCommit::LoseAcknowledgement,
            )
            .await;
        assert!(matches!(
            outcome,
            crate::workflow::LocalAuthorityResult::DurableFactUnclassified
        ));
        let receipt = db
            .read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
            .await
            .unwrap()
            .expect("the injected acknowledgement loss follows a real commit");
        let ProductConversationRenameReceiptRecord::Qualified(receipt) = receipt else {
            panic!("new renames must have qualified receipts");
        };
        assert_eq!(
            serde_json::to_value(ProductConversationRenameReceiptRecord::Qualified(
                receipt.clone()
            ))
            .unwrap(),
            serde_json::to_value(&receipt).unwrap()
        );
        assert_eq!(receipt.title, command.title.as_str());
        assert_eq!(receipt.name_revision_before, 0);
        assert_eq!(
            db.read_product_conversation_rename_detail(
                &gc.id,
                root.product_conversation_id.as_str()
            )
            .await
            .unwrap()
            .name_revision,
            1
        );
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .expect("successful acknowledgement establishes replay")
                .unwrap(),
            ProductConversationRenameResult::Replayed(receipt)
        );
        assert_eq!(
            db.read_product_conversation_rename_detail(
                &gc.id,
                root.product_conversation_id.as_str()
            )
            .await
            .unwrap()
            .name_revision,
            1
        );
    }

    #[tokio::test]
    async fn rejected_rename_with_acknowledged_rollback_is_established_and_unwritten() {
        let db = Database::open_in_memory().await.unwrap();
        let command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        assert!(matches!(
            db.rename_product_conversation("not-a-coordinator", &command)
                .await
                .established()
                .expect("acknowledged rollback establishes rejection"),
            Err(DbError::CurrentCoordinatorRequired)
        ));
        let receipts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM product_conversation_rename_receipts")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(receipts, 0);
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
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id;
        command.source.transcript_id = gc.id.clone();
        let ProductConversationRenameResult::Applied(first) = db
            .rename_product_conversation(&gc.id, &command)
            .await
            .established()
            .expect("rename classification")
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
                .established()
                .expect("rename classification")
                .unwrap(),
            ProductConversationRenameResult::Stale
        );
        let mut changed_locator = command.clone();
        changed_locator.source.message_id = "retry-message".into();
        changed_locator.source.tool_use_id = "retry-call".into();
        assert_eq!(
            db.rename_product_conversation(&gc.id, &changed_locator)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Replayed(first.clone())
        );
        let mut changed_source_owner = command.clone();
        changed_source_owner.source.product_conversation_id = root.product_conversation_id.clone();
        assert_eq!(
            db.rename_product_conversation(&gc.id, &changed_source_owner)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Conflict
        );
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .expect("rename classification")
                .unwrap(),
            ProductConversationRenameResult::Replayed(first.clone())
        );
        assert_eq!(
            db.read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
                .await
                .unwrap(),
            Some(ProductConversationRenameReceiptRecord::Qualified(first))
        );
        assert!(matches!(
            db.read_product_conversation_rename_receipt(&root.id, &command.operation_id)
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }

    #[tokio::test]
    async fn active_close_fence_classifies_new_rename_as_unavailable() {
        use phoenix_core::domain::close::TranscriptConversationId;

        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation(
                "close-fenced-root",
                "close-fenced-root",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id;
        command.source.transcript_id = gc.id.clone();
        db.begin_close_foundation(
            &root.product_conversation_id,
            &TranscriptConversationId::parse(root.id.clone()).unwrap(),
            "close-attempt",
        )
        .await
        .unwrap();

        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .expect("rename classification")
                .unwrap(),
            ProductConversationRenameResult::Unavailable,
        );
        assert!(db
            .read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn archived_root_refuses_new_rename_without_changing_identity() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("archived-root", "archived-root", "/tmp", true, None, None)
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
        assert!(detail.can_rename);
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), detail.name_revision);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id;
        command.source.transcript_id = gc.id.clone();
        sqlx::query("UPDATE conversations SET archived=1 WHERE id=?1")
            .bind(&root.id)
            .execute(db.pool())
            .await
            .unwrap();
        let before = db.get_conversation(&root.id).await.unwrap();
        assert!(
            !db.read_product_conversation_rename_detail(
                &gc.id,
                root.product_conversation_id.as_str()
            )
            .await
            .unwrap()
            .can_rename
        );
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .expect("rename classification")
                .unwrap(),
            ProductConversationRenameResult::Unavailable
        );
        let after = db.get_conversation(&root.id).await.unwrap();
        assert_eq!(after.title, before.title);
        assert_eq!(after.slug, before.slug);
        assert_eq!(after.updated_at, before.updated_at);
        assert_eq!(
            db.read_product_conversation_rename_detail(
                &gc.id,
                root.product_conversation_id.as_str()
            )
            .await
            .unwrap()
            .name_revision,
            detail.name_revision
        );
        assert!(db
            .read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn historical_rename_receipt_survives_target_deletion_and_fences_reuse() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("deleted-target", "deleted-target", "/tmp", true, None, None)
            .await
            .unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id.clone();
        command.source.transcript_id = gc.id.clone();
        let ProductConversationRenameResult::Applied(receipt) = db
            .rename_product_conversation(&gc.id, &command)
            .await
            .established()
            .expect("rename classification")
            .unwrap()
        else {
            panic!("expected applied rename");
        };
        db.delete_conversation(&root.id).await.unwrap();
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations WHERE id=?1")
                .bind(root.product_conversation_id.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(
            db.read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
                .await
                .unwrap(),
            Some(ProductConversationRenameReceiptRecord::Qualified(
                receipt.clone()
            ))
        );
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .expect("rename classification")
                .unwrap(),
            ProductConversationRenameResult::Replayed(receipt.clone())
        );
        let replacement = db
            .create_conversation("replacement", "replacement", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut changed = command.clone();
        changed.product_conversation_id = replacement.product_conversation_id;
        changed.expected_root_transcript_id = replacement.id;
        assert_rename_conflict(&db, &gc.id, &changed).await;
        changed = command.clone();
        changed.title = NormalizedConversationTitle::parse("changed intent").unwrap();
        assert_rename_conflict(&db, &gc.id, &changed).await;
        changed = command.clone();
        changed.source.product_conversation_id = command.product_conversation_id.clone();
        assert_rename_conflict(&db, &gc.id, &changed).await;
        assert_eq!(
            db.read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
                .await
                .unwrap(),
            Some(ProductConversationRenameReceiptRecord::Qualified(receipt))
        );
    }

    async fn assert_rename_conflict(
        db: &Database,
        transcript_id: &str,
        command: &ProductConversationRenameCommand,
    ) {
        assert!(matches!(
            db.rename_product_conversation(transcript_id, command)
                .await
                .established()
                .expect("rename classification"),
            Ok(ProductConversationRenameResult::Conflict)
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
                source_instance_id: InstanceId::new(),
                product_conversation_id: owner,
                transcript_id: "source".into(),
                message_id: "message".into(),
                tool_use_id: "tool".into(),
            },
        }
    }

    #[tokio::test]
    async fn qualified_locator_bytes_replay_or_conflict_without_normalization() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("legacy-rename-target", "before", "/tmp", true, None, None)
            .await
            .unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id.clone();
        command.source.transcript_id = gc.id.clone();
        sqlx::query("INSERT INTO product_conversation_rename_receipts (operation_id, product_conversation_id, name_revision_before, title, source_product_conversation_id, source_transcript_id, source_message_id, source_tool_use_id, naming_root_transcript_id, slug_at_commit, committed_at_unix_micros, source_instance_id) VALUES (?1,?2,0,'title',?3,?4,'  message  ','\ttool\t',?5,'retained-slug',10,?6)")
            .bind(&command.operation_id)
            .bind(command.product_conversation_id.as_str())
            .bind(command.source.product_conversation_id.as_str())
            .bind(&command.source.transcript_id)
            .bind(&command.expected_root_transcript_id)
            .bind(command.source.source_instance_id.to_string())
            .execute(db.pool()).await.unwrap();
        db.delete_conversation(&root.id).await.unwrap();
        let receipt = db
            .read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
            .await
            .unwrap()
            .unwrap();
        let ProductConversationRenameReceiptRecord::Qualified(receipt) = receipt else {
            panic!("a source with an instance identity must have a qualified receipt");
        };
        assert_eq!(receipt.source.message_id, "  message  ");
        assert_eq!(receipt.source.tool_use_id, "\ttool\t");
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Replayed(receipt)
        );
        sqlx::query("UPDATE product_conversation_rename_receipts SET source_product_conversation_id=' \tproduct\t ' WHERE operation_id=?1")
            .bind(&command.operation_id).execute(db.pool()).await.unwrap();
        let retained = db
            .read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
            .await
            .unwrap()
            .unwrap();
        let ProductConversationRenameReceiptRecord::Qualified(retained) = retained else {
            panic!("a source with an instance identity must have a qualified receipt");
        };
        assert_eq!(retained.source.product_conversation_id, " \tproduct\t ");
        assert_eq!(
            db.rename_product_conversation(&gc.id, &command)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ProductConversationRenameResult::Conflict
        );
    }

    #[tokio::test]
    async fn historical_unqualified_receipts_preserve_exact_snapshots_and_locator_bytes() {
        let db = Database::open_in_memory().await.unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let product_conversation_id = ProductConversationId::new();
        for missing_locator in 0..4 {
            let mut locators = [
                " \tproduct\0é ",
                "\ttranscript\n ",
                "  \0message ",
                "\rtool-use\t ",
            ];
            locators[missing_locator] = "";
            let expected = HistoricalProductConversationRenameReceipt {
                operation_id: uuid::Uuid::new_v4().to_string(),
                product_conversation_id: product_conversation_id.clone(),
                title: " Historical\0title \t ".into(),
                slug_at_commit: " historical-slug ".into(),
                naming_root_transcript_id: " root\0snapshot \t ".into(),
                name_revision_before: 7,
                committed_at_unix_micros: 1_234_567_890_123,
                source: HistoricalProductConversationRenameReceiptSource {
                    product_conversation_id: locators[0].into(),
                    transcript_id: locators[1].into(),
                    message_id: locators[2].into(),
                    tool_use_id: locators[3].into(),
                },
            };
            sqlx::query(
                "INSERT INTO product_conversation_rename_receipts
                 (operation_id, product_conversation_id, title, slug_at_commit,
                  naming_root_transcript_id, name_revision_before, committed_at_unix_micros,
                  source_instance_id, source_product_conversation_id, source_transcript_id,
                  source_message_id, source_tool_use_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?11)",
            )
            .bind(&expected.operation_id)
            .bind(expected.product_conversation_id.as_str())
            .bind(&expected.title)
            .bind(&expected.slug_at_commit)
            .bind(&expected.naming_root_transcript_id)
            .bind(expected.name_revision_before)
            .bind(expected.committed_at_unix_micros)
            .bind(&expected.source.product_conversation_id)
            .bind(&expected.source.transcript_id)
            .bind(&expected.source.message_id)
            .bind(&expected.source.tool_use_id)
            .execute(db.pool())
            .await
            .unwrap();

            let record = db
                .read_product_conversation_rename_receipt(&gc.id, &expected.operation_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                record,
                ProductConversationRenameReceiptRecord::HistoricalUnqualified(expected.clone())
            );
            assert_eq!(
                serde_json::to_value(&record).unwrap(),
                serde_json::json!({
                    "operation_id": expected.operation_id,
                    "product_conversation_id": expected.product_conversation_id,
                    "title": expected.title,
                    "slug_at_commit": expected.slug_at_commit,
                    "naming_root_transcript_id": expected.naming_root_transcript_id,
                    "name_revision_before": expected.name_revision_before,
                    "committed_at_unix_micros": expected.committed_at_unix_micros,
                    "source": {
                        "product_conversation_id": locators[0],
                        "transcript_id": locators[1],
                        "message_id": locators[2],
                        "tool_use_id": locators[3],
                    },
                })
            );
        }
    }

    #[tokio::test]
    async fn retry_of_unqualified_historical_operation_conflicts_without_rewriting_it() {
        let db = Database::open_in_memory().await.unwrap();
        let gc = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let root = db
            .create_conversation("unqualified-target", "Original", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        command.product_conversation_id = root.product_conversation_id.clone();
        command.expected_root_transcript_id = root.id.clone();
        command.source.source_instance_id = db.instance_id().await.unwrap();
        command.source.product_conversation_id = gc.product_conversation_id.clone();
        command.source.transcript_id = gc.id.clone();
        sqlx::query(
            "INSERT INTO product_conversation_rename_receipts
             (operation_id, product_conversation_id, title, slug_at_commit,
              naming_root_transcript_id, name_revision_before, committed_at_unix_micros,
              source_instance_id, source_product_conversation_id, source_transcript_id,
              source_message_id, source_tool_use_id)
             VALUES (?1, ?2, ?3, 'historical-slug', ?4, ?5, 123, NULL, ?6, ?7, ?8, ?9)",
        )
        .bind(&command.operation_id)
        .bind(command.product_conversation_id.as_str())
        .bind(command.title.as_str())
        .bind(&command.expected_root_transcript_id)
        .bind(command.expected_name_revision)
        .bind(command.source.product_conversation_id.as_str())
        .bind(&command.source.transcript_id)
        .bind("")
        .bind(&command.source.tool_use_id)
        .execute(db.pool())
        .await
        .unwrap();
        let before = db
            .read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            &before,
            ProductConversationRenameReceiptRecord::HistoricalUnqualified(_)
        ));

        assert_rename_conflict(&db, &gc.id, &command).await;
        assert_eq!(
            db.read_product_conversation_rename_receipt(&gc.id, &command.operation_id)
                .await
                .unwrap(),
            Some(before)
        );
        let detail = db
            .read_product_conversation_rename_detail(&gc.id, root.product_conversation_id.as_str())
            .await
            .unwrap();
        assert_eq!(detail.title, root.title);
        assert_eq!(detail.name_revision, 0);
    }

    #[test]
    fn command_rejects_invalid_native_locator_values() {
        let mut command = test_command(uuid::Uuid::new_v4().to_string(), 0);
        for invalid in ["  \t", "message\0suffix"] {
            command.source.message_id = invalid.into();
            assert!(validate_command(&command).is_err());
        }
        command.source.message_id = "message".into();
        command.expected_root_transcript_id = "  ".into();
        assert!(validate_command(&command).is_err());
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
