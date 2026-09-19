# Present agent-generated SVGs inline as durable conversation artifacts

## Summary

Implement an agent-facing `present_svg` tool that accepts a server-local SVG file, validates it as a bounded static visual, snapshots it into durable conversation-owned storage, and displays it inline in the web conversation. Deliver the complete tool-to-storage-to-UI path, documentation, security controls, and automated/browser verification—not merely a tool schema or preview prototype.

## Context and agreed direction

Users should be able to ask “visualize this information” and receive an inline chart or illustration, conceptually alongside Mermaid but without Mermaid's diagram grammar constraints. The motivating example is a disk-usage table: a horizontal bar chart makes relative directory sizes immediately legible.

Decisions agreed in conversation:

- Start with **file-based `present_svg`**, not `present_html` or automatic SVG Markdown fences.
- Agents should generate SVG using code or charting libraries rather than transcribe large SVG payloads into tool arguments. This is a central benefit, not an incidental implementation detail.
- Publish a visible artifact card, not a collapsed tool log. Keep tool activity inspectable without showing two copies of the visual.
- Render static, self-contained SVG in image context. Never inject arbitrary SVG into Phoenix's application DOM.
- Snapshot bytes into durable conversation storage. Scratch files are staging inputs, not durable references.
- A revised visualization creates a new card. No editing/versioning protocol in this task.
- Keep renderer boundaries reusable, but do not build a generic artifact/plugin framework speculatively.

Source discussion: conversation `69051831-f3b3-4149-bf14-8524d32531a2`, including reviewed message `ef5e0a8a-e722-4039-afb0-087c40abfab1` and subsequent temp-directory clarifications.

## Agent-facing contract

Implement this small interface (use existing tool-schema conventions):

```text
present_svg(
  path="/absolute/server/path/disk-usage.svg",
  title="Largest storage consumers",
  description="Horizontal bars comparing measured directory sizes in GiB."
)
```

- Require an absolute path, a nonempty bounded title, and a nonempty bounded plain-text description used for accessibility/fallback. Document concrete limits.
- The path is on the Phoenix server, not the browser's machine. Do not shell-expand `$TMPDIR`, `~`, or arbitrary expressions in tool arguments.
- Read one bounded regular file using the calling scope's applicable filesystem permissions. Reject directories, devices, FIFOs, inaccessible inputs, and permission escapes; symlink handling must not bypass scope restrictions. Do not widen Explore capabilities to enable this tool.
- On success, return a compact structured artifact reference and validation outcome, not the SVG bytes/base64 or a claim that the agent visually inspected the image.
- On failure, return a bounded actionable error so the agent can regenerate/correct the file. Distinguish invalid input, policy rejection, size/complexity limits, read failure, and persistence failure.
- Derive conversation ownership from trusted invocation context, never an agent-supplied conversation ID.
- Register the tool through all applicable provider/tool-definition and permission surfaces. Explicitly test advertised modes; do not advertise a workflow whose staging file is deleted before `present_svg` can read it.

### Temp-directory guidance

Use the platform temp mechanism in examples, rather than hard-coding `/tmp/disk-usage.svg`:

```sh
artifact_dir=$(mktemp -d "${TMPDIR:-/tmp}/phoenix-svg.XXXXXX")
# Generate "$artifact_dir/disk-usage.svg" with code/a charting library.
printf '%s\n' "$artifact_dir/disk-usage.svg"
```

Pass the resulting resolved absolute filename to `present_svg` in a subsequent tool call. Clean up staging only after publication succeeds; the presentation tool must not delete the caller's source file.

Investigation findings (reverify against implementation/deployment; not a new promised API):

- In the inspected live Direct session, `TMPDIR` was the normal macOS temp directory; `TEMP`, `TMP`, `PHOENIX_TMP_DIR`, and `PHOENIX_SANDBOX_SCRATCH` were unset.
- `PhoenixRuntimeEnvironment` supports `PHOENIX_TMP_DIR` as a Phoenix-wide scratch-root override, not a confirmed workscope-specific directory.
- Explore `apply_child_env` explicitly assigns `TMPDIR` and `PHOENIX_SANDBOX_SCRATCH`. The latter is created per command launch; inspect its cleanup lifecycle before recommending cross-call staging there.
- Explore's `platform_temp_dir` is not necessarily scope-specific. Do not document either variable as a durable per-workscope artifact store.

