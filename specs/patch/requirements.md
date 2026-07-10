# Patch Tool

## User Story

As an LLM agent, I need to edit files precisely so that I can modify code, configuration, and documentation without rewriting entire files or making transcription errors.

## Requirements

### REQ-PATCH-001: File Operations

WHEN agent requests file modification
THE SYSTEM SHALL support these operations:
- `replace`: Substitute unique text with new content
- `insert_before`: Insert new content immediately before a unique anchor, leaving the anchor unchanged
- `insert_after`: Insert new content immediately after a unique anchor, leaving the anchor unchanged
- `append_eof`: Append text at end of file
- `prepend_bof`: Insert text at beginning of file
- `overwrite`: Replace entire file contents (creates file if missing)

WHEN replace operation is requested without replaceAll
THE SYSTEM SHALL require oldText to appear exactly once in the file

WHEN insert_before or insert_after operation is requested
THE SYSTEM SHALL require oldText (the anchor) to appear exactly once in the file
AND insert newText adjacent to the anchor without altering the anchor's bytes

WHEN oldText appears more than once after all safe matching strategies are exhausted
THE SYSTEM SHALL reject the operation
AND identify the failing patch's 1-based position in the patches array and its operation
AND return duplicate-match diagnostics that include:
- the total match count;
- a bounded list of matching locations;
- each reported match's 1-based start line number;
- a short snippet around each reported match;
- guidance to widen oldText or split edits into separate patch calls

WHEN file does not exist
THE SYSTEM SHALL allow only append_eof, prepend_bof, and overwrite operations
AND reject anchor-based operations (replace, insert_before, insert_after) because no anchor can match
AND create parent directories as needed

**Rationale:** Agents need precise, predictable file editing. Requiring unique anchor matches prevents ambiguous edits that could corrupt files. Anchor-based operations (replace and the two inserts) all locate their target the same way, so they share the same uniqueness rule and the same existence precondition.

---

### REQ-PATCH-002: Multiple Patches Per Call

WHEN agent provides multiple patches for a single file
THE SYSTEM SHALL apply all patches atomically
AND apply them against the original file content simultaneously

WHEN two edits in a single call target overlapping byte ranges
THE SYSTEM SHALL reject the entire operation
AND leave the file unchanged

WHEN any patch fails to apply
THE SYSTEM SHALL reject the entire operation
AND leave the file unchanged

WHEN an anchor-based patch fails to locate a unique anchor
THE SYSTEM SHALL identify that patch by its 1-based position in the patches array and operation

**Rationale:** Agents often need multiple related edits. Atomic application prevents partial modifications that leave files in broken states. Because every edit resolves against the original content, two edits whose byte ranges overlap have no well-defined combined result — applying both would silently corrupt the region, so the conflict is rejected rather than resolved by edit ordering. A zero-length insertion positioned exactly at the boundary of another edit does not overlap and is permitted.

---

### REQ-PATCH-003: Clipboard Operations

WHEN agent specifies toClipboard on a replace operation
THE SYSTEM SHALL store the matched oldText to the named clipboard
AND complete the replace operation

WHEN agent specifies fromClipboard
THE SYSTEM SHALL use clipboard content as newText
AND ignore any provided newText field

WHEN clipboard is referenced but does not exist
THE SYSTEM SHALL return error indicating missing clipboard

WHEN agent uses same clipboard name for toClipboard and fromClipboard
THE SYSTEM SHALL perform a copy operation (text remains unchanged)

WHEN a patch call fails after a clipboard write but before completion
THE SYSTEM SHALL leave the clipboards unchanged from before the call

**Rationale:** Clipboards enable cut/copy/paste workflows that reduce transcription errors when moving code within or across files. Clipboard writes are committed only when the whole call succeeds: a call that reports failure leaves the file unchanged, so it must equally leave the clipboards unchanged — otherwise a later fromClipboard could read text staged by a call that never landed.

---

### REQ-PATCH-004: Indentation Adjustment

WHEN agent specifies reindent with strip prefix
THE SYSTEM SHALL remove that prefix from each non-empty line of inserted text

