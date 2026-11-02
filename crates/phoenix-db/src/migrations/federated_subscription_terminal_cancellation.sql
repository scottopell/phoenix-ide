CREATE TEMP TABLE _m150_no_resumable_gaps (
    count INTEGER NOT NULL CHECK(count = 0)
);
INSERT INTO _m150_no_resumable_gaps(count)
SELECT
    (SELECT COUNT(*) FROM federation_delivery_authorizations WHERE kind = 'gap') +
    (SELECT COUNT(*) FROM federation_remote_admissions WHERE kind = 'gap') +
    (SELECT COUNT(*) FROM federation_received_gaps);
DROP TABLE _m150_no_resumable_gaps;

-- Legacy received facts contain no occurrence identity. They cannot be safely
-- reauthorized or compared with a replayed fact after migration.
CREATE TEMP TABLE _m150_no_unidentified_facts (
    count INTEGER NOT NULL CHECK(count = 0)
);
INSERT INTO _m150_no_unidentified_facts(count)
SELECT COUNT(*) FROM federation_received_facts;
DROP TABLE _m150_no_unidentified_facts;

CREATE TEMP TABLE _m150_subscriptions AS
SELECT id, enrollment_id, product_id, state, last_sequence, acknowledged_through,
       pruned_through, created_at_us
FROM federation_subscriptions;
CREATE TEMP TABLE _m150_terminal_identities AS
SELECT event_id, subscription_id, occurrence_kind, occurrence_id, generation,
       sequence, continuation_state, NULL AS evidence_release_reason
FROM federation_terminal_identities;
CREATE TEMP TABLE _m150_terminal_payloads AS
SELECT event_id, source_transcript_id, outcome, context_exhausted, occurred_at_us,
       length(CAST(source_transcript_id AS BLOB))
           + length(CAST(outcome AS BLOB))
           + 96 AS payload_bytes
FROM federation_terminal_payloads;
CREATE TEMP TABLE _m150_delivery_authorizations AS
SELECT authorization_id, subscription_id, after_cursor, through_cursor, kind,
       event_id, gap_transcript_id, gap_outcome
FROM federation_delivery_authorizations;

CREATE TEMP TABLE _m150_remote_subscriptions AS
SELECT destination_instance_id, subscription_id, product_id, state,
       admitted_cursor, enrolled, last_sync_at_us
FROM federation_remote_subscriptions;
CREATE TEMP TABLE _m150_pending_unsubscribes AS
SELECT destination_instance_id, subscription_id
FROM federation_pending_unsubscribes;
CREATE TEMP TABLE _m150_remote_admissions AS
SELECT admission_id, destination_instance_id, subscription_id, kind,
       after_cursor, through_cursor, accepted_transcript_id, suppressed,
       authorization_requested, authorization_id
FROM federation_remote_admissions;
CREATE TEMP TABLE _m150_remote_navigation AS
SELECT admission_id, stable_path, source_path
FROM federation_remote_navigation;
DROP TRIGGER federation_subscription_revoke;
DROP TRIGGER federation_subscription_close;
DROP TRIGGER federation_continuation_admitted;
DROP TRIGGER federation_continuation_failed;
DROP TRIGGER federation_continuation_succeeded;
DROP TRIGGER federation_accept_turn;
DROP TRIGGER federation_accept_steering;
DROP TRIGGER local_subscription_delivery_identity;

DROP TABLE subscription_delivery_identities;
DROP TABLE federation_remote_navigation;
DROP TABLE federation_received_facts;
DROP TABLE federation_received_gaps;
DROP TABLE federation_remote_admissions;
DROP TABLE federation_pending_unsubscribes;
DROP TABLE federation_remote_subscriptions;
DROP TABLE federation_delivery_authorizations;
DROP TABLE federation_terminal_payloads;
DROP TABLE federation_terminal_identities;
DROP TABLE federation_subscriptions;

