use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObservedBranchIdentity {
    pub repository_identity: String,
    pub branch_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedBranchRecord {
    pub identity: ObservedBranchIdentity,
    pub first_observed_head_oid: String,
    pub last_observed_head_oid: String,
    pub first_observed_at: String,
    pub last_observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalGitHeadObservation {
    NamedBranch {
        repository_identity: String,
        branch_name: String,
        head_oid: String,
    },
    Detached {
        repository_identity: String,
        head_oid: String,
    },
    Unborn {
        repository_identity: String,
        branch_name: Option<String>,
    },
    Unavailable {
        repository_identity: Option<String>,
        error: String,
    },
}

impl LocalGitHeadObservation {
    #[must_use]
    pub fn repository_identity(&self) -> Option<&str> {
        match self {
            Self::NamedBranch {
                repository_identity,
                ..
            }
            | Self::Detached {
                repository_identity,
                ..
            }
            | Self::Unborn {
                repository_identity,
                ..
            } => Some(repository_identity.as_str()),
            Self::Unavailable {
                repository_identity,
                ..
            } => repository_identity.as_deref(),
        }
    }
}
