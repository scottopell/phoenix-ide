# Make OTLP tracing bounded, structured, and payload-safe

## Finding

Phoenix deliberately applies `RUST_LOG`/`EnvFilter` only to stdout and file formatting layers. The OpenTelemetry layer in `crates/phoenix-ide/src/logging.rs` currently accepts every tracing span and event except the `http.stream` span. Consequently, dependency TRACE/DEBUG events emitted while `LoggingService` instruments an entire provider future are recorded as events on `llm.request`, regardless of whether local logs suppress them. OpenAI Responses also emits an application DEBUG event for every SSE event. The SDK's default 128-event and 128-attribute limits bound counts somewhat, but do not bound string value sizes; raw SSE diagnostics, response/output payloads, malformed tool arguments, and provider error bodies can still produce very large or sensitive exported values.

Separate local and OTLP filters already exist structurally, but the OTLP filter is too broad. The safe minimum is to make OTLP a spans-only, Phoenix-target allowlist and encode the useful LLM/tool summary on span attributes rather than exporting arbitrary log events. Local logs can retain independently filtered operational events, but raw payload diagnostics must be removed or replaced with bounded, non-content metadata because local logs also must not leak secrets.

## Implementation plan

1. **Define an explicit OTLP contract in `logging.rs`.**
   - Replace the single `http.stream` exclusion with a testable per-layer filter that exports only intentional Phoenix-owned spans at the required level and exports no `tracing` events.
   - Exclude dependency spans/events (`tokio_tungstenite`, `tungstenite`, `hyper_util`, `reqwest`, `sqlx`, and similar targets) regardless of `RUST_LOG`.
   - Continue excluding long-lived `http.stream` spans while preserving them for local stdout/file logs.
   - Keep stdout/file `EnvFilter` behavior independent so operators can use dependency diagnostics locally without shipping them to OTLP.

2. **Add defense-in-depth SDK span limits.**
   - Configure explicit conservative limits for span attributes, links, events, and event attributes for both OTLP and Datadog providers rather than relying on SDK defaults or collector limits.
   - Since the OTLP contract is spans-only, set/verify the event budget accordingly. Limits are a backstop, not the privacy boundary.

3. **Put bounded high-level observability on the intentional spans.**
   - Preserve automatic duration and safe attributes for model, provider, transport, streaming mode, token/cache counts, retry attempt, request id, conversation/root-conversation id, final state, tool name/outcome, and failure category/reason.
   - Record tool duration on `tool.execute` rather than only in its completion event.
   - Thread missing request metadata through a typed request/attempt telemetry context or an equivalent single authoritative structure; do not duplicate prompts, tool schemas, messages, or payloads in telemetry fields.
   - Represent dynamic Codex transport selection/fallback as a small enum-like value (`websocket`, `http_sse`, or equivalent), never as request/frame detail.
   - Record classified, bounded failure categories rather than raw provider bodies in exported span fields.

4. **Remove unsafe/high-volume provider diagnostics.**
   - Remove the per-SSE-event `responses_api SSE event` DEBUG emission and any per-delta/frame logging.
   - Replace full `data`, `item`, malformed tool `arguments`, response body, and `SseParser::diagnostic_dump()` logging with bounded structural metadata such as event type, byte count, parser state/count, provider request id when safe, and classified error code.
   - Audit header-related logging paths and ensure authorization/API-key/custom-header values are never formatted into tracing fields or error strings.
   - Keep useful low-frequency local events for transport fallback/reconnect, request completion, token totals, and classified failures.

5. **Add regression guards around the exported representation.**
   - Use an in-memory OpenTelemetry exporter/subscriber in focused tests to create an LLM span under noisy dependency/application events and assert that exported span data contains no events, forbidden targets, payload sentinels, authorization values, prompt/tool-schema content, or per-delta data.
   - Assert intentional LLM/tool spans retain the required safe attributes and remain within explicit attribute/event budgets.
   - Add provider-level tests proving diagnostic helpers expose only bounded structural data and do not include supplied secret/payload sentinels.
   - Test local and OTLP filters independently, including that a dependency DEBUG event may be enabled locally while still absent from OTLP.

6. **Update the LLM observability requirement/current-reality docs.**
   - Strengthen `REQ-LLM-008` to require bounded structured request telemetry and prohibit prompt, tool-schema, credential/header, frame, delta, and raw payload export.
   - Update the executive mapping after verification; run the spec authoring pre-flight because this touches normative requirements.

## Verification

- Run focused `phoenix-llm` provider/logging tests and `phoenix-ide` logging/runtime tests.
- Run `./dev.py check`.
- Exercise a long streaming Codex/OpenAI and Anthropic turn with OTLP pointed at VictoriaTraces; inspect exported `llm.request`, `conversation.turn`, and `tool.execute` spans for required attributes and forbidden content.
- Confirm VictoriaTraces emits neither `>1000 fields` nor `>2MiB` warnings during normal long-turn use.
- Confirm stdout/file logs still show concise request completion, token usage, retry/fallback, classified failure, and tool completion information under the configured `RUST_LOG`.

## Scope discipline

Do not solve this by raising VictoriaTraces ingestion limits or by trusting SDK truncation. Do not add a general-purpose telemetry payload serializer. The privacy boundary is an explicit spans-only OTLP allowlist plus content-free span attributes; hard limits are only defense in depth.