CREATE TABLE federation_subscriptions (
    id TEXT PRIMARY KEY NOT NULL,
    enrollment_id TEXT NOT NULL REFERENCES federation_enrollments(id),
    product_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'active'
        CHECK(state IN ('active','unsubscribed','revoked','source_closed','failed_gap')),
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK(last_sequence >= 0),
    acknowledged_through INTEGER NOT NULL DEFAULT 0
        CHECK(acknowledged_through BETWEEN 0 AND last_sequence),
    pruned_through INTEGER NOT NULL DEFAULT 0
        CHECK(pruned_through BETWEEN 0 AND last_sequence),
    created_at_us INTEGER NOT NULL CHECK(created_at_us >= 0),
    last_contact_at_us INTEGER CHECK(last_contact_at_us IS NULL OR last_contact_at_us >= 0)
);
CREATE UNIQUE INDEX federation_subscriptions_active
ON federation_subscriptions(enrollment_id, product_id) WHERE state = 'active';
CREATE TRIGGER federation_subscription_binding_immutable
BEFORE UPDATE OF enrollment_id, product_id ON federation_subscriptions
BEGIN SELECT RAISE(ABORT, 'subscription binding is immutable'); END;
CREATE TRIGGER federation_subscription_cannot_reactivate
BEFORE UPDATE OF state ON federation_subscriptions
WHEN OLD.state != 'active' AND NEW.state != OLD.state
BEGIN SELECT RAISE(ABORT, 'ended subscription cannot reactivate'); END;
CREATE TRIGGER federation_subscription_revoke
AFTER UPDATE OF revoked_at_us ON federation_enrollments
WHEN NEW.revoked_at_us IS NOT NULL
BEGIN
    UPDATE federation_subscriptions SET state = 'revoked'
    WHERE enrollment_id = NEW.id AND state = 'active';
END;
CREATE TRIGGER federation_subscription_close
AFTER UPDATE OF ordinary_lifecycle ON product_conversations
WHEN NEW.ordinary_lifecycle = 'history'
BEGIN
    UPDATE federation_subscriptions SET state = 'source_closed'
    WHERE product_id = NEW.id AND state = 'active';
END;

CREATE TABLE federation_terminal_identities (
    event_id TEXT PRIMARY KEY NOT NULL,
    subscription_id TEXT NOT NULL REFERENCES federation_subscriptions(id) ON DELETE CASCADE,
    occurrence_kind TEXT NOT NULL
        CHECK(occurrence_kind IN ('direct_turn','steering','seeded_fork','interaction_response','creation','continuation_summary')),
    occurrence_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    sequence INTEGER CHECK(sequence > 0),
    continuation_state TEXT NOT NULL CHECK(continuation_state IN ('none','awaiting','suppressed')),
    evidence_release_reason TEXT
        CHECK(evidence_release_reason IS NULL OR evidence_release_reason IN ('settled','trust_revoked','source_closed','subscription_replaced','expired')),
    UNIQUE(subscription_id, occurrence_kind, occurrence_id, generation),
    UNIQUE(subscription_id, sequence)
);
CREATE TABLE federation_terminal_payloads (
    event_id TEXT PRIMARY KEY NOT NULL
        REFERENCES federation_terminal_identities(event_id) ON DELETE CASCADE,
    source_transcript_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','failed','cancelled')),
    context_exhausted INTEGER NOT NULL CHECK(context_exhausted IN (0,1)),
    occurred_at_us INTEGER NOT NULL CHECK(occurred_at_us >= 0),
    payload_bytes INTEGER NOT NULL CHECK(payload_bytes >= 0)
);
CREATE TABLE federation_subscription_gaps (
    subscription_id TEXT PRIMARY KEY NOT NULL
        REFERENCES federation_subscriptions(id) ON DELETE CASCADE,
    last_acknowledged INTEGER NOT NULL CHECK(last_acknowledged >= 0),
    last_produced INTEGER NOT NULL CHECK(last_produced > last_acknowledged),
    reason TEXT NOT NULL CHECK(reason IN ('event_count','payload_bytes','disconnected_age')),
    snapshot_transcript_id TEXT NOT NULL,
    snapshot_outcome TEXT CHECK(snapshot_outcome IN ('completed','failed','cancelled')),
    failed_at_us INTEGER NOT NULL CHECK(failed_at_us >= 0),
    terminal_evidence_expires_at_us INTEGER NOT NULL
        CHECK(terminal_evidence_expires_at_us > failed_at_us)
);
CREATE TABLE federation_delivery_authorizations (
    authorization_id TEXT PRIMARY KEY NOT NULL,
    subscription_id TEXT NOT NULL REFERENCES federation_subscriptions(id) ON DELETE CASCADE,
    after_cursor INTEGER NOT NULL CHECK(after_cursor >= 0),
    through_cursor INTEGER NOT NULL CHECK(through_cursor > after_cursor),
    kind TEXT NOT NULL CHECK(kind IN ('fact','gap')),
    event_id TEXT REFERENCES federation_terminal_identities(event_id) ON DELETE CASCADE,
    gap_reason TEXT CHECK(gap_reason IN ('event_count','payload_bytes','disconnected_age')),
    gap_transcript_id TEXT,
    gap_outcome TEXT CHECK(gap_outcome IN ('completed','failed','cancelled')),
    gap_failed_at_us INTEGER CHECK(gap_failed_at_us IS NULL OR gap_failed_at_us >= 0),
    CHECK((kind = 'fact' AND event_id IS NOT NULL AND gap_reason IS NULL
                         AND gap_transcript_id IS NULL AND gap_outcome IS NULL
                         AND gap_failed_at_us IS NULL)
       OR (kind = 'gap' AND event_id IS NULL AND gap_reason IS NOT NULL
                        AND gap_transcript_id IS NOT NULL AND gap_failed_at_us IS NOT NULL)),
    UNIQUE(subscription_id, after_cursor, through_cursor, kind)
);
CREATE TRIGGER federation_subscription_suppress
AFTER UPDATE OF state ON federation_subscriptions
WHEN NEW.state != 'active'
BEGIN
    UPDATE federation_terminal_identities
    SET evidence_release_reason = CASE NEW.state
        WHEN 'unsubscribed' THEN 'settled'
        WHEN 'revoked' THEN 'trust_revoked'
        WHEN 'source_closed' THEN 'source_closed'
        WHEN 'failed_gap' THEN 'settled'
    END
    WHERE subscription_id = NEW.id
      AND evidence_release_reason IS NULL
      AND event_id IN (SELECT event_id FROM federation_terminal_payloads)
      AND (NEW.state != 'failed_gap'
           OR sequence IS NULL
           OR sequence <= (SELECT last_acknowledged FROM federation_subscription_gaps WHERE subscription_id = NEW.id)
           OR sequence > (SELECT last_produced FROM federation_subscription_gaps WHERE subscription_id = NEW.id));
    DELETE FROM federation_delivery_authorizations
    WHERE subscription_id = NEW.id AND NEW.state != 'failed_gap';
    DELETE FROM federation_terminal_payloads WHERE event_id IN (
        SELECT i.event_id FROM federation_terminal_identities i
        LEFT JOIN federation_subscription_gaps g ON g.subscription_id = i.subscription_id
        WHERE i.subscription_id = NEW.id
          AND (NEW.state != 'failed_gap'
               OR i.sequence IS NULL
               OR i.sequence <= g.last_acknowledged
               OR i.sequence > g.last_produced)
    );
