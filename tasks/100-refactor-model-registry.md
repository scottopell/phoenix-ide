---
created: 2026-02-02
priority: p1
status: not_started
tags: [llm, architecture]
---

# Refactor Model Registry for Dynamic Discovery

## Summary

Refactor Phoenix's hard-coded model registry to use a dynamic, provider-agnostic system similar to Shelley's approach. This will enable proper multi-provider support, correct model discovery, and better gateway integration.

## Context

Current issues with Phoenix's model registry:
1. Models are hard-coded in `register_anthropic_models()`
2. Gateway mode incorrectly assumes all models work without API keys
3. Model API names appear to be incorrect (e.g., Claude4Sonnet → claude-sonnet-4-20250514)
4. No way to add new providers without modifying registry code
5. No model metadata (descriptions, provider info)

## Acceptance Criteria

- [ ] Create `Model` struct with id, provider, description, api_name fields
- [ ] Create `all_models()` function returning all possible models
- [ ] Implement factory pattern for model creation
- [ ] Validate models work before registering (even in gateway mode)
- [ ] Fix model API name mappings to match actual provider APIs
- [ ] Support provider enumeration (Anthropic, OpenAI, Fireworks, etc.)
- [ ] Add model metadata to `/api/models` response
- [ ] Maintain backward compatibility with existing conversations
- [ ] Add tests for multi-provider scenarios

## Implementation Notes

### Proposed Model Structure

```rust
#[derive(Debug, Clone)]
pub struct ModelDef {
    pub id: &'static str,           // "claude-4-opus"
    pub provider: Provider,         // Provider::Anthropic
    pub api_name: &'static str,     // "claude-opus-4.5-20251101"
    pub description: &'static str,  // "Claude Opus 4.5 (most capable)"
    pub context_window: usize,      // 200_000
}

pub enum Provider {
    Anthropic,
    OpenAI,
    Fireworks,
    Gemini,
}

// All available model definitions
pub fn all_models() -> &'static [ModelDef] {
    &[
        ModelDef {
            id: "claude-4-opus",
            provider: Provider::Anthropic,
            api_name: "claude-opus-4.5-20251101",
            description: "Claude Opus 4.5 (most capable)",
            context_window: 200_000,
        },
        // ... more models
    ]
}
```

### Registry Changes

```rust
impl ModelRegistry {
    pub fn new(config: &LlmConfig) -> Self {
        let mut services = HashMap::new();
        
        // Try to create each model
        for model_def in all_models() {
            if let Some(service) = Self::try_create_model(model_def, config) {
                services.insert(model_def.id.to_string(), service);
            }
        }
        
        // ...
    }
    
    fn try_create_model(model: &ModelDef, config: &LlmConfig) -> Option<Arc<dyn LlmService>> {
        match model.provider {
            Provider::Anthropic => {
                // Check if we have key (even for gateway)
                let key = config.anthropic_api_key.as_ref()?;
                Some(Arc::new(AnthropicService::new(
                    key.clone(),
                    model,
                    config.gateway.as_deref()
                )))
            }
            // ... other providers
        }
    }
}
```

### API Response Enhancement

```rust
#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub description: String,
    pub context_window: usize,
}

pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,  // Changed from Vec<String>
    pub default: String,
}
```

## Research Notes

From Shelley's implementation:
- Models defined in `models/models.go` with `All()` function
- Each model has a factory that validates requirements
- Gateway URL construction: `gateway + "/_/gateway/{provider}/..."` 
- Even with gateway, API keys are validated
- Rich error messages when models unavailable

## Testing Scenarios

1. Direct mode with only Anthropic key → only Anthropic models
2. Direct mode with multiple keys → models from each provider
3. Gateway mode with no keys → no models (not all models)
4. Gateway mode with keys → only validated models
5. Invalid API key → model not registered
6. Model selection for existing conversations → uses stored model
