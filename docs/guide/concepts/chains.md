---
title: Chains
summary: A run of conversations linked by continuation, named and queryable as one unit — ask it recall questions without re-explaining.
category: concepts
keywords: [chain, continuation, recall, q&a, ask the chain, freshness]
related: [concepts/conversations.md, reference/glossary.md]
---

# Chains

When you continue one conversation into the next — #41 into #42 into #44 — those
links form a **chain**: a single run of work you can name and *query as a unit*.
You don't assemble a chain; it emerges from the continuation graph.

```
 #41 ──▶ #42 ──▶ #44        one chain · "auth refactor"
     continued  continued

 Ask the chain ──▶ a read-only agent reads across every member
```

The point is **recall without re-explaining**: instead of extending a long
conversation — or starting a fresh one and re-supplying context — just to ask
"what did we change in the parser?", you ask the chain.

## How it works

- **Emerges automatically.** Any conversation continued into another forms a
  chain; standalone conversations stay ungrouped. Chains are linear — one line
  of continuations.
- **You name it.** A chain carries an editable name; clearing it to whitespace
  removes the name.
- **Ask it.** A recall question runs a **read-only** agent scoped to the chain:
  it searches and reads across every member, cannot read outside the chain, and
  cannot change anything. Each question is answered fresh — prior answers are
  never fed back — so quality stays consistent as history grows.
- **Answers persist** per chain, each tagged with its age if the chain grew
  after the answer was produced.

## What you'll see

A chain is nested under its name in the sidebar. Its **chain page** is where you
ask recall questions and read past answers — each answer streamed live as it's
written, and tagged with its age when the chain has grown since you asked.

> **Remember:** the chain Q&A agent is read-only and chain-scoped — it recalls
> across the whole run but cannot change anything or see beyond the chain.

## See also

- [Conversations](conversations.md) — what a chain is made of
- [Glossary](../reference/glossary.md) — chain, continuation, chain Q&A
