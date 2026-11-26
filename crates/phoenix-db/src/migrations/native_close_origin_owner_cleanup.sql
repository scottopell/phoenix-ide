-- Completion provenance is immutable while its Close obligation exists, not an
-- independent lifetime owner. Preserve the historical FKs and remove children in
-- dependency order only after the owning obligation has been removed. Pending
-- mandatory failure delivery still independently restricts aggregate deletion.
DROP TRIGGER native_close_completion_retry_admissions_no_delete;
CREATE TRIGGER native_close_completion_retry_admissions_no_delete
BEFORE DELETE ON native_close_completion_retry_admissions
WHEN EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'native Close completion retry admission is retained');
END;

DROP TRIGGER native_close_completion_retry_source_revocations_no_delete;
CREATE TRIGGER native_close_completion_retry_source_revocations_no_delete
BEFORE DELETE ON native_close_completion_retry_source_revocations
WHEN EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'completion source revocation is retained');
END;

DROP TRIGGER native_close_completion_retry_origins_no_delete;
CREATE TRIGGER native_close_completion_retry_origins_no_delete
BEFORE DELETE ON native_close_completion_retry_origins
WHEN EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'native Close completion retry origin is retained');
END;

CREATE TRIGGER close_obligations_cleanup_native_completion_retry_owners
AFTER DELETE ON close_obligations
BEGIN
    DELETE FROM native_close_completion_retry_admissions
    WHERE attempt_id = OLD.attempt_id;
    DELETE FROM native_close_completion_retry_source_revocations
    WHERE attempt_id = OLD.attempt_id;
    DELETE FROM native_close_completion_retry_origins
    WHERE attempt_id = OLD.attempt_id;
END;
