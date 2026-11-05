DROP TRIGGER conversation_goal_preparations_source;
CREATE TRIGGER conversation_goal_preparations_source
BEFORE INSERT ON conversation_goal_preparations
WHEN NOT EXISTS (
    SELECT 1 FROM conversations c JOIN product_conversations p ON p.id=c.product_conversation_id
    JOIN durable_turns t ON t.conversation_id=c.id
    WHERE c.id=NEW.caller_conversation_id AND p.id=NEW.source_product_id AND p.kind='coordinator'
      AND t.turn_id=NEW.turn_id AND t.workflow_id=NEW.workflow_id
      AND (
        EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id=c.id
          AND m.message_id=NEW.message_id AND m.message_type='agent')
        OR (c.state_kind='tool_executing' AND t.disposition='Runtime'
          AND t.terminal_kind IS NULL AND t.owns_conversation=1)
      )
)
BEGIN SELECT RAISE(ABORT,'goal preparation source mismatch'); END;
