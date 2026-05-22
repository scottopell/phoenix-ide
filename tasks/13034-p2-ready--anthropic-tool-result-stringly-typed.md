`AnthropicContentBlock::ToolResult.content` is `serde_json::Value` with
the contract ("String for text-only results; array of content blocks
when images are present") encoded only in a doc comment. The
construction site hand-builds the JSON via `serde_json::json!` literals.
The sibling OpenAI translator demonstrates the correct-by-construction
pattern next door.

## Verified locations

- `crates/phoenix-ide/src/llm/anthropic.rs:991-997` -- type definition:

  ```rust
  ToolResult {
      tool_use_id: String,
      /// String for text-only results; array of content blocks when images are present.
      content: serde_json::Value,
      #[serde(default)]
      is_error: bool,
  },
  ```

- `crates/phoenix-ide/src/llm/anthropic.rs:628-656` -- construction:
  hand-built `serde_json::json!({"type": "text", "text": content})` and
  `{"type": "image", "source": {...}}` literals.

## Why egregious -- sibling provider does it right

`crates/phoenix-ide/src/llm/openai.rs:1044-1047` defines:

```rust
pub(crate) enum ResponsesApiFunctionOutput {
    Text(String),
    Parts(Vec<ResponsesApiContentPart>),
}
```

Used at `openai.rs:660-690` with typed `ResponsesApiContentPart::InputText`
/ `InputImage` variants. Same logical job (text + N images on a
tool result), structurally typed for one provider, ad-hoc JSON for the
other.

AGENTS.md: "If a type permits a value that is semantically wrong, the
type is wrong -- fix the type, not the discipline. Runtime checks,
comments, and conventions that rely on human vigilance are not
substitutes." The doc comment at line 993 is exactly the kind of
distributed-specification comment AGENTS.md warns against -- it will
eventually lie, and the lie will make the next reader trust a shape
the type does not guarantee.

## Concrete risk

The type permits arbitrary `serde_json::Value` for `content`. A future
caller that emits `serde_json::json!({"type": "text", "text": ...})`
as a *single object* (not wrapped in an array) silently sends malformed
content to Anthropic. The compiler cannot stop it; the doc comment is
the only guard.

## Fix direction

Mirror the OpenAI pattern: replace `content: serde_json::Value` with a
two-variant enum (e.g. `AnthropicToolResultContent::Text(String) |
Parts(Vec<AnthropicToolResultPart>)`). Build the typed variants at the
construction site (`anthropic.rs:634-650`) and let serde produce the
existing wire JSON via `#[serde(untagged)]` or similar. Lock the
byte-for-byte wire parity behind a test in `llm/anthropic.rs` (the
`parity_*` test in `api/sse.rs` is a template).

## Related
- 13013 (read-image-parallel-representation, done -- same image bytes
  on the Phoenix side, different layer)
- 13017 (openai-cache-tokens-silently-dropped, done -- prior OpenAI
  side correctness work)
- 13020 (bash-signal-stringly-typed, done -- same family of
  stringly-typed-to-enum migrations)
