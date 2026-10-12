//! Retry/resume classification for LLM errors. Co-owned by the llm error
//! taxonomy and the persisted error-kind schema, so it lives in the base crate.

pub const GENERIC_MAX_ATTEMPTS: u32 = 3;
pub const OVERLOAD_MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoRetryPolicy {
    /// Existing transient-error policy: three total attempts.
    Generic {
        max_attempts: u32,
    },
    /// Capacity-specific policy, kept distinct from generic transient failures.
    ServerOverloaded {
        max_attempts: u32,
    },
    NoAutoRetry,
}

impl AutoRetryPolicy {
    #[must_use]
    pub fn allows_auto_retry(self) -> bool {
        !matches!(self, Self::NoAutoRetry)
    }

    #[must_use]
    pub fn max_attempts(self) -> Option<u32> {
        match self {
            Self::Generic { max_attempts } | Self::ServerOverloaded { max_attempts } => {
                Some(max_attempts)
            }
            Self::NoAutoRetry => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserResumePolicy {
    Resumable,
    NotResumable,
}

impl UserResumePolicy {
    #[must_use]
    pub fn allows_user_resume(self) -> bool {
        matches!(self, Self::Resumable)
    }
}