WHEN agent specifies reindent with add prefix
THE SYSTEM SHALL add that prefix to each non-empty line of inserted text

WHEN strip prefix is not present on a non-empty line
THE SYSTEM SHALL return error indicating strip precondition failed

**Rationale:** Agents frequently move code between different indentation levels. Explicit reindentation prevents whitespace errors.

---

### REQ-PATCH-005: Fuzzy Matching Recovery

WHEN exact oldText match fails
THE SYSTEM SHALL attempt recovery via whitespace normalization:
- Adjust leading whitespace prefix (dedent matching)
- Trim first/last lines if safe

WHEN fuzzy match succeeds
THE SYSTEM SHALL apply the patch using matched boundaries
AND update any toClipboard with actual matched text

WHEN all matching attempts fail
THE SYSTEM SHALL return an "old text not found" error
AND identify the failing patch's 1-based position in the patches array and its operation
AND instruct the agent to re-read the file and retry that patch with current text

**Rationale:** LLMs occasionally generate patches with minor whitespace differences. Safe recovery improves reliability without compromising precision.

---

### REQ-PATCH-006: Tool Schema

WHEN LLM requests patch tool
THE SYSTEM SHALL provide schema with:
- `path` (required string): File path relative to working directory
- `patches` (required array): List of patch operations

WHEN patch operation is specified
THE SYSTEM SHALL accept:
- `operation` (required enum): replace, insert_before, insert_after, append_eof, prepend_bof, overwrite
- `oldText` (string): Text to locate (required for replace, insert_before, insert_after)
- `newText` (string): Replacement text, or content to insert for the insert operations
- `replaceAll` (boolean): For replace only; substitute every exact occurrence instead of requiring a unique match
- `toClipboard` (string): Named clipboard to store oldText
- `fromClipboard` (string): Named clipboard to use as newText
- `reindent` (object): Strip and add prefixes for indentation

**Rationale:** Structured schema enables precise file editing with optional clipboard and reindentation features.

---

### REQ-PATCH-007: Output and Display

WHEN patches are applied successfully
THE SYSTEM SHALL return confirmation to LLM
AND include the applied unified diff in the LLM-visible response so the agent can confirm where each edit landed without re-reading the file
AND generate the same unified diff for UI display

WHEN the applied diff exceeds the response's bounds on line count, single-line length, or total bytes
THE SYSTEM SHALL truncate it
AND state that lines were omitted or shortened so the agent knows the preview is partial

WHEN the diff content contains text resembling the wrapper's own delimiter
THE SYSTEM SHALL neutralize it so file content cannot close the diff block early or be mistaken for tool markup

WHEN file appears to be autogenerated
THE SYSTEM SHALL include warning in response
AND apply patches anyway

WHEN clipboard contents are modified during fuzzy matching
THE SYSTEM SHALL notify LLM of the modification

**Rationale:** Clear feedback helps agents verify edits. Returning the applied diff to the agent — not only to the UI — closes the verify-by-re-reading loop that otherwise follows every edit. That preview must stay a preview: it is bounded on lines, single-line length, and total bytes so a minified file (a huge diff in very few lines) cannot swamp the response, and any text resembling the wrapper delimiter is neutralized so untrusted file content cannot impersonate tool markup. Diffs enable user review. Autogenerated warnings prevent accidental modifications to generated code.

---

### REQ-PATCH-008: Size Limits

WHEN patch input exceeds 60 KB
THE SYSTEM SHALL reject the operation
AND suggest breaking into smaller patches

**Rationale:** Bounding tool-call input size keeps the round-trip cheap and predictable. The unit is bytes of serialized JSON input, matching `MAX_INPUT_SIZE` in the implementation.

---

### REQ-PATCH-009: Mode-Based Availability

WHEN conversation is in Explore mode (Managed workflow)
THE SYSTEM SHALL provide the patch tool restricted to the project's tasks directory
  (typically `tasks/`, discovered per-project — see task 13008) so the agent can draft a
  task file before calling `propose_task`
AND reject a patch operation targeting any path outside that directory
AND return a descriptive error identifying the out-of-scope path and pointing at
  `propose_task` for work that requires editing source files

