---
created: 2026-02-21
priority: p0
status: ready
---

# Refactor to data-driven ModelSpec (eliminate model enums)

## Problem

Current design has semantic confusion:

```rust
pub enum OpenAIModel {
    GPT4o,
    GLM4P7Fireworks,      // ❌ WTF - Fireworks ≠ OpenAI
    DeepseekV3Fireworks,  // ❌ Fireworks model in OpenAI namespace
}

impl OpenAIModel {
    pub fn is_fireworks(&self) -> bool {
        match self {
            OpenAIModel::GLM4P7Fireworks => true,  // ❌ Garbage pattern matching
            ...
        }
    }
}
```

**Root cause:** Conflating provider identity (who owns the model) with wire protocol (which HTTP API it uses).

Fireworks uses OpenAI's HTTP API format, so the code reuses `OpenAIService`. But this creates terrible naming: "Fireworks models" are in the "OpenAI namespace."

## Solution: Correct by Construction

Replace model enums with pure data-driven `ModelSpec`:

```rust
pub struct ModelSpec {
    pub id: String,              // User-facing: "glm-4p7-fireworks"
    pub api_name: String,        // Wire format: "accounts/fireworks/models/glm-4p7"
    pub provider: Provider,      // Fireworks (not OpenAI!)
    pub api_format: ApiFormat,   // OpenAIChat (the wire protocol)
    pub context_window: usize,
    pub recommended: bool,
}

pub enum ApiFormat {
    Anthropic,       // Anthropic Messages API
    OpenAIChat,      // OpenAI Chat Completions (used by OpenAI + Fireworks)
}

pub struct LlmServiceImpl {
    spec: ModelSpec,
    api_key: String,
    gateway: Option<String>,
}
```

**Benefits:**
- Provider and API format are separate concerns ✅
- No `OpenAIModel::FireworksXYZ` nonsense ✅
- Discovered models just create new `ModelSpec` instances ✅
- No enums to exhaust in pattern matching ✅
- No `is_fireworks()` methods ✅

## Implementation Steps

### 1. Create new types in `src/llm/models.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    Anthropic,
    OpenAIChat,
}

#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub id: String,
    pub api_name: String,
    pub provider: Provider,
    pub api_format: ApiFormat,
    pub description: String,
    pub context_window: usize,
    pub recommended: bool,
}

// Convert all hardcoded ModelDef instances to ModelSpec data
pub fn all_models() -> Vec<ModelSpec> {
    vec![
        ModelSpec {
            id: "glm-4p7-fireworks".into(),
            api_name: "accounts/fireworks/models/glm-4p7".into(),
            provider: Provider::Fireworks,  // ✅ Correct provider
            api_format: ApiFormat::OpenAIChat,  // ✅ Explicit wire format
            description: "GLM-4P7 on Fireworks".into(),
            context_window: 128_000,
            recommended: false,
        },
        // ... rest of models
    ]
}
```

### 2. Create unified service in `src/llm/service.rs`

```rust
pub struct LlmServiceImpl {
    pub spec: ModelSpec,
    pub api_key: String,
    pub gateway: Option<String>,
}

#[async_trait]
impl LlmService for LlmServiceImpl {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.spec.api_format {
            ApiFormat::Anthropic => self.complete_anthropic(request).await,
            ApiFormat::OpenAIChat => self.complete_openai_chat(request).await,
        }
    }
}
```

### 3. Refactor `src/llm/anthropic.rs` to stateless functions

```rust
/// Complete using Anthropic Messages API
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let base_url = match gateway {
        Some(gw) => format!("{}/anthropic/v1/messages", gw.trim_end_matches('/')),
        None => "https://api.anthropic.com/v1/messages".to_string(),
    };
    
    let anthropic_request = translate_request(&spec.api_name, request);
    // ... HTTP request logic
}

// Delete AnthropicService struct
// Delete AnthropicModel enum
```

### 4. Refactor `src/llm/openai.rs` to stateless functions

```rust
/// Complete using OpenAI Chat Completions API
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let base_url = construct_base_url(spec.provider, gateway);
    let openai_request = translate_request(&spec.api_name, request);
    // ... HTTP request logic
}

