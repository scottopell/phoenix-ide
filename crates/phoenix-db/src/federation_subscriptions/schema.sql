CREATE TABLE federation_subscription_create_fences (
    id TEXT PRIMARY KEY NOT NULL,
    enrollment_id TEXT NOT NULL REFERENCES federation_enrollments(id),
    product_id TEXT NOT NULL
);
CREATE TRIGGER federation_subscription_create_fence_eligible BEFORE INSERT ON federation_subscription_create_fences
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM federation_enrollments e WHERE e.id = NEW.enrollment_id AND e.revoked_at_us IS NULL
    ) OR EXISTS (
        SELECT 1 FROM federation_subscriptions s WHERE s.id = NEW.id
    ) THEN RAISE(ABORT, 'create fence requires active enrollment and absent subscription') END;
END;
CREATE TRIGGER federation_subscription_create_fence_immutable BEFORE UPDATE ON federation_subscription_create_fences
BEGIN SELECT RAISE(ABORT, 'subscription create fence is immutable'); END;

CREATE TABLE federation_subscriptions (
    id TEXT PRIMARY KEY NOT NULL,
    enrollment_id TEXT NOT NULL REFERENCES federation_enrollments(id),
    product_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'active' CHECK(state IN ('active','unsubscribed','revoked','source_closed')),
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK(last_sequence >= 0),
    acknowledged_through INTEGER NOT NULL DEFAULT 0 CHECK(acknowledged_through BETWEEN 0 AND last_sequence),
    pruned_through INTEGER NOT NULL DEFAULT 0 CHECK(pruned_through BETWEEN 0 AND last_sequence),
    created_at_us INTEGER NOT NULL CHECK(created_at_us >= 0)
);
CREATE UNIQUE INDEX federation_subscriptions_active ON federation_subscriptions(enrollment_id, product_id) WHERE state = 'active';
CREATE TRIGGER federation_subscription_eligible BEFORE INSERT ON federation_subscriptions
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM federation_subscription_create_fences f WHERE f.id = NEW.id
    ) THEN RAISE(ABORT, 'subscription create was fenced by unsubscribe') END;
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM federation_enrollments e, product_conversations p
        WHERE e.id = NEW.enrollment_id AND e.revoked_at_us IS NULL
          AND p.id = NEW.product_id AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open'
    ) THEN RAISE(ABORT, 'subscription requires active enrollment and open ordinary source') END;
END;
CREATE TRIGGER federation_subscription_binding_immutable BEFORE UPDATE OF enrollment_id, product_id ON federation_subscriptions
BEGIN SELECT RAISE(ABORT, 'subscription binding is immutable'); END;
CREATE TRIGGER federation_subscription_cannot_reactivate BEFORE UPDATE OF state ON federation_subscriptions
WHEN OLD.state != 'active' AND NEW.state != OLD.state
BEGIN SELECT RAISE(ABORT, 'ended subscription cannot reactivate'); END;
CREATE TRIGGER federation_subscription_revoke AFTER UPDATE OF revoked_at_us ON federation_enrollments
WHEN NEW.revoked_at_us IS NOT NULL
BEGIN
    UPDATE federation_subscriptions SET state = 'revoked' WHERE enrollment_id = NEW.id AND state = 'active';
END;
CREATE TRIGGER federation_subscription_close AFTER UPDATE OF ordinary_lifecycle ON product_conversations
WHEN NEW.ordinary_lifecycle = 'history'
BEGIN
    UPDATE federation_subscriptions SET state = 'source_closed' WHERE product_id = NEW.id AND state = 'active';
END;