END;
CREATE TRIGGER federation_terminal_evidence_revoke
AFTER UPDATE OF revoked_at_us ON federation_enrollments
WHEN NEW.revoked_at_us IS NOT NULL
BEGIN
    UPDATE federation_terminal_identities
    SET evidence_release_reason = 'trust_revoked'
    WHERE evidence_release_reason IS NULL
      AND event_id IN (SELECT event_id FROM federation_terminal_payloads)
      AND subscription_id IN (
        SELECT id FROM federation_subscriptions WHERE enrollment_id = NEW.id
    );
    DELETE FROM federation_delivery_authorizations
    WHERE subscription_id IN (
        SELECT id FROM federation_subscriptions WHERE enrollment_id = NEW.id
    );
    DELETE FROM federation_terminal_payloads WHERE event_id IN (
        SELECT i.event_id FROM federation_terminal_identities i
        JOIN federation_subscriptions s ON s.id = i.subscription_id
        WHERE s.enrollment_id = NEW.id
    );
END;
CREATE TRIGGER federation_terminal_evidence_close
AFTER UPDATE OF ordinary_lifecycle ON product_conversations
WHEN NEW.ordinary_lifecycle = 'history'
BEGIN
    UPDATE federation_terminal_identities
    SET evidence_release_reason = 'source_closed'
    WHERE evidence_release_reason IS NULL
      AND event_id IN (SELECT event_id FROM federation_terminal_payloads)
      AND subscription_id IN (
        SELECT id FROM federation_subscriptions WHERE product_id = NEW.id
    );
    DELETE FROM federation_delivery_authorizations
    WHERE subscription_id IN (
        SELECT id FROM federation_subscriptions WHERE product_id = NEW.id
    );
    DELETE FROM federation_terminal_payloads WHERE event_id IN (
        SELECT i.event_id FROM federation_terminal_identities i
        JOIN federation_subscriptions s ON s.id = i.subscription_id
        WHERE s.product_id = NEW.id
    );
