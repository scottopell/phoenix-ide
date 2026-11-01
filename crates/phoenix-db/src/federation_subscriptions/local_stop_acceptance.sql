DROP TRIGGER federation_accept_turn;
DROP TRIGGER federation_accept_steering;

CREATE TRIGGER federation_accept_turn AFTER INSERT ON durable_turns
WHEN NEW.origin_subscription_event_id GLOB 'federation:*'
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM federation_remote_admissions a JOIN federation_remote_subscriptions s
          ON s.destination_instance_id = a.destination_instance_id AND s.subscription_id = a.subscription_id
        WHERE a.admission_id = NEW.origin_subscription_event_id AND s.state != 'active'
    ) THEN RAISE(ABORT, 'federation subscription locally stopped') END;
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
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM federation_remote_admissions a JOIN federation_remote_subscriptions s
          ON s.destination_instance_id = a.destination_instance_id AND s.subscription_id = a.subscription_id
        WHERE a.admission_id = NEW.origin_subscription_event_id AND s.state != 'active'
    ) THEN RAISE(ABORT, 'federation subscription locally stopped') END;
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
