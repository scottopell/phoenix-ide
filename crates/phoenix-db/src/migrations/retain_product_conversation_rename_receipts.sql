CREATE TABLE product_conversation_rename_receipts_retained (
    operation_id TEXT PRIMARY KEY NOT NULL CHECK(typeof(operation_id) = 'text'),
    product_conversation_id TEXT NOT NULL
        CHECK(typeof(product_conversation_id) = 'text' AND length(product_conversation_id) > 0),
    name_revision_before INTEGER NOT NULL CHECK(typeof(name_revision_before) = 'integer'
        AND name_revision_before >= 0 AND name_revision_before < 9223372036854775807),
    title TEXT NOT NULL CHECK(typeof(title) = 'text'),
    source_product_conversation_id TEXT NOT NULL CHECK(typeof(source_product_conversation_id) = 'text'
        AND length(source_product_conversation_id) > 0),
    source_transcript_id TEXT NOT NULL CHECK(typeof(source_transcript_id) = 'text'
        AND length(source_transcript_id) > 0),
    source_message_id TEXT NOT NULL CHECK(typeof(source_message_id) = 'text'
        AND length(source_message_id) > 0),
    source_tool_use_id TEXT NOT NULL CHECK(typeof(source_tool_use_id) = 'text'
        AND length(source_tool_use_id) > 0),
    naming_root_transcript_id TEXT NOT NULL CHECK(typeof(naming_root_transcript_id) = 'text'
        AND length(naming_root_transcript_id) > 0),
    slug_at_commit TEXT NOT NULL CHECK(typeof(slug_at_commit) = 'text' AND length(slug_at_commit) > 0),
    committed_at_unix_micros INTEGER NOT NULL CHECK(typeof(committed_at_unix_micros) = 'integer'
        AND committed_at_unix_micros >= 0)
);
INSERT INTO product_conversation_rename_receipts_retained (
    operation_id, product_conversation_id, name_revision_before, title,
    source_product_conversation_id, source_transcript_id, source_message_id,
    source_tool_use_id, naming_root_transcript_id, slug_at_commit, committed_at_unix_micros
)
SELECT operation_id, product_conversation_id, name_revision_before, title,
       source_product_conversation_id, source_transcript_id, source_message_id,
       source_tool_use_id, naming_root_transcript_id, slug_at_commit, committed_at_unix_micros
FROM product_conversation_rename_receipts;
DROP TABLE product_conversation_rename_receipts;
ALTER TABLE product_conversation_rename_receipts_retained RENAME TO product_conversation_rename_receipts;
CREATE INDEX product_conversation_rename_receipts_target
    ON product_conversation_rename_receipts(product_conversation_id);
