# Fixed per-request overhead (reviewer, measured from code)

The production fit put the median fixed overhead at **≈18.7k tokens per request**.
Two sources alone come to about that, at the audit's 0.255 tokens per byte:

| Source | Bytes | ≈ tokens | Sent when |
|---|---:|---:|---|
| Tool definitions for a parent conversation (`ToolRegistry::direct`, 28 tools) | 34,910 | 8.9k | every request |
| `AGENTS.md` of this repo, injected whole as `<project_guidance>` | 36,786 | 9.4k | every request in a phoenix-ide transcript |
| Sub-agent Work registry | 26,682 | 6.8k | every sub-agent request |
| Sub-agent Explore registry | 15,704 | 4.0k | every sub-agent request |

Measured with a throwaway test that serialized `definitions()` (name, description and
JSON schema). That test was not committed. MCP tools, conversation tools and the skill catalog come on top.

## Largest tool definitions (parent registry)

| Tool | Description bytes | Schema bytes | Total |
|---|---:|---:|---:|
| bash | 3,687 | 1,677 | 5,368 |
| patch | 2,627 | 1,547 | 4,179 |
| browser_profile | 510 | 3,293 | 3,818 |
| tmux | 2,237 | 338 | 2,579 |
| spawn_agents | 1,093 | 1,450 | 2,555 |
| browser_key_press | 824 | 812 | 1,653 |
| tmux_run | 466 | 1,032 | 1,506 |
| present_svg | 988 | 432 | 1,431 |

The **12 browser tools total 11,267 bytes (≈2.9k tokens, 32% of the definitions)** and are sent
on every request. Only `ask_user_question` uses `defer_loading`, although `tool_search`
already exists to load deferred tools on demand.

## Leads

1. **Defer the browser tools.** If few transcripts use them, about 2.9k tokens leave every request,
   roughly 15% of fixed overhead. This needs the usage rate from production (asked below).
2. **Trim `AGENTS.md`.** It is the single largest prompt input for phoenix-ide work. Much of it is
   rarely relevant per turn (the roadmap history, deployment, the Allium workflow, the CSS rules) and
   could move behind skills or on-demand reads. This is a project-content choice, not a
   code change.
3. **bash, patch and tmux descriptions** are 2–3.7KB each. Worth a read-through for repetition.

These tokens are almost always cache reads (≈0.1× price), so the dollar saving is smaller than
the token share suggests. They still occupy the context window and dilute attention on every turn.

## Questions for the worker (round 2, or round 1 if cheap)

- The share of transcripts, and of requests, that call any `browser_*` tool; the same for
  `tmux`/`tmux_run`, `present_svg` and `spawn_agents`.
- The fitted intercept split by working repository (hashed), to confirm that
  phoenix-ide transcripts carry the ~9k `AGENTS.md` premium.
