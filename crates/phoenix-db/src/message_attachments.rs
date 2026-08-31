use crate::{DbError, DbResult, FileAttachment, ImageData, Message, MessageContent};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};
use std::collections::HashMap;

struct HydratedAttachments {
    files: Vec<FileAttachment>,
    images: Vec<ImageData>,
}

/// Hydrate normalized attachments with exactly one file query and one image
/// query, independent of transcript length. The bound JSON array avoids both a
/// variable SQL shape and `SQLite`'s host-parameter limit.
pub(crate) async fn hydrate(
    connection: &mut SqliteConnection,
    messages: &mut [Message],
) -> DbResult<()> {
    let file_message_ids = messages
        .iter()
        .filter(|message| {
            matches!(
                message.message_type,
                crate::MessageType::User | crate::MessageType::Skill
            )
        })
        .map(|message| message.message_id.as_str())
        .collect::<Vec<_>>();
    let image_message_ids = messages
        .iter()
        .filter(|message| message.message_type == crate::MessageType::User)
        .map(|message| message.message_id.as_str())
        .collect::<Vec<_>>();
    let file_message_ids = serde_json::to_string(&file_message_ids)
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    let image_message_ids = serde_json::to_string(&image_message_ids)
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;

    let file_rows = sqlx::query(
        "SELECT attachment.message_id, attachment.original_name, attachment.media_type,
                attachment.size_bytes, attachment.stored_path
         FROM json_each(?1) requested
         JOIN message_files attachment ON attachment.message_id = requested.value
         ORDER BY requested.key, attachment.ordinal",
    )
    .bind(&file_message_ids)
    .fetch_all(&mut *connection)
    .await?;
    let image_rows = sqlx::query(
        "SELECT attachment.message_id, attachment.media_type, attachment.data
         FROM json_each(?1) requested
         JOIN message_images attachment ON attachment.message_id = requested.value
         ORDER BY requested.key, attachment.ordinal",
    )
    .bind(&image_message_ids)
    .fetch_all(&mut *connection)
    .await?;

    let mut by_message = HashMap::<String, HydratedAttachments>::new();
    for row in file_rows {
        let message_id = row.try_get("message_id")?;
        by_message
            .entry(message_id)
            .or_insert_with(empty_attachments)
            .files
            .push(file_from_row(&row)?);
    }
    for row in image_rows {
        let (message_id, image) = image_from_row(&row)?;
        by_message
            .entry(message_id)
            .or_insert_with(empty_attachments)
            .images
            .push(image);
    }
    for message in messages {
        if matches!(
            message.message_type,
            crate::MessageType::User | crate::MessageType::Skill
        ) {
            let attachments = by_message
                .remove(&message.message_id)
                .unwrap_or_else(empty_attachments);
            message
                .content
                .set_attachments(attachments.images, attachments.files);
        }
    }
    Ok(())
}

fn empty_attachments() -> HydratedAttachments {
    HydratedAttachments {
        files: Vec::new(),
        images: Vec::new(),
    }
}

fn file_from_row(row: &SqliteRow) -> DbResult<FileAttachment> {
    let persisted_size = row.try_get::<i64, _>("size_bytes")?;
    let size_bytes = u64::try_from(persisted_size).map_err(|_| {
        DbError::Serialization(format!(
            "message file attachment size must be non-negative, got {persisted_size}"
        ))
    })?;
    Ok(FileAttachment {
        original_name: row.try_get("original_name")?,
        media_type: row.try_get("media_type")?,
        size_bytes,
        stored_path: row.try_get("stored_path")?,
    })
}

fn image_from_row(row: &SqliteRow) -> DbResult<(String, ImageData)> {
    Ok((
        row.try_get("message_id")?,
        ImageData {
            media_type: row.try_get("media_type")?,
            data: row.try_get("data")?,
        },
    ))
}

/// Persist normalized attachment children on the caller-owned connection. A
/// transaction-owning caller passes its transaction's connection here, keeping
/// the parent and every child in the same commit boundary.
pub(crate) async fn insert(
    connection: &mut SqliteConnection,
    message_id: &str,
    content: &MessageContent,
) -> DbResult<()> {
    let (images, files) = content.attachments();
    for (ordinal, file) in files.iter().enumerate() {
        let ordinal = attachment_ordinal(ordinal)?;
        let size_bytes = i64::try_from(file.size_bytes).map_err(|_| {
            DbError::Serialization(format!(
                "message {message_id} file attachment size exceeds SQLite INTEGER"
            ))
        })?;
        sqlx::query(
            "INSERT OR IGNORE INTO message_files
             (message_id, ordinal, original_name, media_type, size_bytes, stored_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(message_id)
        .bind(ordinal)
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(size_bytes)
        .bind(&file.stored_path)
        .execute(&mut *connection)
        .await?;
    }
    for (ordinal, image) in images.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO message_images (message_id, ordinal, media_type, data)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(message_id)
        .bind(attachment_ordinal(ordinal)?)
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

fn attachment_ordinal(ordinal: usize) -> DbResult<i64> {
    i64::try_from(ordinal)
        .map_err(|_| DbError::Serialization("attachment ordinal exceeds SQLite INTEGER".into()))
}