END;
CREATE TRIGGER federation_terminal_evidence_replace
AFTER INSERT ON federation_subscriptions
BEGIN
    UPDATE federation_terminal_identities
    SET evidence_release_reason = 'subscription_replaced'
    WHERE evidence_release_reason IS NULL
      AND event_id IN (SELECT event_id FROM federation_terminal_payloads)
      AND subscription_id IN (
        SELECT id FROM federation_subscriptions
        WHERE id != NEW.id AND enrollment_id = NEW.enrollment_id
          AND product_id = NEW.product_id AND state = 'failed_gap'
    );
    DELETE FROM federation_delivery_authorizations
    WHERE subscription_id IN (
        SELECT id FROM federation_subscriptions
        WHERE id != NEW.id AND enrollment_id = NEW.enrollment_id
          AND product_id = NEW.product_id AND state = 'failed_gap'
    );
    DELETE FROM federation_terminal_payloads WHERE event_id IN (
        SELECT i.event_id FROM federation_terminal_identities i
        JOIN federation_subscriptions s ON s.id = i.subscription_id
        WHERE s.id != NEW.id AND s.enrollment_id = NEW.enrollment_id
          AND s.product_id = NEW.product_id AND s.state = 'failed_gap'
    );
END;

INSERT INTO federation_subscriptions(
    id, enrollment_id, product_id, state, last_sequence, acknowledged_through,
    pruned_through, created_at_us, last_contact_at_us
)
SELECT id, enrollment_id, product_id, state, last_sequence, acknowledged_through,
       pruned_through, created_at_us, NULL
FROM _m150_subscriptions;
INSERT INTO federation_terminal_identities
SELECT * FROM _m150_terminal_identities;
INSERT INTO federation_terminal_payloads(
    event_id, source_transcript_id, outcome, context_exhausted, occurred_at_us,
    payload_bytes
)
SELECT event_id, source_transcript_id, outcome, context_exhausted, occurred_at_us,
       payload_bytes
FROM _m150_terminal_payloads;
INSERT INTO federation_delivery_authorizations(
    authorization_id, subscription_id, after_cursor, through_cursor, kind,
    event_id, gap_reason, gap_transcript_id, gap_outcome, gap_failed_at_us
)
SELECT authorization_id, subscription_id, after_cursor, through_cursor, kind,
       event_id, NULL, gap_transcript_id, gap_outcome, NULL
FROM _m150_delivery_authorizations;
CREATE TRIGGER federation_subscription_eligible BEFORE INSERT ON federation_subscriptions
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM federation_subscription_create_fences f WHERE f.id = NEW.id
    ) THEN RAISE(ABORT, 'subscription create was fenced by unsubscribe') END;
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM federation_enrollments e, product_conversations p
        WHERE e.id = NEW.enrollment_id AND e.revoked_at_us IS NULL
          AND p.id = NEW.product_id AND p.kind = 'ordinary'
          AND p.ordinary_lifecycle = 'open'
    ) THEN RAISE(ABORT, 'subscription requires active enrollment and open ordinary source') END;
END;

CREATE TRIGGER federation_continuation_admitted
AFTER INSERT ON automatic_continuation_admissions
BEGIN
    UPDATE federation_terminal_identities SET continuation_state = 'awaiting'
    WHERE continuation_state = 'none'
      AND NOT EXISTS (
          SELECT 1 FROM federation_delivery_authorizations a
          WHERE a.event_id = federation_terminal_identities.event_id
      )
      AND event_id IN (
          SELECT event_id FROM federation_terminal_payloads
          WHERE source_transcript_id = NEW.predecessor_conversation_id
            AND outcome = 'failed' AND context_exhausted = 1
      );
END;
CREATE TRIGGER federation_continuation_failed
AFTER UPDATE OF phase ON automatic_continuation_admissions
WHEN NEW.phase = 'failed' AND OLD.phase != 'failed'
BEGIN
    UPDATE federation_terminal_identities SET continuation_state = 'none'
    WHERE continuation_state = 'awaiting' AND event_id IN (
        SELECT event_id FROM federation_terminal_payloads
        WHERE source_transcript_id = NEW.predecessor_conversation_id
    );
END;
CREATE TRIGGER federation_continuation_succeeded
AFTER INSERT ON completed_continuation_handoffs
BEGIN
    UPDATE federation_subscriptions SET pruned_through = MAX(pruned_through, COALESCE((
        SELECT MAX(i.sequence)
        FROM federation_terminal_identities i
        JOIN federation_terminal_payloads p USING(event_id)
        WHERE i.subscription_id = federation_subscriptions.id
          AND i.continuation_state = 'awaiting'
          AND p.source_transcript_id = NEW.predecessor_conversation_id
    ), 0));
    UPDATE federation_terminal_identities SET continuation_state = 'suppressed'
    WHERE continuation_state = 'awaiting' AND event_id IN (
        SELECT event_id FROM federation_terminal_payloads
        WHERE source_transcript_id = NEW.predecessor_conversation_id
    );
    UPDATE federation_terminal_identities
    SET evidence_release_reason = 'settled'
    WHERE continuation_state = 'suppressed'
      AND evidence_release_reason IS NULL
      AND event_id IN (SELECT event_id FROM federation_terminal_payloads);
    DELETE FROM federation_terminal_payloads WHERE event_id IN (
        SELECT event_id FROM federation_terminal_identities
        WHERE continuation_state = 'suppressed'
    );
