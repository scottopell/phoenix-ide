---
created: 2026-02-02
priority: p1
status: done
tags: [llm, architecture]
---

# Refactor Model Registry for Centralized Model Definition

## Summary

Refactor Phoenix's hard-coded model registry to use a centralized, provider-agnostic system similar to Shelley's approach. This will enable proper multi-provider support, maintainable model definitions, and better gateway integration.

## Context

Current issues with Phoenix's model registry:
1. Models are hard-coded in `register_anthropic_models()` with no central registry
2. Gateway mode incorrectly assumes all models work without API keys
3. Adding new providers requires modifying multiple places in registry code
4. No model metadata (descriptions, provider info)
5. Confusing model IDs (using `claude-4-opus` for Claude 4.5 Opus)

## Acceptance Criteria

- [x] Create `Model` struct with id, provider, description, api_name fields
- [x] Create `all_models()` function returning all possible models (centralized definition)
- [x] Implement factory pattern for model creation
- [x] Validate model prerequisites before registering (check API key exists)
- [x] Fix model API name mappings to match actual provider APIs
- [x] Support provider enumeration (Anthropic, OpenAI, Fireworks, etc.)
- [x] Add model metadata to `/api/models` response
- [x] Provide deployment instructions for database migration/purge
- [x] Add tests for multi-provider scenarios

## Implementation Notes

### Note on "Dynamic" Discovery

This task uses "centralized" rather than truly dynamic discovery because:
- LLM providers don't expose model listing endpoints
- The exe.dev gateway doesn't provide a discovery API
- Models must be defined in code with their API names and metadata
- The improvement is having all models defined in one place rather than scattered

### What "No way to add providers" means

Currently, adding a new provider (e.g., OpenAI) requires:
1. Creating new provider-specific types (`OpenAIModel`, `OpenAIService`)
2. Adding a new `register_openai_models()` function
3. Modifying `ModelRegistry::new()` to call it
4. Updating multiple places for gateway URL construction

With centralized definitions, adding a provider only requires:
1. Adding entries to `all_models()` array
2. Adding a match arm in `try_create_model()`

The code changes are still required, but they're localized and systematic.

### What "Validate model prerequisites" means

Looking at Shelley's code, validation simply checks if the required API key exists:
```go
if config.AnthropicAPIKey == "" {
    return nil, fmt.Errorf("claude-opus-4.5 requires ANTHROPIC_API_KEY")
}
```

This is NOT about making test API calls. The validation happens during factory function execution.
If the factory returns an error, the model is not registered as available.

### Proposed Model Structure

```rust
#[derive(Debug, Clone)]
pub struct ModelDef {
    pub id: &'static str,           // "claude-4.5-opus"
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
            id: "claude-4.5-opus",
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

### Shelley Model Reference

**Source file**: `/home/exedev/shelley/models/models.go`

Current models in Shelley (latest as of git pull):
- `claude-opus-4.5` - Claude Opus 4.5 (default)
- `claude-sonnet-4.5` - Claude Sonnet 4.5  
- `claude-haiku-4.5` - Claude Haiku 4.5
- `glm-4.7-fireworks` - GLM-4.7 on Fireworks
- `gpt-5.2-codex` - GPT-5.2 Codex
- `qwen3-coder-fireworks` - Qwen3 Coder 480B on Fireworks
- `glm-4p6-fireworks` - GLM-4P6 on Fireworks  
- `gemini-3-pro` - Gemini 3 Pro
- `gemini-3-flash` - Gemini 3 Flash
- `predictable` - Deterministic test model (no API key)

Each model definition includes:
- Provider (Anthropic, OpenAI, Fireworks, Gemini)
- Required environment variables
- Description
- Factory function for creation

## Testing Scenarios

1. Direct mode with only Anthropic key → only Anthropic models
2. Direct mode with multiple keys → models from each provider
3. Gateway mode with no keys → no models (not all models)
4. Gateway mode with keys → only validated models
5. Invalid API key → model not registered
6. Model selection for existing conversations → uses stored model

## Deployment Instructions

This change modifies how models are identified and stored. To deploy safely:

1. **Option A: Clean deployment (recommended)**
   ```bash
   # Stop the service
   systemctl stop phoenix-ide
   
   # Remove existing database
   rm ~/.phoenix-ide/phoenix.db
   
   # Deploy new version
   # ... deployment steps ...
   
   # Start service - will create fresh database
   systemctl start phoenix-ide
   ```

2. **Option B: Migrate existing data**
   - Model IDs will change (e.g., `claude-4-opus` → `claude-4.5-opus`)
   - Run migration script: `./scripts/migrate_model_ids.sql`
   - Or just purge if data isn't critical
