CREATE TABLE attempts (
    attempt_id BLOB PRIMARY KEY NOT NULL CHECK(length(attempt_id) = 16),
    deadline BLOB NOT NULL CHECK(length(deadline) = 8),
    revision BLOB NOT NULL CHECK(length(revision) = 8 AND revision != zeroblob(8)),
    phase TEXT NOT NULL CHECK(phase IN ('sampling','discovery','awaiting','cleaning','verifying','recovering','ready','blocked','interrupted','forward','closeout','closed')),
    interrupted_phase TEXT CHECK(interrupted_phase IN ('sampling','discovery','awaiting','cleaning','verifying','recovering')),
    recovery_resume TEXT CHECK(recovery_resume IN ('discovery','awaiting','cleaning','verifying')),
    ownership_cleanup TEXT CHECK(ownership_cleanup IN ('not_started','in_progress')),
    block_reason TEXT CHECK(block_reason IN ('memory','storage','cpu','pressure')),
    verification TEXT CHECK(verification IN ('exact','missing','mismatch','unavailable')),
    compatibility TEXT NOT NULL CHECK(compatibility IN ('pre_migration','candidate_invoked')),
    CHECK(compatibility = 'pre_migration' OR phase IN ('ready','forward')),
    CHECK(phase != 'forward' OR compatibility = 'candidate_invoked'),
    CHECK((phase = 'interrupted') = (interrupted_phase IS NOT NULL)),
    CHECK((coalesce(interrupted_phase, phase) = 'recovering') = (recovery_resume IS NOT NULL)),
    CHECK((coalesce(interrupted_phase, phase) IN ('discovery','awaiting','cleaning','verifying','recovering')) = (ownership_cleanup IS NOT NULL)),
    CHECK((phase = 'blocked') = (block_reason IS NOT NULL)),
    CHECK(verification IS NULL OR phase = 'forward')
) STRICT;
CREATE TABLE retained_candidates (
    attempt_id BLOB PRIMARY KEY NOT NULL REFERENCES attempts ON DELETE CASCADE,
    source_commit BLOB NOT NULL CHECK(length(source_commit) = 20),
    artifact_digest BLOB NOT NULL CHECK(length(artifact_digest) = 32)
) STRICT;
CREATE TABLE resource_policies (
    attempt_id BLOB PRIMARY KEY NOT NULL REFERENCES attempts ON DELETE CASCADE,
    required_samples BLOB NOT NULL CHECK(length(required_samples) = 8 AND required_samples != zeroblob(8)),
    minimum_memory BLOB NOT NULL CHECK(length(minimum_memory) = 8),
    minimum_storage BLOB NOT NULL CHECK(length(minimum_storage) = 8),
    maximum_cpu INTEGER NOT NULL CHECK(maximum_cpu BETWEEN 0 AND 10000),
    maximum_pressure TEXT NOT NULL CHECK(maximum_pressure IN ('nominal','warning','critical'))
) STRICT;
CREATE TABLE resource_samples (
    attempt_id BLOB NOT NULL REFERENCES attempts ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    sequence BLOB NOT NULL CHECK(length(sequence) = 8),
    available_memory BLOB NOT NULL CHECK(length(available_memory) = 8),
    available_storage BLOB NOT NULL CHECK(length(available_storage) = 8),
    cpu INTEGER NOT NULL CHECK(cpu BETWEEN 0 AND 10000),
    pressure TEXT NOT NULL CHECK(pressure IN ('nominal','warning','critical')),
    PRIMARY KEY(attempt_id, ordinal),
    UNIQUE(attempt_id, sequence)
) STRICT;
CREATE TABLE process_identities (
    attempt_id BLOB NOT NULL REFERENCES attempts ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    pid INTEGER NOT NULL CHECK(pid BETWEEN 1 AND 4294967295),
    birth_seconds BLOB NOT NULL CHECK(length(birth_seconds) = 8),
    birth_nanos INTEGER NOT NULL CHECK(birth_nanos BETWEEN 0 AND 999999999),
    PRIMARY KEY(attempt_id, ordinal),
    UNIQUE(attempt_id, pid, birth_seconds, birth_nanos)
) STRICT;
CREATE TABLE sampling_obligations (
    attempt_id BLOB PRIMARY KEY NOT NULL REFERENCES attempts ON DELETE CASCADE,
    root_ordinal INTEGER NOT NULL,
    pending_sequence BLOB NOT NULL CHECK(length(pending_sequence) = 8),
    FOREIGN KEY(attempt_id, root_ordinal) REFERENCES process_identities(attempt_id, ordinal)
) STRICT;
CREATE TABLE ownership (
    attempt_id BLOB PRIMARY KEY NOT NULL REFERENCES attempts ON DELETE CASCADE,
    next_discovery BLOB NOT NULL CHECK(length(next_discovery) = 8)
) STRICT;
CREATE TABLE owned_identities (
    attempt_id BLOB NOT NULL REFERENCES ownership ON DELETE CASCADE,
    identity_ordinal INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('fresh','present','absent','uncertain')),
    current_parent_ordinal INTEGER,
    failure TEXT CHECK(failure IN ('no_such_process','permission','unavailable','malformed')),
    PRIMARY KEY(attempt_id, identity_ordinal),
    FOREIGN KEY(attempt_id, identity_ordinal) REFERENCES process_identities(attempt_id, ordinal),
    FOREIGN KEY(attempt_id, current_parent_ordinal) REFERENCES process_identities(attempt_id, ordinal),
    CHECK(current_parent_ordinal IS NULL OR status = 'present'),
    CHECK((status = 'uncertain') = (failure IS NOT NULL))
) STRICT;
CREATE TABLE pending_child_scans (
    attempt_id BLOB NOT NULL,
    parent_ordinal INTEGER NOT NULL,
    PRIMARY KEY(attempt_id, parent_ordinal),
    FOREIGN KEY(attempt_id, parent_ordinal) REFERENCES owned_identities(attempt_id, identity_ordinal) ON DELETE CASCADE
) STRICT;
CREATE TABLE discovery_epochs (
    attempt_id BLOB NOT NULL REFERENCES ownership ON DELETE CASCADE,
    discovery_id BLOB NOT NULL CHECK(length(discovery_id) = 8),
    hinted_pid INTEGER NOT NULL CHECK(hinted_pid BETWEEN 1 AND 4294967295),
    parent_ordinal INTEGER NOT NULL,
    gap TEXT NOT NULL CHECK(gap IN ('not_observed','observation','different_process')),
    failure TEXT CHECK(failure IN ('no_such_process','permission','unavailable','malformed')),
    replacement_ordinal INTEGER,
    PRIMARY KEY(attempt_id, discovery_id),
    FOREIGN KEY(attempt_id, parent_ordinal) REFERENCES owned_identities(attempt_id, identity_ordinal),
    FOREIGN KEY(attempt_id, replacement_ordinal) REFERENCES process_identities(attempt_id, ordinal),
    CHECK((gap = 'observation') = (failure IS NOT NULL)),
    CHECK((gap = 'different_process') = (replacement_ordinal IS NOT NULL))
) STRICT;
CREATE TABLE guardian_bindings (
    attempt_id BLOB PRIMARY KEY NOT NULL REFERENCES attempts(attempt_id),
    nonce BLOB NOT NULL UNIQUE CHECK(length(nonce) = 16 AND nonce != zeroblob(16)),
    guardian_pid INTEGER NOT NULL CHECK(guardian_pid BETWEEN 1 AND 4294967295),
    guardian_birth_seconds BLOB NOT NULL CHECK(length(guardian_birth_seconds) = 8),
    guardian_birth_nanos INTEGER NOT NULL CHECK(guardian_birth_nanos BETWEEN 0 AND 999999999),
    revision BLOB NOT NULL CHECK(length(revision) = 8 AND revision != zeroblob(8)),
    dispatch_floor BLOB NOT NULL CHECK(length(dispatch_floor) = 8 AND dispatch_floor != zeroblob(8)),
    slot_count INTEGER NOT NULL CHECK(slot_count BETWEEN 0 AND 64),
    UNIQUE(guardian_pid, guardian_birth_seconds, guardian_birth_nanos)
) STRICT;
CREATE TABLE guardian_sessions (
    attempt_id BLOB NOT NULL REFERENCES guardian_bindings(attempt_id),
    generation BLOB NOT NULL CHECK(length(generation) = 8 AND generation != zeroblob(8)),
    interruption_revision BLOB NOT NULL CHECK(length(interruption_revision) = 8 AND interruption_revision != zeroblob(8)),
    state TEXT NOT NULL CHECK(state IN ('active','detached','reconciling')),
    PRIMARY KEY(attempt_id, generation),
    UNIQUE(attempt_id, generation, interruption_revision),
    UNIQUE(attempt_id, interruption_revision)
) STRICT;
CREATE UNIQUE INDEX guardian_one_live_session ON guardian_sessions(attempt_id) WHERE state IN ('active','reconciling');
CREATE TABLE guardian_slots (
    attempt_id BLOB NOT NULL REFERENCES guardian_bindings(attempt_id),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0 AND ordinal < 64),
    pid INTEGER NOT NULL CHECK(pid BETWEEN 1 AND 4294967295),
    birth_seconds BLOB NOT NULL CHECK(length(birth_seconds) = 8),
    birth_nanos INTEGER NOT NULL CHECK(birth_nanos BETWEEN 0 AND 999999999),
    liveness TEXT NOT NULL CHECK(liveness IN ('running','exited','unavailable')),
    PRIMARY KEY(attempt_id, ordinal),
    UNIQUE(pid, birth_seconds, birth_nanos)
) STRICT;
CREATE TABLE native_sampling_issues (
    attempt_id BLOB NOT NULL REFERENCES guardian_bindings(attempt_id),
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision) = 8 AND dispatch_revision != zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal = 0),
    session_generation BLOB NOT NULL CHECK(length(session_generation) = 8 AND session_generation != zeroblob(8)),
    sequence BLOB NOT NULL CHECK(length(sequence) = 8),
    PRIMARY KEY(attempt_id, dispatch_revision, ordinal),
    UNIQUE(attempt_id, session_generation, sequence),
    FOREIGN KEY(attempt_id, session_generation) REFERENCES guardian_sessions(attempt_id, generation)
) STRICT;
CREATE TABLE native_sampling_applications (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision) = 8 AND dispatch_revision != zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal = 0),
    applying_revision BLOB NOT NULL CHECK(length(applying_revision) = 8 AND applying_revision != zeroblob(8)),
    outcome TEXT NOT NULL CHECK(outcome IN ('sample','temporarily_unavailable','unsupported','malformed')),
    available_memory BLOB CHECK(available_memory IS NULL OR length(available_memory) = 8),
    available_storage BLOB CHECK(available_storage IS NULL OR length(available_storage) = 8),
    cpu INTEGER CHECK(cpu BETWEEN 0 AND 10000),
    pressure TEXT CHECK(pressure IN ('nominal','warning','critical')),
    PRIMARY KEY(attempt_id, dispatch_revision, ordinal),
    UNIQUE(attempt_id, dispatch_revision, ordinal, applying_revision),
    FOREIGN KEY(attempt_id, dispatch_revision, ordinal)
        REFERENCES native_sampling_issues(attempt_id, dispatch_revision, ordinal),
    CHECK((outcome = 'sample') = (available_memory IS NOT NULL)),
    CHECK((outcome = 'sample') = (available_storage IS NOT NULL)),
    CHECK((outcome = 'sample') = (cpu IS NOT NULL)),
    CHECK((outcome = 'sample') = (pressure IS NOT NULL))
) STRICT;
CREATE TABLE native_sampling_ack_confirmations (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision) = 8 AND dispatch_revision != zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal = 0),
    applying_revision BLOB NOT NULL CHECK(length(applying_revision) = 8 AND applying_revision != zeroblob(8)),
    confirming_session_generation BLOB NOT NULL CHECK(length(confirming_session_generation) = 8 AND confirming_session_generation != zeroblob(8)),
    PRIMARY KEY(attempt_id, dispatch_revision, ordinal),
    FOREIGN KEY(attempt_id, dispatch_revision, ordinal, applying_revision)
        REFERENCES native_sampling_applications(attempt_id, dispatch_revision, ordinal, applying_revision),
    FOREIGN KEY(attempt_id, confirming_session_generation)
        REFERENCES guardian_sessions(attempt_id, generation)
) STRICT;
CREATE TRIGGER native_sampling_ack_requires_successful_sample
BEFORE INSERT ON native_sampling_ack_confirmations
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM native_sampling_applications
    WHERE attempt_id = NEW.attempt_id
      AND dispatch_revision = NEW.dispatch_revision
      AND ordinal = NEW.ordinal
      AND applying_revision = NEW.applying_revision
      AND outcome = 'sample'
)
BEGIN
    SELECT RAISE(ABORT, 'native sampling ACK requires successful sample application');
