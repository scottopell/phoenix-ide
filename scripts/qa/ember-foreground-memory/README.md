# Foreground memory qualification

This opt-in check supplements the accepted manual memory evaluation. It does not
change that evaluation's semantics or turn its disclosed post-review gap into a
pass. No provider trial is authorized merely by compiling this harness.

## Frozen journeys and oracles

Both journeys use an isolated file database, the actual coordinator tool registry
and authenticated skill catalog, PhoenixNative, discovered `gpt-6-astra` at high
effort, actual runtime input admission and persisted source receipts. Setup uses
explicit mock completions whose tool calls execute against the real registered
tools; SQL/read results are never canned. All visible assistant prose, tool calls,
tool results and model requests are retained for independent review.

1. **Post-review quiet.** Seed two requested items. Cobalt owes a specific invoice
   comparison-period question; Sage must wait without checking or nudging its
   owner. Review Sage through the native CAS update while both items are recent.
   Then backdate only Cobalt's tracking creation/update/review timestamps and the
   original setup messages by three days, preserving the later Sage review and
   message chronology. Actual attention rows remain untouched. The live neutral
   greeting must expose the due Cobalt
   question and inspect its actual item. A normal user reply pauses it until an
   explicit resume. A structured question, if used, is answered through the
   actual `respond_to_question` service on the same running runtime handle.
   The reply must settle, preserve the pause through a real CAS update and leave
   both items recently reviewed. The following arithmetic input must produce
   only the correct answer, zero tool calls and no newly captured due cue.
   All visible prose, including pre-tool text and structured questions, counts.
   No real worker is running; worker health, subscription delivery and nudging
   behavior outside this isolated guard are not qualified.
2. **Contextual hidden recovery.** Seed ten unrelated requested outcomes with
   concrete stopping questions. Continue the actual transcript twice using
   handoffs that omit them. List the actual first eight and freeze one of the
   two omitted items, its current topic prompt and its expected stopping point
   during preparation, before inference. The prompt names only the domain,
   never the old title, identifier or owed question. All items are recently
   reviewed: there must be no due cue. A real bounded read/query must return the
   selected item identifier, original assistant source-checkpoint identifier
   and full saved stopping point. The response must ask the saved specific
   question rather than provide coincidental generic advice. No duplicate item
   may be added. This is one prepared topic, not a relevance-ranking benchmark.

Both journeys preserve the earlier persisted message prefix and coordinator
identity. External actions are intercepted and recorded as failures; read-only
local tools, native tracking mutations and the question tool execute normally.
These checks exercise real admitted runtime turns and structured reply
validation, but not HTTP transport, UI interaction, process-crash recovery,
scanner behavior, remote workers or deployment.

## Budgets and output capability

Per journey: at most 10 provider requests, 2,000,000 accumulated request Debug
bytes, 60 seconds per provider request, 120 seconds per whole turn and eight tool
cycles per turn. Across both: at most 20 requests and 4,000,000 Debug bytes. No
retries, skipped cases or fallback model are allowed. A timeout/provider failure
ends qualification without automatic retry.

The request asks for at most 2,048 output tokens. The Codex route does not enforce
that field on the wire; it is **not** a hard generation or billing cap. The
harness accepts a returned response only when output usage is reported and is
at most 2,048 tokens, and serialized content is at most 16 KiB. Unknown or larger
output ends qualification before executing its tool calls, after the provider
may already have generated it. Trace records distinguish the requested limit
from reported usage and accepted content bytes.

## Running

First coordinate the shared compiler and run the deterministic fixture:

```sh
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 RUST_TEST_THREADS=1 \
  cargo test -p phoenix_ide --lib foreground_question_reply_uses_real_protocol_and_file_reads -- --nocapture
```

Retain a copy of the resulting test executable in an ignored artifact directory.
Set `PHOENIX_FOREGROUND_QA_TEST_BINARY` to its absolute path. The wrapper verifies
that it contains the exact ignored qualification test, hashes it and loads the
normal resolved Phoenix environment. It never builds or starts a server.

```sh
PHOENIX_FOREGROUND_QA_MODE=prepare \
PHOENIX_FOREGROUND_QA_OUTPUT=/absolute/fresh/prepared \
  ./dev.py qa ember-foreground-memory
```

Preparation makes no model calls. It freezes normal discovered model/effort,
actual outbound model, connection, endpoint-configuration hash, authenticated
catalog/language, binary, tool definitions and assembled system prompt. It
creates consistent SQLite snapshots using `VACUUM INTO` before admitting the
mock prompt probe; the live seed has neither probe input nor its in-flight claim.

Independent review must approve the exact `manifest.json` digest before live:

```sh
PHOENIX_FOREGROUND_QA_MODE=live \
PHOENIX_FOREGROUND_QA_PREPARED=/absolute/prepared/manifest.json \
PHOENIX_FOREGROUND_QA_APPROVED_SHA256=approved_digest \
PHOENIX_FOREGROUND_QA_OUTPUT=/absolute/fresh/live \
  ./dev.py qa ember-foreground-memory
```

Live runs verify the frozen route/binary/limits, copy only the prepared closed
SQLite snapshot, and admit fresh actual inputs. Automatic oracle success is
recorded as pending independent full-trace review; it is not activation or a
production claim. Results and screenshots remain private ignored artifacts;
commit only a concise sanitized qualification summary after review.