END;

CREATE TABLE federation_remote_subscriptions (
    destination_instance_id TEXT NOT NULL,
    subscription_id TEXT NOT NULL,
    product_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active'
        CHECK(state IN ('active','unsubscribed','revoked','source_closed','failed_gap')),
    admitted_cursor INTEGER NOT NULL DEFAULT 0 CHECK(admitted_cursor >= 0),
    enrolled INTEGER NOT NULL DEFAULT 0 CHECK(enrolled IN (0,1)),
    last_sync_at_us INTEGER NOT NULL DEFAULT 0 CHECK(last_sync_at_us >= 0),
    PRIMARY KEY(destination_instance_id, subscription_id)
);
CREATE UNIQUE INDEX federation_remote_active
ON federation_remote_subscriptions(destination_instance_id, product_id)
WHERE state = 'active';
CREATE TABLE federation_pending_unsubscribes (
    destination_instance_id TEXT NOT NULL,
    subscription_id TEXT NOT NULL,
    PRIMARY KEY(destination_instance_id, subscription_id),
    FOREIGN KEY(destination_instance_id, subscription_id)
        REFERENCES federation_remote_subscriptions(destination_instance_id, subscription_id)
);
CREATE TRIGGER federation_remote_pending_unsubscribe
BEFORE INSERT ON federation_remote_subscriptions
WHEN EXISTS (
    SELECT 1
    FROM federation_pending_unsubscribes u
    JOIN federation_remote_subscriptions s USING(destination_instance_id,subscription_id)
    WHERE s.destination_instance_id = NEW.destination_instance_id
      AND s.product_id = NEW.product_id
)
BEGIN SELECT RAISE(ABORT, 'remote unsubscribe must be confirmed before reenrollment'); END;
CREATE TABLE federation_remote_admissions (
    admission_id TEXT PRIMARY KEY NOT NULL CHECK(admission_id GLOB 'federation:*'),
    destination_instance_id TEXT NOT NULL,
    subscription_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('fact','gap')),
    after_cursor INTEGER NOT NULL CHECK(after_cursor >= 0),
    through_cursor INTEGER NOT NULL CHECK(through_cursor > after_cursor),
    accepted_transcript_id TEXT,
    suppression_reason TEXT CHECK(suppression_reason IN ('authorization_denied','trust_revoked','source_unreachable','evidence_expired','subscription_ended')),
    authorization_requested INTEGER NOT NULL DEFAULT 0 CHECK(authorization_requested IN (0,1)),
    authorization_id TEXT,
    terminal_settlement_attempts INTEGER NOT NULL DEFAULT 0 CHECK(terminal_settlement_attempts BETWEEN 0 AND 3),
    CHECK(authorization_id IS NULL OR (suppression_reason IS NULL AND authorization_requested = 1)),
    CHECK(accepted_transcript_id IS NULL OR (suppression_reason IS NULL AND ((kind = 'fact' AND authorization_id IS NOT NULL) OR (kind = 'gap' AND authorization_requested = 0 AND authorization_id IS NULL)))),
    CHECK(suppression_reason IS NULL OR (kind = 'fact' AND accepted_transcript_id IS NULL AND authorization_id IS NULL)),
    CHECK(kind = 'fact' OR terminal_settlement_attempts = 0),
    FOREIGN KEY(destination_instance_id, subscription_id)
        REFERENCES federation_remote_subscriptions(destination_instance_id, subscription_id),
    UNIQUE(destination_instance_id, subscription_id, through_cursor, kind),
    UNIQUE(admission_id, kind)
);
CREATE TABLE federation_remote_navigation (
    admission_id TEXT PRIMARY KEY NOT NULL
        REFERENCES federation_remote_admissions(admission_id) ON DELETE CASCADE,
    stable_path TEXT NOT NULL
        CHECK(stable_path GLOB '/c/*' AND instr(stable_path, '?') = 0 AND instr(stable_path, '#') = 0),
    source_path TEXT NOT NULL
        CHECK(source_path GLOB '/c/*[?]source_transcript=*' AND instr(source_path, '#') = 0 AND instr(source_path, '&') = 0)
);
CREATE TRIGGER federation_remote_navigation_required
BEFORE UPDATE OF authorization_id ON federation_remote_admissions
WHEN NEW.authorization_id IS NOT NULL
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM federation_remote_navigation n
        WHERE n.admission_id = NEW.admission_id
    ) THEN RAISE(ABORT, 'authorized delivery requires source navigation') END;
