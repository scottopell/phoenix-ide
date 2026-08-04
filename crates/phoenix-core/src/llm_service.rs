//! Narrow LLM-capability traits for the acyclic base crate.
//!
//! These traits let downstream crates (e.g. `tools`) depend on a single
//! completion capability without pulling in the full provider error taxonomy
//! (`LlmError`, `QuotaDetails`, `TokenChunk`) or streaming machinery that lives
//! in the `llm` module. The concrete `LlmService` / `ModelRegistry` types in the
//! `llm` module implement these via thin bridges.

use crate::domain::llm_types::{LlmRequest, LlmResponse};
use std::sync::Arc;

/// A single LLM completion capability. The error is a plain String so this
/// base-crate trait stays free of the provider error taxonomy (`LlmError`,
/// `QuotaDetails`, `TokenChunk`) — callers that need rich errors use the
/// concrete service in the `llm` module directly.
#[async_trait::async_trait]
pub trait CompletionService: Send + Sync {
    /// Non-streaming completion.
    ///
    /// # Errors
    /// Returns a display string describing the failure (the rich error type is
    /// flattened to text at the bridge so this trait carries no provider error
    /// taxonomy).
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, String>;
}

#[derive(Clone)]
pub struct CompletionSelection {
    pub model_id: String,
    pub service: Arc<dyn CompletionService>,
    pub effective_effort: crate::domain::llm_types::EffectiveEffort,
    pub max_output_tokens: Option<u32>,
}

/// Selects a completion service by model id. Implemented by the concrete
/// `ModelRegistry` in the `llm` module.
pub trait LlmSelector: Send + Sync {
    /// Service for a specific model id, if available.
    fn get(&self, model_id: &str) -> Option<Arc<dyn CompletionService>>;
    /// Default/fallback service, if any model is configured.
    fn default_service(&self) -> Option<Arc<dyn CompletionService>>;
    fn default_selection(&self) -> Option<CompletionSelection> {
        self.default_service().map(|service| CompletionSelection {
            model_id: "unknown".to_string(),
            service,
            effective_effort: crate::domain::llm_types::EffectiveEffort::native_unknown(),
            max_output_tokens: None,
        })
    }
    /// A fast, cheap model for auxiliary work (result filtering, titles). The
    /// concrete preference list spans the supported providers and is owned by
    /// the implementor so there is one source of truth; the default here just
    /// uses the default service for selectors that make no cheap-versus-capable
    /// distinction.
    fn get_cheap_model(&self) -> Option<Arc<dyn CompletionService>> {
        self.default_service()
    }

    fn get_cheap_selection(&self) -> Option<CompletionSelection> {
        self.get_cheap_model().map(|service| CompletionSelection {
            model_id: "unknown".to_string(),
            service,
            effective_effort: crate::domain::llm_types::EffectiveEffort::native_unknown(),
            max_output_tokens: None,
        })
    }
}
