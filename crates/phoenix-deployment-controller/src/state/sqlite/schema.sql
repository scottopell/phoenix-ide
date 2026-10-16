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
PRAGMA user_version = 2;
