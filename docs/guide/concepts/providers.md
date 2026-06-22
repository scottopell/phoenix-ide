---
title: Providers & models
summary: Phoenix talks to several LLM providers behind one interface — you pick the model, it handles the rest.
category: concepts
keywords: [provider, model, anthropic, openai, fireworks, gateway, picker]
related: [concepts/conversations.md, howto/getting-started.md, reference/glossary.md]
---

# Providers & models

Phoenix speaks to several LLM providers — Anthropic, OpenAI, Fireworks — behind a
single interface. You choose a **model**; Phoenix handles the provider-specific
details. The model is a property of the conversation, like its mode.

## How it works

- **One interface, many providers.** The agent runtime is the same whichever
  model you pick; only the backend differs.
- **A model registry.** Phoenix discovers available models from the configured
  gateway (or from the providers you've supplied keys for) and offers them with
  their context windows and descriptions.
- **Per-conversation choice.** You set a model when you start a conversation, and
  can change it between turns.

## What you'll see

A **model picker** in the conversation's status bar shows the current model.
It's available only when the conversation is **idle or errored** — not while the
agent is mid-run — and offers a recommended set with an option to show all.

> **Remember:** you can't switch models while the agent is working. Pick before
> you send, or wait for the conversation to go idle.

## See also

- [Conversations](conversations.md) — the model is one of its properties
- [Getting started](../howto/getting-started.md) — choosing a model up front
- [Glossary](../reference/glossary.md) — canonical terms