-- The identity survives removal of the payload, preventing re-publication.
CREATE TABLE federation_terminal_identities (
    event_id TEXT PRIMARY KEY NOT NULL,
    subscription_id TEXT NOT NULL REFERENCES federation_subscriptions(id) ON DELETE CASCADE,
    occurrence_kind TEXT NOT NULL CHECK(occurrence_kind IN ('direct_turn','steering','seeded_fork','interaction_response','creation','continuation_summary')),
    occurrence_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    sequence INTEGER CHECK(sequence > 0),
    continuation_state TEXT NOT NULL CHECK(continuation_state IN ('none','awaiting','suppressed')),
    UNIQUE(subscription_id, occurrence_kind, occurrence_id, generation),
    UNIQUE(subscription_id, sequence)
);
CREATE TABLE federation_terminal_payloads (
    event_id TEXT PRIMARY KEY NOT NULL REFERENCES federation_terminal_identities(event_id) ON DELETE CASCADE,
    source_transcript_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','failed','cancelled')),
    context_exhausted INTEGER NOT NULL CHECK(context_exhausted IN (0,1)),
    occurred_at_us INTEGER NOT NULL CHECK(occurred_at_us >= 0)
);
CREATE TABLE federation_delivery_authorizations (
    authorization_id TEXT PRIMARY KEY NOT NULL,
    subscription_id TEXT NOT NULL REFERENCES federation_subscriptions(id) ON DELETE CASCADE,
    after_cursor INTEGER NOT NULL CHECK(after_cursor >= 0),
    through_cursor INTEGER NOT NULL CHECK(through_cursor > after_cursor),
    kind TEXT NOT NULL CHECK(kind IN ('fact','gap')),
    event_id TEXT REFERENCES federation_terminal_identities(event_id) ON DELETE CASCADE,
    gap_transcript_id TEXT,
    gap_outcome TEXT CHECK(gap_outcome IN ('completed','failed','cancelled')),
    CHECK((kind = 'fact' AND event_id IS NOT NULL AND gap_transcript_id IS NULL AND gap_outcome IS NULL)
       OR (kind = 'gap' AND event_id IS NULL AND gap_transcript_id IS NOT NULL)),
    UNIQUE(subscription_id, after_cursor, through_cursor, kind)
);
CREATE TRIGGER federation_subscription_suppress AFTER UPDATE OF state ON federation_subscriptions
WHEN NEW.state != 'active'
BEGIN
    DELETE FROM federation_terminal_payloads WHERE event_id IN
        (SELECT i.event_id FROM federation_terminal_identities i WHERE i.subscription_id = NEW.id
         AND NOT EXISTS (SELECT 1 FROM federation_delivery_authorizations a WHERE a.event_id = i.event_id));
END;
CREATE TABLE federation_terminal_occurrences (
    occurrence_kind TEXT NOT NULL,
    occurrence_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    product_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    PRIMARY KEY(occurrence_kind,occurrence_id,generation)
);
CREATE TABLE federation_terminal_current (
    product_id TEXT PRIMARY KEY NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    source_transcript_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','failed','cancelled'))
);
CREATE TRIGGER federation_terminal_projection_changed AFTER UPDATE OF state ON conversations
WHEN OLD.state != NEW.state
BEGIN
    DELETE FROM federation_terminal_current WHERE source_transcript_id = NEW.id;
END;
CREATE TRIGGER federation_continuation_admitted AFTER INSERT ON automatic_continuation_admissions
BEGIN
    UPDATE federation_terminal_identities SET continuation_state = 'awaiting'
    WHERE continuation_state = 'none' AND NOT EXISTS (SELECT 1 FROM federation_delivery_authorizations a WHERE a.event_id = federation_terminal_identities.event_id) AND event_id IN (
        SELECT event_id FROM federation_terminal_payloads
        WHERE source_transcript_id = NEW.predecessor_conversation_id AND outcome = 'failed' AND context_exhausted = 1);
END;
CREATE TRIGGER federation_continuation_failed AFTER UPDATE OF phase ON automatic_continuation_admissions
WHEN NEW.phase = 'failed' AND OLD.phase != 'failed'
BEGIN
    UPDATE federation_terminal_identities SET continuation_state = 'none'
    WHERE continuation_state = 'awaiting' AND event_id IN (
        SELECT event_id FROM federation_terminal_payloads WHERE source_transcript_id = NEW.predecessor_conversation_id);
END;
CREATE TRIGGER federation_continuation_succeeded AFTER INSERT ON completed_continuation_handoffs
BEGIN
    UPDATE federation_subscriptions SET pruned_through = MAX(pruned_through, COALESCE((
        SELECT MAX(i.sequence) FROM federation_terminal_identities i JOIN federation_terminal_payloads p USING(event_id)
        WHERE i.subscription_id = federation_subscriptions.id AND i.continuation_state = 'awaiting'
          AND p.source_transcript_id = NEW.predecessor_conversation_id), 0));
    UPDATE federation_terminal_identities SET continuation_state = 'suppressed'
    WHERE continuation_state = 'awaiting' AND event_id IN (
        SELECT event_id FROM federation_terminal_payloads WHERE source_transcript_id = NEW.predecessor_conversation_id);
    DELETE FROM federation_terminal_payloads WHERE event_id IN
        (SELECT event_id FROM federation_terminal_identities WHERE continuation_state = 'suppressed');
