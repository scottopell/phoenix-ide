# Implement Centralized Model Registry

## Context & Motivation

You're refactoring Phoenix IDE's model registry from a scattered, hard-coded approach to a centralized, maintainable system inspired by Shelley's design. This is architectural work that will make the codebase more maintainable and extensible.

**Why this matters:**
- Currently, adding a new LLM provider requires touching multiple files and functions
- The gateway mode makes incorrect assumptions (registering all models without validation)
- Model definitions are scattered rather than centralized
- No metadata about models is available to users

## Design Principles to Follow

1. **Centralization over Distribution**: All model definitions should live in ONE place (`all_models()` function), not scattered across provider-specific registration functions.

2. **Fail-Fast Validation**: Even in gateway mode, validate prerequisites (API keys) before registering models. Don't assume the gateway handles everything.

3. **Data over Logic**: Model definitions should be data structures, not buried in registration logic. This makes them easier to maintain and reason about.

4. **Explicit over Implicit**: Make model metadata explicit (provider, description, context window) rather than deriving it from IDs.

5. **Composition over Modification**: Adding new providers should compose with existing code (add to array, add match arm) rather than modifying existing functions.

## Key Insights from Research

From studying Shelley's implementation:
- The "dynamic" in "dynamic discovery" is misleading - it's really about centralized definition
- Validation means "check prerequisites", not "make test API calls"
- Even with a gateway, API keys are required (the gateway is a proxy, not a key manager)
- Model IDs should be clear and consistent (e.g., `claude-4.5-opus` not `claude-4-opus`)

## What Success Looks Like

1. A developer can add a new model by adding ONE entry to the `all_models()` array
2. A developer can add a new provider by adding entries to the array and ONE match arm
3. The `/api/models` endpoint returns rich metadata, not just IDs
4. Gateway mode only registers models that have valid prerequisites
5. The code is more readable and maintainable

## Implementation Approach

Start by studying `/home/exedev/shelley/models/models.go` to understand the pattern. Focus on:
- How models are defined as data
- How the factory pattern validates prerequisites
- How providers are enumerated

Don't just port the code - understand the design philosophy and adapt it to Rust idioms.

## Task Reference

See `tasks/100-refactor-model-registry.md` for detailed requirements and acceptance criteria.