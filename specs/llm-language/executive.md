# LLM Language — Executive Summary

## Requirements Summary

Choose Phoenix, Caveman, or an experimental ADHD response style when creating each conversation. Settings links to a read-only prompt viewer. ADHD keeps the standard operational guidance and adds action-first presentation instructions.

Ember has its own language selector in its activity controls. It loads and saves the current Coordinator's language independently of other chats and the global default. A save requires an idle current transcript and recreates its runtime so subsequent requests use the saved prompt and tool language. Continuations inherit that persisted language.

## Technical Summary

The Rust language catalog supplies creation choices and prompt inspection. The creation form submits its selection explicitly, defaults fresh drafts to Phoenix, and retains the current draft choice through reload/recovery. The backend's existing default-setting API remains for non-UI callers; the UI does not select or inherit it. Conversations persist the selected ID and propagate it through existing continuation and child creation paths. Migration 169 widens the creation-job language constraint in place using the existing writable-schema guard, preserving rows and foreign keys.

The ADHD text is an original adaptation of the presentation ideas in [ayghri/i-have-adhd](https://github.com/ayghri/i-have-adhd). It omits diagnosis assumptions, blanket approval rules, and ungrounded estimates. It does not install or invoke an external skill.

## Status Summary

| Requirement | Status | Evidence |
| --- | --- | --- |
| REQ-LL-001 | Implemented; focused checks pass | Catalog/parser tests, creation/replay API test, migration preservation test, UI submission test, desktop/mobile browser checks and draft reload |
| REQ-LL-002 | Experimental; static checks and one live sample pass | Prompt composition and operational-guidance checks; a new ADHD conversation using gpt-6-luna returned three direct numbered PR-review steps without tools. Broader model/style quality remains unverified. |
| REQ-LL-003 | Implemented; focused checks pass | Six backend regressions cover idle/current-head and pending-work guards, synchronized runtime invalidation, prompt inspection and restart/continuation. UI save/retry tests and real desktop/mobile save and reload checks pass. Independent adversarial review found no remaining actionable defects. |