END;

CREATE TABLE federation_remote_subscriptions (
    destination_instance_id TEXT NOT NULL,
    subscription_id TEXT NOT NULL,
    product_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active' CHECK(state IN ('active','unsubscribed','revoked','source_closed')),
    admitted_cursor INTEGER NOT NULL DEFAULT 0 CHECK(admitted_cursor >= 0),
    enrolled INTEGER NOT NULL DEFAULT 0 CHECK(enrolled IN (0,1)),
    last_sync_at_us INTEGER NOT NULL DEFAULT 0 CHECK(last_sync_at_us >= 0),
    PRIMARY KEY(destination_instance_id, subscription_id)
);
CREATE UNIQUE INDEX federation_remote_active ON federation_remote_subscriptions(destination_instance_id, product_id) WHERE state = 'active';
CREATE TABLE federation_pending_unsubscribes (
    destination_instance_id TEXT NOT NULL,
    subscription_id TEXT NOT NULL,
    PRIMARY KEY(destination_instance_id, subscription_id),
    FOREIGN KEY(destination_instance_id, subscription_id) REFERENCES federation_remote_subscriptions(destination_instance_id, subscription_id)
);
CREATE TRIGGER federation_remote_pending_unsubscribe BEFORE INSERT ON federation_remote_subscriptions
WHEN EXISTS (SELECT 1 FROM federation_pending_unsubscribes u JOIN federation_remote_subscriptions s USING(destination_instance_id,subscription_id)
             WHERE s.destination_instance_id = NEW.destination_instance_id AND s.product_id = NEW.product_id)
BEGIN SELECT RAISE(ABORT, 'remote unsubscribe must be confirmed before reenrollment'); END;
CREATE TABLE federation_remote_admissions (
    admission_id TEXT PRIMARY KEY NOT NULL CHECK(admission_id GLOB 'federation:*'),
    destination_instance_id TEXT NOT NULL,
    subscription_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('fact','gap')),
    after_cursor INTEGER NOT NULL CHECK(after_cursor >= 0),
    through_cursor INTEGER NOT NULL CHECK(through_cursor > after_cursor),
    accepted_transcript_id TEXT,
    suppressed INTEGER NOT NULL DEFAULT 0 CHECK(suppressed IN (0,1)),
    authorization_requested INTEGER NOT NULL DEFAULT 0 CHECK(authorization_requested IN (0,1)),
    authorization_id TEXT,
    CHECK(authorization_id IS NULL OR (suppressed = 0 AND authorization_requested = 1)),
    CHECK(accepted_transcript_id IS NULL OR authorization_id IS NOT NULL),
    FOREIGN KEY(destination_instance_id, subscription_id) REFERENCES federation_remote_subscriptions(destination_instance_id, subscription_id),
    UNIQUE(destination_instance_id, subscription_id, through_cursor, kind),
    UNIQUE(admission_id, kind)
);
CREATE TABLE federation_remote_navigation (
    admission_id TEXT PRIMARY KEY NOT NULL REFERENCES federation_remote_admissions(admission_id) ON DELETE CASCADE,
    stable_path TEXT NOT NULL CHECK(stable_path GLOB '/c/*' AND instr(stable_path, '?') = 0 AND instr(stable_path, '#') = 0),
    source_path TEXT NOT NULL CHECK(source_path GLOB '/c/*[?]source_transcript=*' AND instr(source_path, '#') = 0 AND instr(source_path, '&') = 0)
);
CREATE TRIGGER federation_remote_navigation_required BEFORE UPDATE OF authorization_id ON federation_remote_admissions
WHEN NEW.authorization_id IS NOT NULL
BEGIN
    SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM federation_remote_navigation n WHERE n.admission_id = NEW.admission_id)
        THEN RAISE(ABORT, 'authorized delivery requires source navigation') END;
