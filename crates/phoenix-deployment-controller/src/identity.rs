use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProcessId(NonZeroU32);

impl ProcessId {
    #[must_use]
    pub const fn new(raw: NonZeroU32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    #[must_use]
    pub const fn is_protected(self) -> bool {
        self.get() == 1
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BirthToken {
    seconds: u64,
    nanos: u32,
}

impl BirthToken {
    pub fn new(seconds: u64, nanos: u32) -> Result<Self, InvalidBirthToken> {
        if nanos >= 1_000_000_000 {
            return Err(InvalidBirthToken);
        }
        Ok(Self { seconds, nanos })
    }

    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.seconds
    }

    #[must_use]
    pub const fn nanos(self) -> u32 {
        self.nanos
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidBirthToken;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProcessIdentity {
    pid: ProcessId,
    birth: BirthToken,
}

impl ProcessIdentity {
    #[must_use]
    pub const fn new(pid: ProcessId, birth: BirthToken) -> Self {
        Self { pid, birth }
    }

    #[must_use]
    pub const fn pid(self) -> ProcessId {
        self.pid
    }

    #[must_use]
    pub const fn birth(self) -> BirthToken {
        self.birth
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DiscoveryId(u64);

impl DiscoveryId {
    #[must_use]
    pub const fn new(sequence: u64) -> Self {
        Self(sequence)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CandidateDiscovery {
    id: DiscoveryId,
    hinted_pid: ProcessId,
    verified_parent: ProcessIdentity,
}

impl CandidateDiscovery {
    #[must_use]
    pub const fn new(
        id: DiscoveryId,
        hinted_pid: ProcessId,
        verified_parent: ProcessIdentity,
    ) -> Self {
        Self {
            id,
            hinted_pid,
            verified_parent,
        }
    }

    #[must_use]
    pub const fn id(self) -> DiscoveryId {
        self.id
    }

    #[must_use]
    pub const fn hinted_pid(self) -> ProcessId {
        self.hinted_pid
    }

    #[must_use]
    pub const fn verified_parent(self) -> ProcessIdentity {
        self.verified_parent
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableChildEvidence {
    discovery: CandidateDiscovery,
    identity: ProcessIdentity,
}

impl StableChildEvidence {
    pub fn new(
        discovery: CandidateDiscovery,
        identity: ProcessIdentity,
        observed_parent: ProcessIdentity,
    ) -> Result<Self, ChildEvidenceMismatch> {
        if identity.pid() != discovery.hinted_pid()
            || observed_parent != discovery.verified_parent()
            || identity.pid().is_protected()
        {
            return Err(ChildEvidenceMismatch);
        }
        Ok(Self {
            discovery,
            identity,
        })
    }

    #[must_use]
    pub const fn discovery(self) -> CandidateDiscovery {
        self.discovery
    }

    #[must_use]
    pub const fn identity(self) -> ProcessIdentity {
        self.identity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildEvidenceMismatch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationFailure {
    NoSuchProcess,
    PermissionDenied,
    TemporarilyUnavailable,
    Malformed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildIdentityObservation {
    Stable(StableChildEvidence),
    Unresolved(ObservationFailure),
    DifferentProcess(ProcessIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityObservation {
    SameIdentity {
        current_parent: Option<ProcessIdentity>,
    },
    ExactIdentityAbsent,
    PidReused(ProcessIdentity),
    Unresolved(ObservationFailure),
}
