CREATE TABLE product_creation_initial_execution_receipts (
    request_id TEXT PRIMARY KEY NOT NULL REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE,
    terminal_kind TEXT NOT NULL CHECK (terminal_kind IN ('Completed', 'Failed', 'Cancelled')),
    failure_reason TEXT,
    CHECK ((terminal_kind = 'Failed' AND failure_reason IS NOT NULL AND typeof(failure_reason) = 'text')
        OR (terminal_kind IN ('Completed', 'Cancelled') AND failure_reason IS NULL))
);
CREATE TRIGGER product_creation_initial_execution_receipt_immutable
BEFORE UPDATE ON product_creation_initial_execution_receipts
BEGIN SELECT RAISE(ABORT, 'initial execution receipt is immutable'); END;
