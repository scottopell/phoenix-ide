# Frontend tooltipify pass

## Goal

Take an autonomous pass through the Phoenix React frontend using `/Users/scottopell/dev/scott-skills/tooltipify` principles: every interactive surface should clearly describe what it does, without inventing behavior that is not wired in code.

Existing coverage is partial: several components already use `title`/`aria-label`, but gaps remain in settings controls, conversation actions, question options/navigation, status banners, dialogs, and view controls.

## Scope

- Use the tooltipify workflow:
  1. Run the bundled AST scanner against `ui/src` if available:
     `uv run /Users/scottopell/dev/scott-skills/tooltipify/scripts/scan_interactive_elements.py --target ui/src --format json`
  2. Treat scanner output as the source of truth for interactive elements.
  3. Read handlers/icon/context before choosing tooltip text.
  4. Apply tooltips using the existing frontend convention: native `title` plus matching `aria-label` for icon-only controls.
  5. Report filled gaps and deliberately skipped elements.

- Focus on shared/user-visible React surfaces under `ui/src/components` and adjacent UI utilities, especially:
  - `ConversationList` row, chain, archive, new-conversation, rename/archive/delete actions.
  - `SettingsDropdown` trigger, theme/density/notification/language/Codex actions.
  - `QuestionPanel` breadcrumbs, navigation, submit/dismiss, options, notes affordances.
  - `ContextIndicator`, `BrowserViewPanel`, `LlmStatusBanner`, `ConfirmDialog`, and small icon/action buttons.
  - Linkified file references or role=`button` spans where they behave like controls.

## Tooltip writing rules

- Prefer short verb + object text: `Archive conversation`, `Delete chain (can't be undone)`, `Open chain "name"`.
- Do not merely restate visible labels unless the current UI gives no additional context.
- Make toggles state-aware: `Switch to compact density`, `Switch to full density`, `Show archived conversations`, `Show active conversations`.
- Destructive actions should warn when the action cannot be undone or opens a destructive confirmation.
- Include keyboard shortcuts only when code verifies them.
- Do not add tooltips to decorative/non-interactive elements.
- Do not add a tooltip to a dead/unhandled control; report it instead.

## Implementation notes

- Avoid introducing a second tooltip library or custom tooltip component for this pass.
- Keep behavior unchanged; this is a discoverability/accessibility copy pass.
- For icon-only buttons, ensure `aria-label` matches the tooltip text or is semantically equivalent.
- Prefer constants/helper functions only if they reduce repeated dynamic strings without overengineering.

## Validation

- Run the scanner before and after; record remaining intentional skips.
- Run relevant UI checks available through `./dev.py check` or at minimum TypeScript/lint lanes if full check is too slow.
- Spot-check representative screens manually if the dev server is running.

## Deliverable

A concise summary listing:

- Number of interactive elements scanned.
- Number of tooltip/aria gaps filled.
- Files changed.
- Any skipped/dead/ambiguous controls and why.