END;
CREATE TABLE federation_received_facts (
    admission_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'fact' CHECK(kind = 'fact'),
    destination_instance_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    source_transcript_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','failed','cancelled')),
    occurred_at_us INTEGER NOT NULL CHECK(occurred_at_us >= 0),
    FOREIGN KEY(admission_id, kind) REFERENCES federation_remote_admissions(admission_id, kind),
    UNIQUE(destination_instance_id, event_id)
);
CREATE TABLE federation_received_gaps (
    admission_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'gap' CHECK(kind = 'gap'),
    source_transcript_id TEXT NOT NULL,
    current_outcome TEXT CHECK(current_outcome IN ('completed','failed','cancelled')),
    FOREIGN KEY(admission_id, kind) REFERENCES federation_remote_admissions(admission_id, kind)
);
CREATE TRIGGER federation_remote_binding_immutable BEFORE UPDATE OF destination_instance_id, subscription_id, product_id ON federation_remote_subscriptions
BEGIN SELECT RAISE(ABORT, 'remote subscription binding is immutable'); END;
CREATE TRIGGER federation_remote_no_reactivate BEFORE UPDATE OF state ON federation_remote_subscriptions
WHEN OLD.state != 'active' AND NEW.state != OLD.state
BEGIN SELECT RAISE(ABORT, 'ended remote subscription cannot reactivate'); END;
CREATE TRIGGER federation_remote_end AFTER UPDATE OF state ON federation_remote_subscriptions
WHEN NEW.state != 'active'
BEGIN
    DELETE FROM federation_remote_navigation
    WHERE admission_id IN (
        SELECT admission_id FROM federation_remote_admissions
        WHERE destination_instance_id = NEW.destination_instance_id
          AND subscription_id = NEW.subscription_id
          AND accepted_transcript_id IS NULL);
    UPDATE federation_remote_admissions
    SET suppressed = 1, authorization_requested = 0, authorization_id = NULL
    WHERE destination_instance_id = NEW.destination_instance_id
      AND subscription_id = NEW.subscription_id
      AND accepted_transcript_id IS NULL;
END;
DROP TRIGGER watch_event_accept_turn;
DROP TRIGGER watch_event_accept_steering;
CREATE TRIGGER watch_event_accept_turn AFTER INSERT ON durable_turns
WHEN NEW.origin_subscription_event_id IS NOT NULL AND NEW.origin_subscription_event_id NOT GLOB 'federation:*'
BEGIN
    UPDATE coordinator_watch_events SET delivery_state = 'accepted', accepted_transcript_id = NEW.conversation_id
    WHERE event_id = NEW.origin_subscription_event_id AND delivery_state = 'pending'
      AND continuation_state = 'none'
      AND (route_kind = 'mandatory_close_failure' OR
           (route_kind = 'subscription' AND EXISTS (
                SELECT 1 FROM coordinator_watches w JOIN product_conversations p
                    ON p.id = w.source_product_conversation_id
                WHERE w.id = coordinator_watch_events.watch_id AND w.ended_at_us IS NULL
                  AND p.ordinary_lifecycle = 'open'
                  AND NOT EXISTS (SELECT 1 FROM close_obligations o
                                  WHERE o.product_conversation_id = p.id AND o.phase != 'completed'))))
      AND EXISTS (SELECT 1 FROM product_conversations p JOIN conversations c
                    ON c.product_conversation_id = p.id
                  WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                    AND EXISTS (SELECT 1 FROM conversations head
                                WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1));
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'watch event no longer deliverable') END;
END;
CREATE TRIGGER watch_event_accept_steering AFTER INSERT ON steering_messages
WHEN NEW.origin_subscription_event_id IS NOT NULL AND NEW.origin_subscription_event_id NOT GLOB 'federation:*'
BEGIN
    UPDATE coordinator_watch_events SET delivery_state = 'accepted', accepted_transcript_id = NEW.conversation_id
    WHERE event_id = NEW.origin_subscription_event_id AND delivery_state = 'pending'
      AND continuation_state = 'none'
      AND (route_kind = 'mandatory_close_failure' OR
           (route_kind = 'subscription' AND EXISTS (
                SELECT 1 FROM coordinator_watches w JOIN product_conversations p
                    ON p.id = w.source_product_conversation_id
                WHERE w.id = coordinator_watch_events.watch_id AND w.ended_at_us IS NULL
                  AND p.ordinary_lifecycle = 'open'
                  AND NOT EXISTS (SELECT 1 FROM close_obligations o
                                  WHERE o.product_conversation_id = p.id AND o.phase != 'completed'))))
      AND EXISTS (SELECT 1 FROM product_conversations p JOIN conversations c
                    ON c.product_conversation_id = p.id
                  WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                    AND EXISTS (SELECT 1 FROM conversations head
                                WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1));
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'watch event no longer deliverable') END;
END;

