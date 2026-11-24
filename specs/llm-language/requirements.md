# LLM Language

## User Story

As a Phoenix user, I want to choose how the assistant communicates so I can follow its work with less effort.

## Requirements

### REQ-LL-001: Select and inspect a built-in language

THE SYSTEM SHALL offer Phoenix, Caveman, and ADHD in a shared language catalog with descriptions and inspectable prompts.

WHEN the user starts a conversation
THE SYSTEM SHALL offer a language picker on desktop and mobile, initialized to Phoenix independently of the saved global preference, and submit the selected language explicitly with that conversation's creation intent.

THE SYSTEM SHALL preserve the selection through draft reloads and creation retries, keep existing conversations' selected language, and pass that selection to continuations and sub-agents.

THE settings UI SHALL show inspectable prompts without a global language selector.

### REQ-LL-002: Actionable ADHD responses

WHEN a conversation uses ADHD
THE SYSTEM SHALL retain Phoenix operational instructions and add response guidance that leads with the answer or next action, numbers sequential steps, keeps visible groups small, anchors ongoing work in its goal and state, suppresses tangents, and makes verified progress clear.

THE guidance SHALL preserve completeness, uncertainty, user-requested depth, tool contracts, authorization and evidence standards; avoid invented timings and unnecessary user homework; and apply to ordinary conversations, the Coordinator, and its research agents.

### REQ-LL-003: Change Ember's language independently

THE SYSTEM SHALL offer an Ember-specific language selector after Coordinator creation, showing its saved language and the shared catalog.

WHEN the user changes Ember's language while its current transcript is idle
THE SYSTEM SHALL persist the selection for that Coordinator, apply it to subsequent requests and continuations, and preserve its identity, history and authority.

THE SYSTEM SHALL reject a change while Ember is starting, working, or its current transcript changes during the save, explain the refusal, and retain the saved selection. The control SHALL expose loading and save failures with a retry path.

Changing Ember's language SHALL NOT change the global default, ordinary conversations, or already-created research agents.