END;
CREATE TABLE federation_received_facts (
    admission_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'fact' CHECK(kind = 'fact'),
    destination_instance_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    occurrence_kind TEXT NOT NULL
        CHECK(occurrence_kind IN ('direct_turn','steering','seeded_fork','interaction_response','creation','continuation_summary')),
    occurrence_id TEXT NOT NULL CHECK(length(occurrence_id) > 0),
    occurrence_generation INTEGER NOT NULL CHECK(occurrence_generation >= 0),
    source_transcript_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','failed','cancelled')),
    occurred_at_us INTEGER NOT NULL CHECK(occurred_at_us >= 0),
    FOREIGN KEY(admission_id, kind)
        REFERENCES federation_remote_admissions(admission_id, kind),
    UNIQUE(destination_instance_id, event_id)
);
CREATE TABLE federation_received_gaps (
    admission_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'gap' CHECK(kind = 'gap'),
    last_acknowledged INTEGER NOT NULL CHECK(last_acknowledged >= 0),
    last_produced INTEGER NOT NULL CHECK(last_produced > last_acknowledged),
    reason TEXT NOT NULL CHECK(reason IN ('event_count','payload_bytes','disconnected_age')),
    failed_at_us INTEGER NOT NULL CHECK(failed_at_us >= 0),
    terminal_evidence_expires_at_us INTEGER NOT NULL
        CHECK(terminal_evidence_expires_at_us > failed_at_us),
    source_transcript_id TEXT NOT NULL,
    current_outcome TEXT CHECK(current_outcome IN ('completed','failed','cancelled')),
    FOREIGN KEY(admission_id, kind)
        REFERENCES federation_remote_admissions(admission_id, kind)
);
CREATE TRIGGER federation_remote_binding_immutable
BEFORE UPDATE OF destination_instance_id, subscription_id, product_id
ON federation_remote_subscriptions
BEGIN SELECT RAISE(ABORT, 'remote subscription binding is immutable'); END;
CREATE TRIGGER federation_remote_no_reactivate
BEFORE UPDATE OF state ON federation_remote_subscriptions
WHEN OLD.state != 'active' AND NEW.state != OLD.state
BEGIN SELECT RAISE(ABORT, 'ended remote subscription cannot reactivate'); END;
CREATE TRIGGER federation_remote_end
AFTER UPDATE OF state ON federation_remote_subscriptions
WHEN NEW.state != 'active'
BEGIN
    UPDATE federation_remote_admissions
    SET suppression_reason = CASE NEW.state
            WHEN 'revoked' THEN 'trust_revoked'
            ELSE 'subscription_ended'
        END,
        authorization_requested = 0,
        authorization_id = NULL
    WHERE destination_instance_id = NEW.destination_instance_id
      AND subscription_id = NEW.subscription_id
      AND accepted_transcript_id IS NULL
      AND NEW.state != 'failed_gap';
    UPDATE federation_remote_admissions
    SET terminal_settlement_attempts = CASE WHEN authorization_requested = 1 THEN 1 ELSE 0 END
    WHERE destination_instance_id = NEW.destination_instance_id
      AND subscription_id = NEW.subscription_id
      AND kind = 'fact'
      AND accepted_transcript_id IS NULL
      AND suppression_reason IS NULL
      AND NEW.state = 'failed_gap';
    DELETE FROM federation_remote_navigation
    WHERE admission_id IN (
        SELECT admission_id FROM federation_remote_admissions
        WHERE destination_instance_id = NEW.destination_instance_id
          AND subscription_id = NEW.subscription_id
          AND accepted_transcript_id IS NULL
          AND suppression_reason IS NOT NULL
    );
END;

INSERT INTO federation_remote_subscriptions
SELECT * FROM _m150_remote_subscriptions;
INSERT INTO federation_pending_unsubscribes
SELECT * FROM _m150_pending_unsubscribes;
INSERT INTO federation_remote_admissions(
    admission_id,
    destination_instance_id,
    subscription_id,
    kind,
    after_cursor,
    through_cursor,
    accepted_transcript_id,
    suppression_reason,
    authorization_requested,
    authorization_id,
    terminal_settlement_attempts
)
SELECT
    admission_id,
    destination_instance_id,
    subscription_id,
    kind,
    after_cursor,
    through_cursor,
    accepted_transcript_id,
    CASE WHEN suppressed = 1 THEN 'subscription_ended' END,
    authorization_requested,
    authorization_id,
    0