CREATE TRIGGER federation_accept_turn AFTER INSERT ON durable_turns
WHEN NEW.origin_subscription_event_id GLOB 'federation:*'
BEGIN
    UPDATE federation_remote_admissions SET accepted_transcript_id = NEW.conversation_id
    WHERE admission_id = NEW.origin_subscription_event_id AND accepted_transcript_id IS NULL AND suppressed = 0 AND authorization_id IS NOT NULL
      AND EXISTS (SELECT 1 FROM federation_remote_subscriptions s
                  WHERE s.destination_instance_id = federation_remote_admissions.destination_instance_id
                    AND s.subscription_id = federation_remote_admissions.subscription_id
                    AND s.state = 'active'
                    AND s.admitted_cursor = federation_remote_admissions.after_cursor)
      AND ((kind = 'fact' AND EXISTS (SELECT 1 FROM federation_received_facts f WHERE f.admission_id = federation_remote_admissions.admission_id))
        OR (kind = 'gap' AND EXISTS (SELECT 1 FROM federation_received_gaps g WHERE g.admission_id = federation_remote_admissions.admission_id)))
      AND EXISTS (SELECT 1 FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
                  WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                    AND EXISTS (SELECT 1 FROM conversations head WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1));
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'remote fact no longer deliverable') END;
END;
CREATE TRIGGER federation_accept_steering AFTER INSERT ON steering_messages
WHEN NEW.origin_subscription_event_id GLOB 'federation:*'
BEGIN
    UPDATE federation_remote_admissions SET accepted_transcript_id = NEW.conversation_id
    WHERE admission_id = NEW.origin_subscription_event_id AND accepted_transcript_id IS NULL AND suppressed = 0 AND authorization_id IS NOT NULL
      AND EXISTS (SELECT 1 FROM federation_remote_subscriptions s
                  WHERE s.destination_instance_id = federation_remote_admissions.destination_instance_id
                    AND s.subscription_id = federation_remote_admissions.subscription_id
                    AND s.state = 'active'
                    AND s.admitted_cursor = federation_remote_admissions.after_cursor)
      AND ((kind = 'fact' AND EXISTS (SELECT 1 FROM federation_received_facts f WHERE f.admission_id = federation_remote_admissions.admission_id))
        OR (kind = 'gap' AND EXISTS (SELECT 1 FROM federation_received_gaps g WHERE g.admission_id = federation_remote_admissions.admission_id)))
      AND EXISTS (SELECT 1 FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
                  WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                    AND EXISTS (SELECT 1 FROM conversations head WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1));
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'remote fact no longer deliverable') END;
END;
CREATE TRIGGER federation_advance_admitted_cursor AFTER UPDATE OF accepted_transcript_id ON federation_remote_admissions
WHEN OLD.accepted_transcript_id IS NULL AND NEW.accepted_transcript_id IS NOT NULL
BEGIN
    UPDATE federation_remote_subscriptions SET admitted_cursor = NEW.through_cursor
    WHERE destination_instance_id = NEW.destination_instance_id AND subscription_id = NEW.subscription_id
      AND admitted_cursor = NEW.after_cursor;
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'remote admission cursor changed') END;
END;

CREATE TABLE subscription_delivery_identities (
    id TEXT PRIMARY KEY NOT NULL,
    local_event_id TEXT UNIQUE REFERENCES coordinator_watch_events(event_id) ON DELETE CASCADE,
    remote_admission_id TEXT UNIQUE REFERENCES federation_remote_admissions(admission_id) ON DELETE CASCADE,
    CHECK ((local_event_id IS NOT NULL AND id = local_event_id AND id NOT GLOB 'federation:*' AND remote_admission_id IS NULL)
        OR (remote_admission_id IS NOT NULL AND id = remote_admission_id AND local_event_id IS NULL))
);
INSERT INTO subscription_delivery_identities(id,local_event_id) SELECT event_id,event_id FROM coordinator_watch_events;
CREATE TRIGGER local_subscription_delivery_identity AFTER INSERT ON coordinator_watch_events
BEGIN INSERT INTO subscription_delivery_identities(id,local_event_id) VALUES (NEW.event_id,NEW.event_id); END;
CREATE TRIGGER remote_subscription_delivery_identity AFTER INSERT ON federation_remote_admissions
BEGIN INSERT INTO subscription_delivery_identities(id,remote_admission_id) VALUES (NEW.admission_id,NEW.admission_id); END;