WHEN conversation is in Direct mode, or in a sub-agent's Explore context (which has no
  worktree of its own)
THE SYSTEM SHALL NOT register the patch tool's task-dir-restricted variant — Direct mode
  gets the full unscoped patch tool, and Explore sub-agents get no patch tool at all

WHEN conversation is in Work mode
THE SYSTEM SHALL enable full patch tool functionality
AND the patch tool SHALL operate within the conversation's worktree directory

WHEN a patch operation targets a path outside the worktree directory
THE SYSTEM SHALL reject the operation
AND return a descriptive error identifying the out-of-scope path

**Rationale:** Explore mode is read-only for *source* files, but the agent still needs to
write the task file it proposes — so the patch tool is present in Explore, allowlisted to
the tasks directory only (REQ-PROJ-003). Out-of-scope writes are rejected with a clear
error. In Work mode the allowlist widens to the conversation's isolated worktree — a
conversation cannot use patch to modify the main checkout or another conversation's
worktree.

---

### REQ-PATCH-010: Anchored Insertion

WHEN agent requests insert_before with a unique anchor
THE SYSTEM SHALL insert newText immediately before the anchor's first byte
AND leave the anchor's bytes unchanged

WHEN agent requests insert_after with a unique anchor
THE SYSTEM SHALL insert newText immediately after the anchor's last byte
AND leave the anchor's bytes unchanged

WHEN the anchor is located via a fuzzy matching strategy
THE SYSTEM SHALL insert relative to the matched bytes in the file, not the agent-supplied anchor text

**Rationale:** Placing new content next to existing code is a common need (a new test beside its
siblings, an import below the last import). Without a dedicated affordance the agent must express
it as a replace whose newText repeats the anchor verbatim — the same re-transcription hazard the
clipboard exists to eliminate — or fall back to append_eof, which drops the content at the end of
the file where it is frequently syntactically wrong. Anchored insertion makes "keep this, add
beside it" a first-class operation in which the anchor appears once and cannot be corrupted. It
shares the anchor-location machinery, uniqueness rule, and duplicate diagnostics of replace; it is
the matched-anchor counterpart of prepend_bof and append_eof, which insert at the file's
boundaries.

---

### REQ-PATCH-011: Replace All Occurrences

WHEN replace operation is requested with replaceAll
THE SYSTEM SHALL substitute every exact occurrence of oldText with newText
AND waive the single-occurrence uniqueness requirement

WHEN replaceAll is requested
THE SYSTEM SHALL match exact occurrences only
AND NOT apply the fuzzy recovery strategies (dedent, trimmed-line, skeleton)

WHEN replaceAll is set on any operation other than replace
THE SYSTEM SHALL reject the operation rather than silently ignore the flag

WHEN replaceAll finds no exact occurrence and oldText is genuinely absent
THE SYSTEM SHALL return the "old text not found" error

WHEN replaceAll finds no exact occurrence but oldText matches existing text under the fuzzy strategies (a near match differing by whitespace or Unicode lookalikes)
THE SYSTEM SHALL return an inexact-match diagnostic that says the text is present only as a near match, that replaceAll is exact-only, and that the agent should copy the exact bytes or use per-site replace patches

**Rationale:** Mechanical refactors that change every copy of a repeated block to the same new
text are otherwise impossible in one call: the uniqueness rule rejects them, and widening context
cannot disambiguate genuinely identical blocks. replaceAll is the explicit, opt-in escape hatch for
that case, and being opt-in keeps the default safe — a multi-site replacement never happens by
accident. Fuzzy recovery is deliberately excluded: it exists to pin down a single best candidate
when the agent's text is slightly off, a notion that has no well-defined meaning across many
occurrences, and applying it to "all" risks rewriting bytes the agent never intended to touch.
Because exact-only matching would otherwise surface a slightly-off oldText as a misleading "not
found" when the text is plainly in the file, the inexact-match case is reported distinctly — the
failure itself tells the agent how to recover rather than requiring prior knowledge of the
exact-only rule.
