---
name: phoenix-guide-sync
description: Audit the Phoenix User Guide (docs/guide/) for drift against the code and specs, then fix or report it. Use on a schedule, after merging a feature/spec change, or when asked to "sync the guide", "check the docs for drift", or "update the user guide".
---

# Phoenix Guide Sync

The user guide under `docs/guide/` documents Phoenix for product users. The code
and specs move; the guide rots. This skill detects the rot and resolves it —
applying safe fixes directly and surfacing judgment calls instead of guessing.

Run it routinely (see [Scheduling](#scheduling)), or on demand after a feature
lands.

The authoring principles you enforce live in `docs/guide/AUTHORING.md` — read it
first. Any page you add or rewrite must pass its pre-flight checklist; in
particular, **Principle 1 (ground every UI label in its rendering component,
verbatim)** is the defect this skill exists to catch.

## Sources of truth

The guide is downstream of these. When they and the guide disagree, **they** are
right and the guide is the bug.

| Guide area | Source of truth |
|------------|-----------------|
| Feature/concept inventory | `specs/*/executive.md` (and `requirements.md`) |
| Tool inventory & behavior | `crates/phoenix-ide/src/tools.rs` registry + `crates/phoenix-tools/`; each tool's `specs/<tool>/executive.md` |
| Exact tool values (caps, limits, statuses) | the tool's spec + Allium `specs/<tool>/*.allium` |
| Modes & lifecycle | `specs/projects/executive.md`, `specs/bedrock/` |
| Keyboard shortcuts | `specs/keyboard-interaction/`, `ui/src/components/ShortcutHelpPanel.tsx` |
| Input grammar (`@` `/` `./`) | `specs/inline-references/` |
| **Exact UI labels quoted in how-tos/concepts** | the React component that renders them under `ui/src/` (button text, card titles, banners, badge tooltips) |
| In-app routes the guide links to | `ui/src/App.tsx` |
| Manifest / nav ordering | `docs/guide/SUMMARY.md` |

## Drift categories

Check all four, in order:

1. **Structural** — cheapest, check first.
   - Files in `docs/guide/` not listed in `SUMMARY.md`, or `SUMMARY.md` links to
     files that don't exist.
   - Broken relative `.md` links between guide pages.
   - Missing/invalid frontmatter (`title`, `summary`, `category`, `keywords`,
     `related` required on every page; see `_templates/`).
   - `related:` entries pointing at nonexistent files.
2. **Coverage** — what exists in code but not in the guide, and vice versa.
   - A tool in the registry with no `reference/tools/<tool>.md` card (or only a
     `*(planned)*` stub in `SUMMARY.md`).
   - A spec under `specs/` describing a user-facing feature with no concept/how-to
     page. (Skip contributor-only specs — this guide is product-users-only.)
   - A guide page for a tool/feature that no longer exists or was renamed.
3. **Content** — documented values that are now wrong. These are the costly ones.
   - Reference cards state exact "drift targets" (e.g. bash's *8 live handles*).
     Re-verify each against its spec.
   - Mode × tool-availability claims vs `specs/projects` + tool registry.
   - Keyboard shortcuts vs `ShortcutHelpPanel.tsx`.
   - Route links vs `ui/src/App.tsx`.
   - **Quoted UI labels.** Any string a page puts in quotes or `code`/**bold** as
     something the user clicks or reads (button text, workflow-card titles,
     banners, badge tooltips) must match the literal string in the rendering
     component verbatim. Paraphrased or invented labels are the single most
     common quality defect — grep the component for the quoted text; if it's not
     there word-for-word, it's drift.
4. **Template conformance** — pages drifting from the section's template shape in
   `_templates/` (missing "See also", reference card missing its limits table, etc.).

## Procedure

1. **Snapshot the guide.** List `docs/guide/**`, parse each page's frontmatter,
   and read `SUMMARY.md`.
2. **Build the current inventory** from the sources above: the tool list, the
   spec list with one-line summaries, the modes table, the shortcut list, the
   route list, and the headline values for each existing reference card.
3. **Diff** inventory against the guide, per category. For broad sweeps across
   many specs/tools, delegate to a search sub-agent and keep only the findings.
4. **Classify** every finding (next section) and act on it.
5. **Emit a drift report** (format below) even when you also applied fixes, so
   there's a record of what changed and what's outstanding.
6. **Pass the pre-flight.** Any page you added or rewrote must satisfy the
   `docs/guide/AUTHORING.md` checklist before you commit it — especially UI
   grounding. Keep `SUMMARY.md` in sync with any page you add or remove, and run
   `./dev.py check` if you touched a file the check validates.

## Classify, then act

<classification>
**Auto-fix** (apply directly, no need to ask):
- Structural drift: add a missing `SUMMARY.md` entry, fix a broken relative link,
  add a missing frontmatter field, correct a `related:` path.
- Pure renames where the new name is unambiguous from the source.
- Reference-card values that are *factually* stale where the new value is stated
  unambiguously in the spec (e.g. cap 8 → 12) — fix the number, note it in the report.

**Propose** (create a task with `taskmd new`, link it in the report) when the fix
needs real writing or judgment:
- A new user-facing feature needs a whole new concept/how-to page.
- A feature was removed and its page needs rewriting, not just deletion.
- Coverage gaps that are a *(planned)* stub becoming real content.

**Ask** (use `AskUserQuestion`) when the source itself is ambiguous:
- A spec and the code disagree about a value — don't silently pick one.
- It's unclear whether a spec describes a user-facing feature (in scope) or a
  contributor/internal concern (out of scope for this guide).
</classification>

Never invent a value to fill a gap, and never delete a page to make a link error
go away — fix the link or the manifest instead.

## Output: drift report

Always print this, even on a clean run:

```
GUIDE DRIFT REPORT — <date>
Structural:   <n fixed> / <n outstanding>
Coverage:     <n fixed> / <n proposed> / <n outstanding>
Content:      <n fixed> / <n flagged>
Conformance:  <n fixed>

Applied:
- <file>: <what changed>

Proposed (tasks):
- <task id>: <title>

Needs a decision:
- <finding + the question asked>
```

A clean run prints the header with zeros and "No drift detected." — that is a
valid, useful result, not a no-op to skip.

## Scheduling

This skill is the instruction set; scheduling is separate. To run it on a cadence
during a session, use `/loop` (e.g. `/loop 1d /phoenix-guide-sync`). For a
one-shot future check, `send_later` works if available. On a clean run it should
end quietly (print the zero report, no task, no message); only escalate when there
is drift to fix or a decision to make.