## Safe SVG ingestion and serving

Treat file contents and metadata as untrusted, regardless of which agent generated them.

- Use an XML/SVG-aware parser and an explicit supported static subset, not regex-only sanitization. Disable DTD/entity resolution and all parser resource fetching; reject external entities/DOCTYPE.
- Reject scripts, event-handler attributes, `foreignObject`/embedded HTML, navigation links, animation, and external resource references (network, file, or embedded data payloads). Apply URL rules to CSS as well as attributes, including namespace/encoding variants.
- Support safe local fragment references needed by common chart-library output: definitions, glyph/path reuse, clip paths, and gradients as appropriate. Bound recursive references and expansion/rendering complexity.
- Support a documented safe styling subset. Chart-library-generated SVGs must work in realistic fixtures; don't accidentally make “use charting libraries” impossible by rejecting every style declaration or local `use` reference.
- Establish explicit byte, element/nesting, attribute/path/reference complexity, and dimension limits. Reject non-finite/invalid geometry and impose resource bounds on validation/rendering. Document chosen limits and test both sides of boundaries.
- Prefer actionable rejection of unsupported visual features over silently publishing a materially altered chart. If canonicalization removes harmless metadata, distinguish that from removing visual content.
- Render the accepted snapshot through an image element/image context, never raw HTML injection, `object`, or an unsandboxed document. Preview and expanded view share the same safe path.
- Serve artifacts through authenticated, ownership-checked opaque-ID endpoints using existing access conventions. Never expose arbitrary server paths through an artifact URL.
- Set appropriate MIME, nosniff, restrictive CSP/resource policy, and download disposition. Direct navigation to the artifact endpoint must not turn it into an active same-origin SVG document.
- Source view displays escaped text only. Downloads contain only the accepted safe artifact, not rejected original content.
- Do not log file contents or return unbounded parser diagnostics.

## Durable storage and lifecycle

- Inspect and reuse existing attachment/artifact ownership and storage mechanisms where suitable; do not assume that an existing attachment directory is durable merely because of its name.
- Persist the accepted bytes and metadata/reference as conversation-owned data. Store an immutable snapshot, not a dependency on the input file or its mtime.
- Publication succeeds only once the snapshot and its durable conversation association are committed. Define and test failure/cancellation cleanup across file storage and database persistence.
- Integrate with tool-result persistence and replay so reload/reconnect/runtime recovery displays the same artifact. Do not rely on a frontend-only tool-call cache or ephemeral URL.
- Follow existing retry/replay identity semantics to avoid duplicate publications from replaying one completed invocation. Separate explicit invocations may publish separate cards.
- Preserve artifacts after staging deletion, process restart, worktree removal, and scope retirement while the owning conversation is retained. Integrate with existing conversation deletion/retention cleanup, without adding a general garbage-collection subsystem unnecessarily.
- A remote browser retrieves bytes from Phoenix; it never needs access to the source filesystem. Authorization must cover preview, source, and download equally.
- Keep model-facing history compact. Use existing tool-result/attachment typing rather than dumping large markup into subsequent model contexts.

## Inline web UI

- Show title, responsive inline preview, and a useful accessible description/fallback at the publishing tool result's chronological location.
- Provide expand/zoom, download SVG, and view-source controls. Expanded view is keyboard accessible and can be dismissed with Escape; preserve normal chat input/scroll behavior.
- Preserve aspect ratio, avoid distorted charts, and bound initial height so very tall/wide artifacts do not dominate the transcript. Reserve layout space from validated dimensions when possible.
- Ensure readability in light/dark themes without rewriting chart colors unpredictably; use an appropriate preview surface/background for transparent SVGs.
- Display useful loading, unavailable/corrupt artifact, and publication-failure states. Never silently leave a blank card.
- Work on narrow/mobile browser layouts and remote clients. Check the existing native-client decoding path for compatibility; native bespoke SVG rendering is not required here, but unsupported clients must retain a readable title/description/reference rather than fail message decoding.
- Existing Mermaid rendering, ordinary SVG code examples, generic tool inspection, and older persisted messages must keep working.

## Implementation plan

