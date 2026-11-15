CREATE TABLE product_conversation_read_state (
    product_conversation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES product_conversations(id) ON DELETE CASCADE,
    read_through_message_id TEXT
        REFERENCES messages(message_id) ON DELETE SET NULL,
    marked_unread INTEGER NOT NULL
        CHECK (typeof(marked_unread) = 'integer' AND marked_unread IN (0, 1))
) WITHOUT ROWID;

CREATE INDEX product_conversation_read_state_message
    ON product_conversation_read_state(read_through_message_id);

INSERT INTO product_conversation_read_state(
    product_conversation_id, read_through_message_id, marked_unread
)
SELECT product.id, (
    SELECT message.message_id
    FROM conversations head
    JOIN messages message ON message.conversation_id = head.id
    WHERE head.product_conversation_id = product.id
      AND head.parent_conversation_id IS NULL
      AND head.runtime_role IN ('user', 'coordinator')
      AND head.continued_in_conv_id IS NULL
    ORDER BY message.sequence_id DESC
    LIMIT 1
), 0
FROM product_conversations product;
