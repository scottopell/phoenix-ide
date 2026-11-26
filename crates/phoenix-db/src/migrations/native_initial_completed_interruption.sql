CREATE VIEW native_close_initial_complete_retirement_proofs AS
SELECT receipt.request_id, receipt.request_fingerprint,
       receipt.source_transcript_id, receipt.source_message_id, receipt.source_tool_use_id,
       receipt.product_conversation_id, receipt.attempt_id, receipt.run_ordinal,
       anchor.inspection_generation, anchor.inspection_fingerprint
FROM native_close_initial_receipts receipt
JOIN close_runs run
  ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = receipt.run_ordinal
JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id
JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = receipt.attempt_id
JOIN close_attempt_direct_turn_settlement_captures settlement
  ON settlement.attempt_id = receipt.attempt_id
WHERE receipt.intent = 'initial' AND receipt.run_ordinal = 1
  AND receipt.request_fingerprint <> ''
  AND run.status IN ('running', 'stopped', 'completed')
  AND obligation.native_managed = 1 AND obligation.topology_sealed = 1
  AND obligation.product_conversation_id = receipt.product_conversation_id
  AND obligation.inspection_generation = anchor.inspection_generation
  AND obligation.inspection_fingerprint = anchor.inspection_fingerprint
  AND NOT EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions excluded
                  WHERE excluded.attempt_id = receipt.attempt_id)
  -- Match Rust's sorted, UTF-8 byte-length-prefixed aggregate encoding.
  -- Empty inspection preserves its immutable original generation; its
  -- fingerprint is still the canonical no-worktree sentinel below.
  AND anchor.inspection_generation = (
      SELECT coalesce('v1' || group_concat(component, ''), anchor.inspection_generation) FROM (
          SELECT length(CAST(scope AS BLOB)) || ':' || scope ||
                 length(CAST(generation AS BLOB)) || ':' || generation AS component
          FROM close_retirement_inspections
          WHERE attempt_id = receipt.attempt_id ORDER BY scope COLLATE BINARY
      )
  )
  AND anchor.inspection_fingerprint = (
      SELECT coalesce('v1' || group_concat(component, ''), 'no-worktree') FROM (
          SELECT length(CAST(scope AS BLOB)) || ':' || scope ||
                 length(CAST(fingerprint AS BLOB)) || ':' || fingerprint AS component
          FROM close_retirement_inspections
          WHERE attempt_id = receipt.attempt_id ORDER BY scope COLLATE BINARY
      )
  )
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_inspections inspection
      WHERE inspection.attempt_id = receipt.attempt_id
        AND NOT EXISTS (SELECT 1 FROM close_attempt_scopes captured
                        JOIN work_scopes scope ON scope.id = captured.scope
                        WHERE captured.attempt_id = inspection.attempt_id
                          AND captured.scope = inspection.scope
                          AND scope.environment_kind = 'allocated_worktree')
  )
  AND NOT EXISTS (
      SELECT 1 FROM close_attempt_scopes captured
      JOIN work_scopes scope ON scope.id = captured.scope
      WHERE captured.attempt_id = receipt.attempt_id
        AND scope.environment_kind = 'allocated_worktree'
        AND NOT EXISTS (SELECT 1 FROM close_retirement_inspections inspection
                        WHERE inspection.attempt_id = captured.attempt_id
                          AND inspection.scope = captured.scope)
  )
  -- Extra evidence is not evidence of completion: reject residuals and any
  -- outcome or dispatch that is not an exact member of this original plan.
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_resources retired
      WHERE retired.attempt_id = receipt.attempt_id
        AND (retired.proof_kind NOT IN ('retired', 'absence_adopted')
          OR NOT EXISTS (
              SELECT 1 FROM close_expected_retirement_resources expected
              WHERE expected.attempt_id = retired.attempt_id AND expected.scope = retired.scope
                AND expected.inspection_generation = retired.inspection_generation
                AND expected.inspection_fingerprint = retired.inspection_fingerprint
                AND expected.resource_kind = retired.resource_kind
                AND expected.identity_kind = retired.identity_kind
                AND expected.identity_codec = retired.identity_codec
                AND expected.identity_value = retired.identity_value
          ))
  )
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_resource_dispatches dispatch
      WHERE dispatch.attempt_id = receipt.attempt_id
        AND NOT EXISTS (
            SELECT 1 FROM close_expected_retirement_resources expected
            WHERE expected.attempt_id = dispatch.attempt_id AND expected.scope = dispatch.scope
              AND expected.inspection_generation = dispatch.inspection_generation
              AND expected.inspection_fingerprint = dispatch.inspection_fingerprint
              AND expected.resource_kind = dispatch.resource_kind
              AND expected.identity_kind = dispatch.identity_kind
              AND expected.identity_codec = dispatch.identity_codec
              AND expected.identity_value = dispatch.identity_value
        )
  )
  AND EXISTS (SELECT 1 FROM close_attempt_participants participant
              WHERE participant.attempt_id = receipt.attempt_id)
  AND NOT EXISTS (
      SELECT 1 FROM close_attempt_participants captured
      JOIN conversations participant ON participant.id = captured.conversation_id
      WHERE captured.attempt_id = receipt.attempt_id
        AND captured.settlement_state = 'live'
        AND (EXISTS (
            SELECT 1 FROM durable_turns turn
            WHERE turn.conversation_id = participant.id
              AND ((turn.owns_conversation = 1 AND turn.terminal_kind IS NULL)
                OR EXISTS (SELECT 1 FROM direct_turn_terminal_obligations terminal
                           WHERE terminal.turn_id = turn.turn_id))
        ) OR EXISTS (
            SELECT 1 FROM conversation_creation_jobs creation
            WHERE creation.conversation_id = participant.id
              AND (creation.status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling')
                OR (creation.status = 'failed' AND EXISTS (
                    SELECT 1 FROM conversation_creation_resource_reservations reservation
                    WHERE reservation.job_id = creation.id AND reservation.status != 'released'
                )))
        ))
  )
  AND NOT EXISTS (SELECT 1 FROM close_attempt_direct_turn_settlements target
                  WHERE target.attempt_id = receipt.attempt_id AND target.settled_at IS NULL)
  AND EXISTS (SELECT 1 FROM close_attempt_scopes captured
              WHERE captured.attempt_id = receipt.attempt_id)
  AND NOT EXISTS (
      SELECT 1 FROM close_attempt_scopes captured
      WHERE captured.attempt_id = receipt.attempt_id
        AND (NOT EXISTS (
            SELECT 1 FROM close_retirement_inventories inventory
            WHERE inventory.attempt_id = captured.attempt_id AND inventory.scope = captured.scope
              AND inventory.inspection_generation = anchor.inspection_generation
              AND inventory.inspection_fingerprint = anchor.inspection_fingerprint
              AND inventory.sealed = 1
        ) OR NOT EXISTS (
            SELECT 1 FROM close_expected_retirement_resources expected
            JOIN close_retirement_resources retired
              ON retired.attempt_id = expected.attempt_id AND retired.scope = expected.scope
             AND retired.inspection_generation = expected.inspection_generation
             AND retired.inspection_fingerprint = expected.inspection_fingerprint
             AND retired.resource_kind = expected.resource_kind
             AND retired.identity_kind = expected.identity_kind
             AND retired.identity_codec = expected.identity_codec
             AND retired.identity_value = expected.identity_value
            WHERE expected.attempt_id = captured.attempt_id AND expected.scope = captured.scope
              AND expected.inspection_generation = anchor.inspection_generation
              AND expected.inspection_fingerprint = anchor.inspection_fingerprint
              AND expected.resource_kind = 'work_scope' AND expected.identity_kind = 'opaque'
              AND expected.identity_codec = 'opaque_string_v1' AND expected.identity_value = captured.scope
              AND retired.proof_kind IN ('retired', 'absence_adopted')
        ))
  )
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_inventories inventory
      WHERE inventory.attempt_id = receipt.attempt_id
        AND (inventory.sealed <> 1
          OR inventory.inspection_generation <> anchor.inspection_generation
          OR inventory.inspection_fingerprint <> anchor.inspection_fingerprint
          OR NOT EXISTS (SELECT 1 FROM close_attempt_scopes captured
                         WHERE captured.attempt_id = inventory.attempt_id AND captured.scope = inventory.scope))
  )
  AND NOT EXISTS (
      SELECT 1 FROM close_expected_retirement_resources expected
      WHERE expected.attempt_id = receipt.attempt_id
        AND (expected.inspection_generation <> anchor.inspection_generation
          OR expected.inspection_fingerprint <> anchor.inspection_fingerprint
          OR NOT EXISTS (SELECT 1 FROM close_attempt_scopes captured
                         WHERE captured.attempt_id = expected.attempt_id AND captured.scope = expected.scope)
          OR NOT EXISTS (
              SELECT 1 FROM close_retirement_resources retired
              WHERE retired.attempt_id = expected.attempt_id AND retired.scope = expected.scope
                AND retired.inspection_generation = expected.inspection_generation
                AND retired.inspection_fingerprint = expected.inspection_fingerprint
                AND retired.resource_kind = expected.resource_kind
                AND retired.identity_kind = expected.identity_kind
                AND retired.identity_codec = expected.identity_codec
                AND retired.identity_value = expected.identity_value
                AND retired.proof_kind IN ('retired', 'absence_adopted')
          ) OR NOT EXISTS (
              SELECT 1 FROM close_retirement_resource_dispatches dispatch
              WHERE dispatch.attempt_id = expected.attempt_id AND dispatch.scope = expected.scope
                AND dispatch.inspection_generation = expected.inspection_generation
                AND dispatch.inspection_fingerprint = expected.inspection_fingerprint
                AND dispatch.resource_kind = expected.resource_kind
                AND dispatch.identity_kind = expected.identity_kind
                AND dispatch.identity_codec = expected.identity_codec
                AND dispatch.identity_value = expected.identity_value
          ) OR (expected.resource_kind IN ('bash_process_group', 'pty_session', 'browser_session')
            AND NOT EXISTS (
              SELECT 1 FROM close_process_step_successes success
              WHERE success.attempt_id = expected.attempt_id AND success.run_ordinal = 1
                AND success.scope = expected.scope AND success.resource_kind = expected.resource_kind
                AND expected.identity_kind = 'opaque' AND expected.identity_codec = 'opaque_string_v1'
                AND success.identity_value = expected.identity_value
                AND success.outcome IN ('retired', 'absence_verified')
          )))
  );
