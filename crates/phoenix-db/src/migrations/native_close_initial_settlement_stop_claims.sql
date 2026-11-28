CREATE TABLE native_close_initial_settlement_stop_claims (
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL CHECK (run_ordinal = 1),
    turn_id INTEGER NOT NULL,
    expected_generation INTEGER NOT NULL CHECK (expected_generation >= 0),
    PRIMARY KEY (attempt_id, run_ordinal, turn_id, expected_generation),
    FOREIGN KEY (attempt_id, run_ordinal)
        REFERENCES close_runs(attempt_id, run_ordinal) ON DELETE CASCADE,
    FOREIGN KEY (attempt_id, turn_id)
        REFERENCES close_attempt_direct_turn_settlements(attempt_id, turn_id) ON DELETE CASCADE
) STRICT;
