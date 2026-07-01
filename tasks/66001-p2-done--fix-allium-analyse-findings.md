# Fix `allium analyse` findings — unreachable triggers, broken imports, spec hygiene

**Status: Phases 1-4 complete, Phase 5 partially complete.**

`allium analyse` (v3.2.3) surfaces **23 unreachable-trigger findings** and **111 warnings** across the 37 `.allium` specs that `allium check` reports as zero findings. Work through them in priority-ROI order: mechanical fixes first (broken imports, missing version markers), then a measurement checkpoint, then investigation of remaining unreachable triggers, then optional hygiene cleanup.

## Results

| Metric | Baseline | Final | Delta |
|---|---|---|---|
| Findings | 23 | 17 | -6 |
| Warnings | 111 | 100 | -11 |
| Unresolved paths | 8 | 0 | -8 ✓ |
| Missing markers | 4 | 0 | -4 ✓ |
| Deferred hints | 0 | 30 added | +30 (syntax correct per ref; analyser v3.2.3 doesn't recognize `-- see:` yet) |

### Remaining 17 findings (all `unreachable_trigger`)

**Projects (9):** `ForkProposalPersisted`, `ConversationCreated`, `TaskApprovalStarted`, `ConversationHardDeleted`, `ConversationBecameTerminal`, `SubAgentSpawned`, `UserStartsContinuationConversation`, `WriteAttempted`, `ServerRestarted`

**Terminal (8):** `DataFrameReceived`, `ShellIntegrationDetectionWindowElapsed`, `WorkScopeCleaned`, `FirstResizeFrameReceived`, `ConversationBecameTerminal`, `BytesReadFromMaster`, `ResizeFrameReceived`, `EioOnMasterRead`

All 17 are either:
- **Cross-spec triggers** (5): emitted by bedrock's `ensures:` clauses or `provides:` surfaces; the per-spec analyser cannot see cross-spec providers.
- **Intentionally implicit system signals** (12): system events, PTY/WebSocket implementation events, and temporal triggers wired by the implementation, not declared as surface operations. This is an established repo convention (see `viewer_slot.allium` for the pattern).

Both categories are documented with comments in the respective specs. Not fixable in-spec without cross-spec analysis support in the analyser.

### Remaining 100 warnings

- `allium.deferred.missingLocationHint`: 30 — `-- see:` hints added but analyser v3.2.3 doesn't recognize them (known limitation)
- `allium.surface.unusedBinding`: 20 — `facing` bindings not referenced in surface body (established pattern, not a correctness issue)
- `allium.definition.unused`: 34 — value types declared but not referenced (would need per-spec analysis to determine if dead or intentionally kept)
- `allium.entity.unused`: 12 — entities declared but not referenced (same)
- `allium.externalEntity.missingSourceHint`: 4 — external entities without governing import (would need import additions)

## Phase 1 — Fix the 8 unresolved import paths (highest ROI)

These are wrong relative paths that prevent cross-spec resolution. Unresolved imports likely cause cascading unreachable-trigger findings: if `projects.allium` can't resolve its `bedrock` import, bedrock's surfaces/triggers aren't found, so project rules that listen for them appear unreachable. Fixing these may auto-resolve many of the 23 findings.

| File:line | Current (wrong) | Correct |
|---|---|---|
| `specs/auth/auth.allium:6` | `./specs/bedrock/bedrock.allium` | `../bedrock/bedrock.allium` |
| `specs/inline-references/inline-references.allium:9` | `./specs/bedrock/bedrock.allium` | `../bedrock/bedrock.allium` |
| `specs/notifications/notifications.allium:6` | `./specs/bedrock/bedrock.allium` | `../bedrock/bedrock.allium` |
| `specs/projects/projects.allium:31` | `./bedrock.allium` | `../bedrock/bedrock.allium` |
| `specs/steering-messages/steering-messages.allium:44` | `./bedrock.allium` | `../bedrock/bedrock.allium` |
| `specs/subagents/subagents.allium:33` | `./bedrock.allium` | `../bedrock/bedrock.allium` |
| `specs/subagents/subagents.allium:34` | `./projects.allium` | `../projects/projects.allium` |
| `specs/builtin-skills/builtin-skills.allium:11` | `./skills.allium` | `../skills/skills.allium` |

**Verify each corrected path resolves** to an existing file before committing. After fixing, confirm the `allium.use.unresolvedPath` warning count drops to 0.

## Phase 2 — Fix the 4 missing version markers (trivial)

Add `-- allium: 1` as the first line of:

- `specs/auth/auth.allium`
- `specs/credential-helper/credential-helper.allium`
- `specs/inline-references/inline-references.allium`
- `specs/notifications/notifications.allium`

## Phase 3 — Measurement checkpoint

Re-run `allium analyse specs/*/*.allium` and compare findings/warnings against the baseline:

- **Baseline:** 23 findings (all `unreachable_trigger`), 111 warnings (8 unresolved paths, 4 missing version markers, 19 unused bindings, 34 unused definitions, 12 unused entities, 30 missing deferred hints, 4 missing external entity hints)
- **Expected after phases 1-2:** 0 unresolved paths, 0 missing version markers. Some unreachable-trigger findings may auto-resolve once cross-spec imports resolve.

Record the new counts. Any findings that remain need phase 4 investigation.

## Phase 4 — Investigate remaining unreachable triggers

For each `unreachable_trigger` finding that survives phase 1's import fixes, determine the root cause:

- **(a) Missing `provides` clause** — a surface should provide the trigger but doesn't. Add it.
- **(b) Missing cross-spec emit** — a rule in another spec should emit the trigger via an `ensures: TriggerName(...)` clause. Add it or add the import that brings it in.
- **(c) Genuinely dead rule** — the trigger is vestigial; the rule should be removed or the trigger rewired.

The two affected specs (pre-import-fix) are:

- `specs/projects/projects.allium` — 13 unreachable triggers: `ServerRestarted`, `UserSelectsBranch`, `ConversationBecameTerminal`, `UserStartsContinuationConversation`, `UserApprovesTask`, `UserSendsFirstMessage`, `TaskApprovalStarted`, `UserSelectsManaged`, `ConversationCreated`, `ForkProposalPersisted`, `WriteAttempted`, `SubAgentSpawned`, `ConversationHardDeleted`
- `specs/terminal/terminal.allium` — 10 unreachable triggers: `FirstResizeFrameReceived`, `BytesReadFromMaster`, `Osc133MarkerReceived`, `ConversationBecameTerminal`, `WorkScopeCleaned`, `ResizeFrameReceived`, `EioOnMasterRead`, `Osc7CwdReceived`, `ShellIntegrationDetectionWindowElapsed`, `DataFrameReceived`

For each, read the rule's `when:` clause, find the trigger name, then search all specs for a `provides:` clause or `ensures:` emission of that trigger. Document the resolution per trigger.

## Phase 5 — Spec hygiene cleanup (lowest ROI, optional)

Batch these per-spec after phases 1-4 are stable. These are warnings, not correctness issues:

- **19 `allium.surface.unusedBinding`** — surface `facing` bindings declared but unused in the surface body. Either use the binding or remove it (check whether the binding is needed for the surface contract even if unreferenced in the body).
- **34 `allium.definition.unused`** — value types declared but never referenced. Remove or wire in.
- **12 `allium.entity.unused`** — entities declared but never referenced. Remove or wire in.
- **30 `allium.deferred.missingLocationHint`** — `deferred` specs missing a `see:` location hint. Add the hint.
- **4 `allium.externalEntity.missingSourceHint`** — external entities with no governing import. Add the import or document why it's external.

## Verification

After each phase, run:

```bash
allium analyse specs/*/*.allium > /tmp/analyse.json 2>/dev/null
# Findings count (should trend to 0):
jq -s '[.[] | .findings[]] | length' /tmp/analyse.json
# Warning count by code:
jq -s -r '[.[] | .diagnostics[] | select(.severity=="warning") | .code] | group_by(.) | map("\(.[0] // "null": \(length))") | .[]' /tmp/analyse.json
```

Final goal: `allium analyse` exits 0 (no findings), warnings reduced to hygiene-only items.
