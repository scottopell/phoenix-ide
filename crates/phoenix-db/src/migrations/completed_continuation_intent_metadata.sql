ALTER TABLE completed_continuation_handoffs
ADD COLUMN handoff TEXT
CHECK (handoff IS NULL OR length(trim(handoff)) > 0);
ALTER TABLE completed_continuation_handoffs
ADD COLUMN user_agent TEXT;
ALTER TABLE completed_continuation_handoffs
ADD COLUMN intent_metadata_known INTEGER NOT NULL DEFAULT 0
CHECK (typeof(intent_metadata_known) = 'integer' AND intent_metadata_known IN (0, 1));

DROP TRIGGER continuation_dispatch_intents_immutable_identity;
CREATE TRIGGER continuation_dispatch_intents_immutable_identity
BEFORE UPDATE OF parent_conversation_id, successor_conversation_id,
                 message_id, handoff, user_agent, opening_authority
ON continuation_dispatch_intents
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'continuation dispatch intent identity is immutable');
END;

DROP TRIGGER consume_continuation_dispatch_intent;
CREATE TRIGGER consume_continuation_dispatch_intent
AFTER INSERT ON messages
WHEN EXISTS (
    SELECT 1 FROM continuation_dispatch_intents intent
    WHERE (NEW.message_id = intent.message_id
           OR NEW.message_id = intent.successor_conversation_id || ':' || intent.message_id)
      AND intent.successor_conversation_id = NEW.conversation_id
      AND (
          (intent.opening_authority = 'generated_predecessor_context'
           AND NEW.message_type = 'continuation'
           AND NEW.content = json_object('summary', intent.handoff))
          OR (intent.opening_authority = 'user_authorized_instruction'
              AND NEW.message_type = 'user'
              AND json_extract(NEW.content, '$.text') = intent.handoff)
      )
)
BEGIN
    INSERT INTO completed_continuation_handoffs (
        predecessor_conversation_id, successor_conversation_id,
        continuation_message_id, accepted_successor_message_id,
        opening_authority, handoff, user_agent, intent_metadata_known
    )
    SELECT intent.parent_conversation_id, intent.successor_conversation_id,
           continuation.message_id, NEW.message_id,
           intent.opening_authority, intent.handoff, intent.user_agent, 1
    FROM continuation_dispatch_intents intent
    JOIN messages continuation
      ON continuation.conversation_id = intent.parent_conversation_id
     AND continuation.message_type = 'continuation'
    WHERE (NEW.message_id = intent.message_id
           OR NEW.message_id = intent.successor_conversation_id || ':' || intent.message_id)
      AND intent.successor_conversation_id = NEW.conversation_id
      AND (
          (intent.opening_authority = 'generated_predecessor_context'
           AND NEW.message_type = 'continuation'
           AND NEW.content = json_object('summary', intent.handoff))
          OR (intent.opening_authority = 'user_authorized_instruction'
              AND NEW.message_type = 'user'
              AND json_extract(NEW.content, '$.text') = intent.handoff)
      )
    ORDER BY continuation.sequence_id DESC, continuation.message_id DESC
    LIMIT 1;

    DELETE FROM continuation_dispatch_intents
    WHERE successor_conversation_id = NEW.conversation_id
      AND (message_id = NEW.message_id
           OR NEW.message_id = successor_conversation_id || ':' || message_id)
      AND (
          (opening_authority = 'generated_predecessor_context'
           AND NEW.message_type = 'continuation'
           AND NEW.content = json_object('summary', handoff))
          OR (opening_authority = 'user_authorized_instruction'
              AND NEW.message_type = 'user'
              AND json_extract(NEW.content, '$.text') = handoff)
      );
END;