1. **Map current contracts.** Read applicable tool-registration, permission, message/presentation, attachments, authorization, persistence/replay, and retention specs/code. Inspect Mermaid/image UI only for useful existing patterns; this is not a Markdown renderer change.
2. **Record requirements and decisions.** Follow spEARS v2: add/update timeless REQ IDs and executive coverage, and an ADR for file-based static SVG presentation versus fences/HTML. Add Allium only if the publication lifecycle merits it. Do not create a legacy design.md or speculative HTML architecture.
3. **Build bounded ingestion + durable publication.** Implement parser/policy, storage association, authenticated serving, failure cleanup, and unit/integration tests.
4. **Wire the tool end to end.** Add schema, trusted-context ownership, mode/permission policy, provider exposure, compact result, replay-safe serialization, and agent-facing guidance/examples.
5. **Build the inline card.** Reuse existing message infrastructure; add responsive preview, accessible expansion, source/download, and fallback states.
6. **Exercise realistic and hostile content.** Verify library-generated charts as well as hand-authored SVGs. Run end-to-end publication/reload/restart/source-deletion flows and browser security tests.
7. **Update status and handoff evidence.** Run relevant checks through `./dev.py`, record commands/results and browser evidence, and update executive verification coverage. Production deployment is not required by this task.

Useful starting points already inspected:

- `crates/phoenix-core/src/runtime_env.rs` — runtime/temp/storage paths.
- `crates/phoenix-tools/src/bash/sandbox.rs` — `apply_child_env`, `scratch_dir`, `platform_temp_dir`.
- `crates/phoenix-tools/src/bash/operations.rs` — launch modes and scratch cleanup lifecycle.
- `crates/phoenix-tools/`, `crates/phoenix-db/`, `crates/phoenix-ide/src/api/`, and `ui/src/` — discover actual extension points before choosing concrete storage/message types.

## Acceptance criteria and verification

- [ ] A real agent can generate a chart using code/a charting library, call `present_svg` with its resolved filename, and produce a visible inline card without pasting SVG into tool arguments.
- [ ] A disk-usage horizontal bar chart shows exact GiB labels, keeps free space separate, and does not add nested breakdowns to their parent totals. This is an example/agent-guidance check, not a disk-specific tool feature.
- [ ] At least one representative chart-library SVG and a hand-authored SVG render correctly, including supported internal references, labels, and styling.
- [ ] Invalid/missing inputs, forbidden content, out-of-policy paths, and limit violations return actionable errors without publishing a successful artifact.
- [ ] Security tests cover script/event handlers, embedded HTML, CSS/attribute external URLs, namespace variants, entity expansion, recursive references, extreme dimensions/complexity, and file-read boundary cases.
- [ ] Browser tests show no injected script execution, application DOM access, navigation, or external resource requests from malicious SVG fixtures; test preview, expansion, direct artifact URL, and source view.
- [ ] Preview/source/download are authorization-equivalent and cannot retrieve an unrelated conversation's artifact or arbitrary filesystem path beyond the caller's existing access authority.
- [ ] Replacing or deleting the staging file after success does not change/break the artifact; reload, reconnect, runtime restart, and retained-conversation scope/worktree retirement preserve it.
- [ ] Failure/cancellation between snapshot creation and association persistence does not report success or leave permanently unowned artifacts; replay does not duplicate a completed publication.
- [ ] Card controls, description fallback, keyboard interaction, theme behavior, long titles, very wide/tall images, and narrow viewport layouts have automated coverage and actual browser verification.
- [ ] Persisted older messages and clients without bespoke SVG support remain readable; Mermaid and SVG code-fence behavior are unchanged.
- [ ] Tool descriptions document static-only policy, generation via code/libraries, temp staging lifetime, absolute paths, limits, and the distinction between validation and visual inspection.
- [ ] Relevant Rust/UI/integration checks pass through the repository's supported workflow; document exact verification evidence and any environmental limitations.

## Explicit non-goals

- `present_html`, JavaScript execution, interactive widgets, external fonts/assets/network access.
- Automatic rendering of `svg` Markdown fences or a new presentation-fence syntax.
- Artifact editing/version trees, agent-to-widget callbacks, dashboards, a plugin ecosystem.
- A built-in chart grammar or Phoenix-owned disk-usage analyzer.
- Automatically claiming visual correctness from parser success, or requiring every agent to visually inspect every chart.
- A new global temp-directory/workscope lifecycle architecture or dedicated native SVG renderer.