FROM _m150_remote_admissions;
INSERT INTO federation_remote_navigation
SELECT * FROM _m150_remote_navigation;
CREATE TRIGGER federation_accept_turn
AFTER INSERT ON durable_turns
WHEN NEW.origin_subscription_event_id GLOB 'federation:*'
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1
        FROM federation_remote_admissions a
        JOIN federation_remote_subscriptions s
          ON s.destination_instance_id = a.destination_instance_id
         AND s.subscription_id = a.subscription_id
        WHERE a.admission_id = NEW.origin_subscription_event_id
          AND NOT (
              (a.kind = 'fact' AND s.state IN ('active','failed_gap'))
              OR (a.kind = 'gap' AND s.state = 'failed_gap')
          )
    ) THEN RAISE(ABORT, 'federation subscription locally stopped') END;

    UPDATE federation_remote_admissions SET accepted_transcript_id = NEW.conversation_id
    WHERE admission_id = NEW.origin_subscription_event_id
      AND accepted_transcript_id IS NULL AND suppression_reason IS NULL
      AND ((kind = 'fact' AND authorization_id IS NOT NULL)
        OR (kind = 'gap' AND authorization_requested = 0 AND authorization_id IS NULL))
      AND EXISTS (
          SELECT 1 FROM federation_remote_subscriptions s
          WHERE s.destination_instance_id = federation_remote_admissions.destination_instance_id
            AND s.subscription_id = federation_remote_admissions.subscription_id
            AND (
                (federation_remote_admissions.kind = 'fact'
                 AND s.state IN ('active','failed_gap'))
                OR (federation_remote_admissions.kind = 'gap'
                    AND s.state = 'failed_gap'
                    AND NOT EXISTS (
                        SELECT 1 FROM federation_remote_admissions prefix
                        WHERE prefix.destination_instance_id = federation_remote_admissions.destination_instance_id
                          AND prefix.subscription_id = federation_remote_admissions.subscription_id
                          AND prefix.kind = 'fact'
                          AND prefix.accepted_transcript_id IS NULL
                          AND prefix.suppression_reason IS NULL
                    ))
            )
      )
      AND ((kind = 'fact' AND EXISTS (
               SELECT 1 FROM federation_received_facts f
               WHERE f.admission_id = federation_remote_admissions.admission_id
           ))
        OR (kind = 'gap' AND EXISTS (
               SELECT 1 FROM federation_received_gaps g
               WHERE g.admission_id = federation_remote_admissions.admission_id
           )))
      AND EXISTS (
          SELECT 1 FROM conversations c
          JOIN product_conversations p ON p.id = c.product_conversation_id
          WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
            AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
            AND p.ordinary_lifecycle IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM close_obligations o
                WHERE o.product_conversation_id = p.id AND o.phase != 'completed'
            )
            AND EXISTS (
                SELECT 1 FROM conversations head
                WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1
            )
      );
    SELECT CASE WHEN changes() != 1
        THEN RAISE(ABORT, 'remote fact no longer deliverable') END;
