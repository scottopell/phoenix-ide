ALTER TABLE product_conversations ADD COLUMN auto_continue_disable_epoch INTEGER NOT NULL DEFAULT 0
    CHECK(typeof(auto_continue_disable_epoch)='integer' AND auto_continue_disable_epoch >= 0);
CREATE TRIGGER auto_continue_explicit_disable_epoch
AFTER UPDATE OF auto_continue_on_context_exhaustion ON product_conversations
WHEN NEW.auto_continue_on_context_exhaustion=0
BEGIN
    UPDATE product_conversations SET auto_continue_disable_epoch=auto_continue_disable_epoch+1 WHERE id=NEW.id;
END;
CREATE TABLE ordinary_self_goal_admissions (
    turn_id INTEGER PRIMARY KEY REFERENCES durable_turns(turn_id),
    workflow_id INTEGER NOT NULL,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id),
    transcript_id TEXT NOT NULL REFERENCES conversations(id),
    disable_epoch INTEGER NOT NULL CHECK(disable_epoch >= 0)
) STRICT;
