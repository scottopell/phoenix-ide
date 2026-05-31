//! Retry/resume classification for LLM errors. Co-owned by the llm error
//! taxonomy and the persisted error-kind schema, so it lives in the base crate.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoRetryPolicy {
    AutoRetryable,
    NoAutoRetry,
}

impl AutoRetryPolicy {
    #[must_use]
    pub fn allows_auto_retry(self) -> bool {
        matches!(self, Self::AutoRetryable)
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