END;
CREATE TRIGGER federation_accept_steering
AFTER INSERT ON steering_messages
WHEN NEW.origin_subscription_event_id GLOB 'federation:*'
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1
        FROM federation_remote_admissions a
        JOIN federation_remote_subscriptions s
          ON s.destination_instance_id = a.destination_instance_id
         AND s.subscription_id = a.subscription_id
        WHERE a.admission_id = NEW.origin_subscription_event_id
          AND NOT (
              (a.kind = 'fact' AND s.state IN ('active','failed_gap'))
              OR (a.kind = 'gap' AND s.state = 'failed_gap')
          )
    ) THEN RAISE(ABORT, 'federation subscription locally stopped') END;

    UPDATE federation_remote_admissions SET accepted_transcript_id = NEW.conversation_id
    WHERE admission_id = NEW.origin_subscription_event_id
      AND accepted_transcript_id IS NULL AND suppression_reason IS NULL
      AND ((kind = 'fact' AND authorization_id IS NOT NULL)
        OR (kind = 'gap' AND authorization_requested = 0 AND authorization_id IS NULL))
      AND EXISTS (
          SELECT 1 FROM federation_remote_subscriptions s
          WHERE s.destination_instance_id = federation_remote_admissions.destination_instance_id
            AND s.subscription_id = federation_remote_admissions.subscription_id
            AND (
                (federation_remote_admissions.kind = 'fact'
                 AND s.state IN ('active','failed_gap'))
                OR (federation_remote_admissions.kind = 'gap'
                    AND s.state = 'failed_gap'
                    AND NOT EXISTS (
                        SELECT 1 FROM federation_remote_admissions prefix
                        WHERE prefix.destination_instance_id = federation_remote_admissions.destination_instance_id
                          AND prefix.subscription_id = federation_remote_admissions.subscription_id
                          AND prefix.kind = 'fact'
                          AND prefix.accepted_transcript_id IS NULL
                          AND prefix.suppression_reason IS NULL
                    ))
            )
      )
      AND ((kind = 'fact' AND EXISTS (
               SELECT 1 FROM federation_received_facts f
               WHERE f.admission_id = federation_remote_admissions.admission_id
           ))
        OR (kind = 'gap' AND EXISTS (
               SELECT 1 FROM federation_received_gaps g
               WHERE g.admission_id = federation_remote_admissions.admission_id
           )))
      AND EXISTS (
          SELECT 1 FROM conversations c
          JOIN product_conversations p ON p.id = c.product_conversation_id
          WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
            AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
            AND p.ordinary_lifecycle IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM close_obligations o
                WHERE o.product_conversation_id = p.id AND o.phase != 'completed'
            )
            AND EXISTS (
                SELECT 1 FROM conversations head
                WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1
            )
      );
    SELECT CASE WHEN changes() != 1
        THEN RAISE(ABORT, 'remote fact no longer deliverable') END;
END;
CREATE TRIGGER federation_advance_admitted_cursor
AFTER UPDATE OF accepted_transcript_id ON federation_remote_admissions
WHEN OLD.accepted_transcript_id IS NULL
 AND NEW.accepted_transcript_id IS NOT NULL
 AND NEW.kind = 'fact'
BEGIN
    UPDATE federation_remote_subscriptions
    SET admitted_cursor = COALESCE(
        (
            SELECT MIN(a.after_cursor)
            FROM federation_remote_admissions a
            WHERE a.destination_instance_id = NEW.destination_instance_id
              AND a.subscription_id = NEW.subscription_id
              AND a.kind = 'fact'
              AND a.accepted_transcript_id IS NULL
        ),
        (
            SELECT MAX(a.through_cursor)
            FROM federation_remote_admissions a
            WHERE a.destination_instance_id = NEW.destination_instance_id
              AND a.subscription_id = NEW.subscription_id
              AND a.kind = 'fact'
              AND a.accepted_transcript_id IS NOT NULL
        ),
        admitted_cursor
    )
    WHERE destination_instance_id = NEW.destination_instance_id
      AND subscription_id = NEW.subscription_id;
END;

CREATE TABLE subscription_delivery_identities (
    id TEXT PRIMARY KEY NOT NULL,
    local_event_id TEXT UNIQUE REFERENCES coordinator_watch_events(event_id) ON DELETE CASCADE,
    remote_admission_id TEXT UNIQUE REFERENCES federation_remote_admissions(admission_id) ON DELETE CASCADE,
    CHECK ((local_event_id IS NOT NULL AND id = local_event_id
            AND id NOT GLOB 'federation:*' AND remote_admission_id IS NULL)
        OR (remote_admission_id IS NOT NULL AND id = remote_admission_id
            AND local_event_id IS NULL))
);
INSERT INTO subscription_delivery_identities(id, local_event_id)
SELECT event_id, event_id FROM coordinator_watch_events;
INSERT INTO subscription_delivery_identities(id, remote_admission_id)
SELECT admission_id, admission_id FROM federation_remote_admissions;
CREATE TRIGGER local_subscription_delivery_identity
AFTER INSERT ON coordinator_watch_events
BEGIN
    INSERT INTO subscription_delivery_identities(id,local_event_id)
    VALUES (NEW.event_id,NEW.event_id);
END;
CREATE TRIGGER remote_subscription_delivery_identity
AFTER INSERT ON federation_remote_admissions
BEGIN
    INSERT INTO subscription_delivery_identities(id,remote_admission_id)
    VALUES (NEW.admission_id,NEW.admission_id);
END;

DROP TABLE _m150_subscriptions;
DROP TABLE _m150_terminal_identities;
DROP TABLE _m150_terminal_payloads;
DROP TABLE _m150_delivery_authorizations;
DROP TABLE _m150_remote_subscriptions;
DROP TABLE _m150_pending_unsubscribes;
DROP TABLE _m150_remote_admissions;
DROP TABLE _m150_remote_navigation;