fn construct_base_url(provider: Provider, gateway: Option<&str>) -> String {
    match gateway {
        Some(gw) => match provider {
            Provider::OpenAI => format!("{}/openai/v1", gw),
            Provider::Fireworks => format!("{}/fireworks/inference/v1", gw),
            Provider::Anthropic => panic!("OpenAI API called for Anthropic"),
        },
        None => match provider {
            Provider::OpenAI => "https://api.openai.com/v1".into(),
            Provider::Fireworks => "https://api.fireworks.ai/inference/v1".into(),
            Provider::Anthropic => panic!("OpenAI API called for Anthropic"),
        },
    }
}

// Delete OpenAIService struct
// Delete OpenAIModel enum
```

### 5. Update `src/llm/registry.rs`

```rust
fn try_create_model(spec: &ModelSpec, config: &LlmConfig) -> Option<Arc<dyn LlmService>> {
    let api_key = if config.gateway.is_some() {
        "implicit".to_string()
    } else {
        match spec.provider {
            Provider::Anthropic => config.anthropic_api_key.as_ref()?,
            Provider::OpenAI => config.openai_api_key.as_ref()?,
            Provider::Fireworks => config.fireworks_api_key.as_ref()?,
        }.clone()
    };

    let service = Arc::new(LlmServiceImpl::new(
        spec.clone(),
        api_key,
        config.gateway.clone(),
    ));

    Some(Arc::new(LoggingService::new(service)))
}
```

### 6. Update `src/llm/discovery.rs`

Dynamic discovery becomes trivial - just create `ModelSpec` instances:

```rust
pub async fn discover_models(gateway_url: &str) -> Vec<ModelSpec> {
    let mut specs = Vec::new();
    
    // Query Anthropic endpoint
    if let Ok(anthropic_models) = discover_anthropic(gateway_url).await {
        for model in anthropic_models {
            specs.push(ModelSpec {
                id: model.id.clone(),
                api_name: model.id,
                provider: Provider::Anthropic,
                api_format: ApiFormat::Anthropic,
                description: model.display_name.unwrap_or_default(),
                context_window: 200_000,  // Default, can refine
                recommended: false,
            });
        }
    }
    
    // Similar for OpenAI and Fireworks...
    
    specs
}
```

### 7. Update exports in `src/llm.rs`

```rust
pub use models::{all_models, ApiFormat, ModelSpec, Provider};
pub use service::LlmServiceImpl;

// Remove:
// pub use anthropic::AnthropicService;
// pub use openai::OpenAIService;
```

### 8. Add error helpers in `src/llm/error.rs`

```rust
impl LlmError {
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::InvalidRequest, message)
    }

    pub fn from_http_status(status: u16, body: String) -> Self {
        match status {
            401 | 403 => Self::auth(format!("Authentication failed: {}", body)),
            429 => Self::rate_limit(format!("Rate limited: {}", body)),
            400..=499 => Self::invalid_request(format!("Bad request ({}): {}", status, body)),
            500..=599 => Self::server_error(format!("Server error ({}): {}", status, body)),
            _ => Self::unknown(format!("HTTP {}: {}", status, body)),
        }
    }
}
```

## Files to Modify

- `src/llm/models.rs` - Replace ModelDef with ModelSpec, delete factory functions
- `src/llm/service.rs` - **NEW** - Unified service implementation
- `src/llm/anthropic.rs` - Convert to stateless functions, delete service/enum
- `src/llm/openai.rs` - Convert to stateless functions, delete service/enum (~900 lines)
- `src/llm/registry.rs` - Update to use ModelSpec
- `src/llm/discovery.rs` - Simplify to create ModelSpec instances
- `src/llm/error.rs` - Add helper methods
- `src/llm.rs` - Update exports

## Testing Strategy

1. **Compile:** `cargo check` should pass with no warnings
2. **Unit tests:** All existing tests should pass
3. **Integration:** `./phoenix-client.py --api-url http://localhost:8031 --model claude-4.5-haiku "test"`
4. **Discovery:** Check logs show "Discovered X models from gateway"
5. **API:** `curl http://localhost:8031/api/models | jq '.models | length'`

## Success Criteria

- ✅ No more `OpenAIModel` or `AnthropicModel` enums
- ✅ No `OpenAIService` or `AnthropicService` structs
- ✅ Single `LlmServiceImpl` that dispatches by `ApiFormat`
- ✅ Provider and API format are separate fields
- ✅ All existing functionality works
- ✅ Dynamic discovery simplified

## Partial Work Completed

Commit `d6245ad` added `recommended: bool` field. The full refactor builds on this.

## Estimated Effort

2-3 hours focused work. The design is clear, implementation is mostly mechanical translation.
