# Centralize LLM language prompts and add prompt preview page

## Goal

Add a link from Settings to a route where users can inspect the exact built-in LLM language prompt text for Phoenix/regular mode and Caveman mode. Keep the language catalog centralized so future custom user overrides can extend one place instead of duplicating labels/prompts across Rust and UI.

## Current facts

- Built-in prompt text lives in `crates/phoenix-core/src/llm_language.rs`.
- Settings UI config for LLM Language lives in `ui/src/components/SettingsDropdown.tsx`.
- UI labels are duplicated in `LLM_LANGUAGE_LABELS` instead of coming from backend metadata.
- Existing API `/api/settings/llm-language` returns only `{ language, available }`.
- Existing `/api/system-prompt/:id` shows the composed prompt for one conversation, but not the built-in language prompt catalog.
- New SPA routes must be added in both `ui/src/App.tsx` and `crates/phoenix-ide/src/api/spa_routes.rs`.

## Implementation plan

1. Create a single backend language catalog API shape.
   - Add a typed language metadata struct near `phoenix_core::llm_language::LlmLanguage`.
   - Expose each built-in language with stable id, display label, description/tooltip, and exact built-in prompt snippets.
   - Include at minimum:
     - base prompt
     - Explore mode block template
     - Work mode block template
     - Direct mode block
     - Branch mode block template
     - sub-agent suffix
     - next-task hint template
     - PR autofix instruction template
   - Use harmless placeholders for dynamic values like branch, base branch, worktree path, tasks dir, next id, and artifact path so the page can show exact templates.

2. Update `/api/settings/llm-language` to return metadata.
   - Keep current fields for compatibility: `language`, `available`.
   - Add `languages: LanguageCatalogEntry[]` so Settings and the new page consume the same source.
   - Update `ui/src/api.ts` types.

3. Remove duplicated UI language metadata.
   - Replace `LLM_LANGUAGE_LABELS` in `SettingsDropdown.tsx` with labels/tooltips from the API response.
   - Keep a raw-id fallback for resilience.

4. Add prompt preview page.
   - New route, likely `/settings/llm-language` or `/settings/prompts`.
   - Page loads `api.getLlmLanguageSetting()` and renders one section per language.
   - Show current default inline and allow switching using the same setting API, or link back to Settings if we want read-only. Prefer allowing switching: this is the configuration page the Settings link points to.
   - Render prompt snippets in copyable/preformatted blocks with clear labels.
   - State clearly that the default applies to new conversations; existing conversations stay pinned.

5. Link from Settings.
   - In the LLM Language section, add a small link/button: `View prompts →`.
   - Close the dropdown and navigate to the new route.

6. Register SPA route.
   - Add React lazy route in `ui/src/App.tsx`.
   - Add matching route to `crates/phoenix-ide/src/api/spa_routes.rs` so refresh/bookmark works.

7. Tests/checks.
   - Add Rust tests that language catalog includes every `LlmLanguage::ALL` entry and non-empty snippets.
   - Add/update UI tests if existing settings tests cover the dropdown.
   - Run `./dev.py check`.