END;
CREATE TRIGGER native_sampling_confirmed_application_is_immutable
BEFORE UPDATE ON native_sampling_applications
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM native_sampling_ack_confirmations
    WHERE attempt_id = OLD.attempt_id
      AND dispatch_revision = OLD.dispatch_revision
      AND ordinal = OLD.ordinal
)
BEGIN
    SELECT RAISE(ABORT, 'confirmed native sampling application is immutable');
END;
CREATE TABLE native_reconciliation_completions (
    attempt_id BLOB NOT NULL,
    session_generation BLOB NOT NULL CHECK(length(session_generation) = 8 AND session_generation != zeroblob(8)),
    interruption_revision BLOB NOT NULL CHECK(length(interruption_revision) = 8 AND interruption_revision != zeroblob(8)),
    recovery_revision BLOB NOT NULL CHECK(length(recovery_revision) = 8 AND recovery_revision > interruption_revision),
    binding_revision BLOB NOT NULL CHECK(length(binding_revision) = 8 AND binding_revision != zeroblob(8)),
    PRIMARY KEY(attempt_id, session_generation),
    UNIQUE(attempt_id, recovery_revision),
    FOREIGN KEY(attempt_id, session_generation, interruption_revision)
        REFERENCES guardian_sessions(attempt_id, generation, interruption_revision)
) STRICT;
CREATE TRIGGER native_reconciliation_completion_requires_exact_preparation
BEFORE INSERT ON native_reconciliation_completions
WHEN NOT EXISTS (
    SELECT 1 FROM attempts a
    JOIN guardian_bindings b USING(attempt_id)
    JOIN guardian_sessions s USING(attempt_id)
    WHERE a.attempt_id=NEW.attempt_id
      AND a.revision=NEW.recovery_revision AND a.phase='sampling'
      AND b.revision=NEW.binding_revision AND b.dispatch_floor=NEW.interruption_revision
      AND s.generation=NEW.session_generation
      AND s.interruption_revision=NEW.interruption_revision AND s.state='reconciling'
      AND s.generation=(SELECT max(generation) FROM guardian_sessions WHERE attempt_id=NEW.attempt_id)
)
BEGIN SELECT RAISE(ABORT, 'completion requires exact prepared checkpoint and session'); END;
CREATE TRIGGER native_reconciliation_completion_is_immutable
BEFORE UPDATE ON native_reconciliation_completions
BEGIN SELECT RAISE(ABORT, 'prepared completion is immutable'); END;
CREATE TRIGGER native_reconciliation_completion_cannot_be_deleted
BEFORE DELETE ON native_reconciliation_completions
BEGIN SELECT RAISE(ABORT, 'prepared completion is immutable'); END;
CREATE TABLE native_reconciliation_activations (
    attempt_id BLOB NOT NULL,
    session_generation BLOB NOT NULL CHECK(length(session_generation)=8 AND session_generation!=zeroblob(8)),
    binding_revision BLOB NOT NULL CHECK(length(binding_revision)=8 AND binding_revision!=zeroblob(8)),
    PRIMARY KEY(attempt_id, session_generation),
    FOREIGN KEY(attempt_id, session_generation)
        REFERENCES native_reconciliation_completions(attempt_id, session_generation)
) STRICT;
CREATE TRIGGER native_reconciliation_activation_requires_exact_active_binding
BEFORE INSERT ON native_reconciliation_activations
WHEN NOT EXISTS (
    SELECT 1 FROM native_reconciliation_completions c
    JOIN attempts a USING(attempt_id)
    JOIN guardian_bindings b USING(attempt_id)
    JOIN guardian_sessions s ON s.attempt_id=c.attempt_id AND s.generation=c.session_generation
    WHERE c.attempt_id=NEW.attempt_id AND c.session_generation=NEW.session_generation
      AND a.revision=c.recovery_revision AND a.phase='sampling'
      AND b.revision=NEW.binding_revision AND b.revision>c.binding_revision
      AND b.dispatch_floor=c.recovery_revision
      AND s.interruption_revision=c.interruption_revision AND s.state='active'
      AND s.generation=(SELECT max(generation) FROM guardian_sessions WHERE attempt_id=NEW.attempt_id)
)
BEGIN SELECT RAISE(ABORT, 'activation requires exact active checkpoint and session'); END;
CREATE TRIGGER native_reconciliation_activation_is_immutable
BEFORE UPDATE ON native_reconciliation_activations
BEGIN SELECT RAISE(ABORT, 'activation is immutable'); END;
CREATE TRIGGER native_reconciliation_activation_cannot_be_deleted
BEFORE DELETE ON native_reconciliation_activations
BEGIN SELECT RAISE(ABORT, 'activation is immutable'); END;
CREATE TABLE native_child_list_issues (
    attempt_id BLOB NOT NULL REFERENCES guardian_bindings(attempt_id),
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision)=8 AND dispatch_revision!=zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal=0),
    session_generation BLOB NOT NULL CHECK(length(session_generation)=8 AND session_generation!=zeroblob(8)),
    parent_pid INTEGER NOT NULL CHECK(parent_pid BETWEEN 1 AND 4294967295),
    parent_birth_seconds BLOB NOT NULL CHECK(length(parent_birth_seconds)=8),
    parent_birth_nanos INTEGER NOT NULL CHECK(parent_birth_nanos BETWEEN 0 AND 999999999),
    PRIMARY KEY(attempt_id,dispatch_revision,ordinal),
    FOREIGN KEY(attempt_id,session_generation) REFERENCES guardian_sessions(attempt_id,generation)
) STRICT;
CREATE TABLE native_child_list_applications (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision)=8 AND dispatch_revision!=zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal=0),
    applying_revision BLOB NOT NULL CHECK(length(applying_revision)=8 AND applying_revision!=zeroblob(8)),
    outcome TEXT NOT NULL CHECK(outcome IN ('success','permission_denied','temporarily_unavailable','malformed')),
    child_count INTEGER CHECK(child_count >= 0),
    CHECK((outcome='success')=(child_count IS NOT NULL)),
    PRIMARY KEY(attempt_id,dispatch_revision,ordinal),
    UNIQUE(attempt_id,dispatch_revision,ordinal,applying_revision),
    FOREIGN KEY(attempt_id,dispatch_revision,ordinal) REFERENCES native_child_list_issues(attempt_id,dispatch_revision,ordinal)
) STRICT;
CREATE TABLE native_child_list_result_children (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL,
    issue_ordinal INTEGER NOT NULL CHECK(issue_ordinal=0),
    child_ordinal INTEGER NOT NULL CHECK(child_ordinal>=0),
    pid INTEGER NOT NULL CHECK(pid BETWEEN 1 AND 4294967295),
    PRIMARY KEY(attempt_id,dispatch_revision,issue_ordinal,child_ordinal),
    UNIQUE(attempt_id,dispatch_revision,issue_ordinal,pid),
    FOREIGN KEY(attempt_id,dispatch_revision,issue_ordinal) REFERENCES native_child_list_applications(attempt_id,dispatch_revision,ordinal)
) STRICT;
CREATE TRIGGER native_child_list_result_children_require_success BEFORE INSERT ON native_child_list_result_children
WHEN NOT EXISTS (SELECT 1 FROM native_child_list_applications WHERE attempt_id=NEW.attempt_id AND dispatch_revision=NEW.dispatch_revision AND ordinal=NEW.issue_ordinal AND outcome='success')
BEGIN SELECT RAISE(ABORT,'child rows require successful application'); END;
CREATE TABLE native_child_list_ack_confirmations (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision)=8 AND dispatch_revision!=zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal=0),
    applying_revision BLOB NOT NULL CHECK(length(applying_revision)=8 AND applying_revision!=zeroblob(8)),
    confirming_session_generation BLOB NOT NULL CHECK(length(confirming_session_generation)=8 AND confirming_session_generation!=zeroblob(8)),
    PRIMARY KEY(attempt_id,dispatch_revision,ordinal),
    FOREIGN KEY(attempt_id,dispatch_revision,ordinal,applying_revision) REFERENCES native_child_list_applications(attempt_id,dispatch_revision,ordinal,applying_revision),
    FOREIGN KEY(attempt_id,confirming_session_generation) REFERENCES guardian_sessions(attempt_id,generation)
) STRICT;
CREATE TRIGGER native_child_list_ack_requires_success BEFORE INSERT ON native_child_list_ack_confirmations
WHEN NOT EXISTS (SELECT 1 FROM native_child_list_applications WHERE attempt_id=NEW.attempt_id AND dispatch_revision=NEW.dispatch_revision AND ordinal=NEW.ordinal AND applying_revision=NEW.applying_revision AND outcome='success')
BEGIN SELECT RAISE(ABORT,'child list ACK requires success'); END;
CREATE TRIGGER native_child_list_issue_immutable BEFORE UPDATE ON native_child_list_issues
BEGIN SELECT RAISE(ABORT,'child list issue is immutable'); END;
CREATE TRIGGER native_child_list_issue_no_delete BEFORE DELETE ON native_child_list_issues
BEGIN SELECT RAISE(ABORT,'child list issue is immutable'); END;
CREATE TRIGGER native_child_list_application_immutable BEFORE UPDATE ON native_child_list_applications
BEGIN SELECT RAISE(ABORT,'child list application is immutable'); END;
CREATE TRIGGER native_child_list_child_immutable BEFORE UPDATE ON native_child_list_result_children
BEGIN SELECT RAISE(ABORT,'child list child is immutable'); END;
CREATE TRIGGER native_child_list_ack_immutable BEFORE UPDATE ON native_child_list_ack_confirmations
BEGIN SELECT RAISE(ABORT,'child list ACK is immutable'); END;
CREATE TRIGGER native_child_list_application_no_delete BEFORE DELETE ON native_child_list_applications
BEGIN SELECT RAISE(ABORT,'child list application is immutable'); END;
CREATE TRIGGER native_child_list_child_no_delete BEFORE DELETE ON native_child_list_result_children
BEGIN SELECT RAISE(ABORT,'child list child is immutable'); END;
CREATE TRIGGER native_child_list_ack_no_delete BEFORE DELETE ON native_child_list_ack_confirmations
BEGIN SELECT RAISE(ABORT,'child list ACK is immutable'); END;
CREATE TABLE native_identity_observation_issues (
    attempt_id BLOB NOT NULL REFERENCES guardian_bindings(attempt_id),
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision)=8 AND dispatch_revision!=zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    session_generation BLOB NOT NULL CHECK(length(session_generation)=8 AND session_generation!=zeroblob(8)),
    target_pid INTEGER NOT NULL CHECK(target_pid BETWEEN 1 AND 4294967295),
    target_birth_seconds BLOB NOT NULL CHECK(length(target_birth_seconds)=8),
    target_birth_nanos INTEGER NOT NULL CHECK(target_birth_nanos BETWEEN 0 AND 999999999),
    PRIMARY KEY(attempt_id,dispatch_revision,ordinal),
    FOREIGN KEY(attempt_id,session_generation) REFERENCES guardian_sessions(attempt_id,generation)
) STRICT;
CREATE TRIGGER native_identity_observation_issue_requires_root BEFORE INSERT ON native_identity_observation_issues
WHEN NOT EXISTS (
    SELECT 1 FROM attempts a JOIN guardian_bindings b USING(attempt_id)
    JOIN guardian_sessions s ON s.attempt_id=b.attempt_id AND s.generation=NEW.session_generation
    JOIN guardian_slots slot ON slot.attempt_id=b.attempt_id
    WHERE a.attempt_id=NEW.attempt_id AND a.revision=NEW.dispatch_revision
      AND a.phase IN ('cleaning','verifying') AND b.dispatch_floor<=NEW.dispatch_revision
      AND s.state='active' AND s.generation=(SELECT max(generation) FROM guardian_sessions WHERE attempt_id=a.attempt_id)
      AND b.slot_count=1 AND slot.ordinal=0 AND slot.pid=NEW.target_pid
      AND slot.birth_seconds=NEW.target_birth_seconds AND slot.birth_nanos=NEW.target_birth_nanos
)
BEGIN SELECT RAISE(ABORT,'identity issue requires exact active owned root'); END;
CREATE TABLE native_identity_observation_applications (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision)=8 AND dispatch_revision!=zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    applying_revision BLOB NOT NULL CHECK(length(applying_revision)=8 AND applying_revision!=zeroblob(8)),
    outcome TEXT NOT NULL CHECK(outcome IN ('same_identity','exact_identity_absent','pid_reused','unresolved')),
    current_parent_pid INTEGER CHECK(current_parent_pid BETWEEN 1 AND 4294967295),
    current_parent_birth_seconds BLOB CHECK(current_parent_birth_seconds IS NULL OR length(current_parent_birth_seconds)=8),
    current_parent_birth_nanos INTEGER CHECK(current_parent_birth_nanos BETWEEN 0 AND 999999999),
    replacement_pid INTEGER CHECK(replacement_pid BETWEEN 1 AND 4294967295),
    replacement_birth_seconds BLOB CHECK(replacement_birth_seconds IS NULL OR length(replacement_birth_seconds)=8),
    replacement_birth_nanos INTEGER CHECK(replacement_birth_nanos BETWEEN 0 AND 999999999),
    failure TEXT CHECK(failure IN ('no_such_process','permission_denied','temporarily_unavailable','malformed')),
    CHECK((current_parent_pid IS NULL)=(current_parent_birth_seconds IS NULL) AND (current_parent_pid IS NULL)=(current_parent_birth_nanos IS NULL)),
    CHECK((replacement_pid IS NULL)=(replacement_birth_seconds IS NULL) AND (replacement_pid IS NULL)=(replacement_birth_nanos IS NULL)),
    CHECK(outcome='same_identity' OR current_parent_pid IS NULL),
    CHECK((outcome='pid_reused')=(replacement_pid IS NOT NULL)),
    CHECK((outcome='unresolved')=(failure IS NOT NULL)),
    PRIMARY KEY(attempt_id,dispatch_revision,ordinal),
    UNIQUE(attempt_id,dispatch_revision,ordinal,applying_revision),
    FOREIGN KEY(attempt_id,dispatch_revision,ordinal) REFERENCES native_identity_observation_issues(attempt_id,dispatch_revision,ordinal)
) STRICT;
CREATE TABLE native_identity_observation_ack_confirmations (
    attempt_id BLOB NOT NULL,
    dispatch_revision BLOB NOT NULL CHECK(length(dispatch_revision)=8 AND dispatch_revision!=zeroblob(8)),
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    applying_revision BLOB NOT NULL CHECK(length(applying_revision)=8 AND applying_revision!=zeroblob(8)),
    confirming_session_generation BLOB NOT NULL CHECK(length(confirming_session_generation)=8 AND confirming_session_generation!=zeroblob(8)),
    PRIMARY KEY(attempt_id,dispatch_revision,ordinal),
    FOREIGN KEY(attempt_id,dispatch_revision,ordinal,applying_revision) REFERENCES native_identity_observation_applications(attempt_id,dispatch_revision,ordinal,applying_revision),
    FOREIGN KEY(attempt_id,confirming_session_generation) REFERENCES guardian_sessions(attempt_id,generation)
) STRICT;
CREATE TRIGGER native_identity_observation_issue_immutable BEFORE UPDATE ON native_identity_observation_issues
BEGIN SELECT RAISE(ABORT,'identity issue immutable'); END;
CREATE TRIGGER native_identity_observation_issue_no_delete BEFORE DELETE ON native_identity_observation_issues
BEGIN SELECT RAISE(ABORT,'identity issue immutable'); END;
CREATE TRIGGER native_identity_observation_application_immutable BEFORE UPDATE ON native_identity_observation_applications
BEGIN SELECT RAISE(ABORT,'identity application immutable'); END;
CREATE TRIGGER native_identity_observation_application_no_delete BEFORE DELETE ON native_identity_observation_applications
BEGIN SELECT RAISE(ABORT,'identity application immutable'); END;
CREATE TRIGGER native_identity_observation_ack_immutable BEFORE UPDATE ON native_identity_observation_ack_confirmations
BEGIN SELECT RAISE(ABORT,'identity ACK immutable'); END;
CREATE TRIGGER native_identity_observation_ack_no_delete BEFORE DELETE ON native_identity_observation_ack_confirmations
BEGIN SELECT RAISE(ABORT,'identity ACK immutable'); END;
PRAGMA user_version = 7;
