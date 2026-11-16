# Paired correction: recovery observed, read-only harness rejection

The independently approved paired run at
`d50bbbf54107e57f6a5f73beb43e96313e269942` preserved both original closed seeds,
all source/target identities, prompts, expected outcomes, prefixes, tools and
budgets from the [first run](foreground-c14462de7.md). Only the reviewed static
guidance, test settlement synchronization and actual probe metadata changed.
Independent full-output review accepted J1 again and confirmed J2's actual hidden
record/source/stopping-point recovery. The complete run remains **unqualified**:
the test guard incorrectly rejected a legitimate local read-only source search.
No retry, fallback, fixture/oracle change or threshold relaxation was used.

## Exact evidence

- Model/outbound model `gpt-6-astra`, high effort, Codex, PhoenixNative and the
  authenticated coordinator skill catalog. All ten requests carried
  `Explicit(High)` with known output usage.
- Retained binary SHA256
  `9386a4afd67b075c903085599211619f83396bb0cb607c91da4d4adfbe51440f`.
- Approved re-prepared manifest SHA256
  `c43a792f384bb1c96db4f5a952afc908ce6719d35b6b80ca36e282913347e7ee`,
  bound to original manifest
  `14a05618eb40408c2abfd0d8f1f046ca76ad51f200506b0d1e0ce3da2b49cea8`.
- The same 29-tool definitions SHA256
  `be42ab762e8f1347502189808de9760ba2664fb64789cca82c098e6a3815ed37`.
- J1 system SHA256
  `118db5737a95618db15c46ddcccb7de5ec4f80ab5b71a9c46e26d9f5d71cb86f`;
  J2 system SHA256
  `7fe7b0501446438aabf922c56f1daf2708a742960e9b6a64d356d71e111ff397`.
- J1 full trace SHA256
  `f34c10e399ceef87d24c29ff698fdfcbb35e96d975cb375c0c82742f7776e69b`;
  J2 full trace SHA256
  `d1bb7b3048f2eb7ad5792abcaa7e2fc91bc979131e82fbcfe7b626b853da0e5c`.
- Original stopped live manifest SHA256
  `be61c6ddf6a370d28a727c19faa7462afc542fd8915cc2736eb05e2a619ed9a7`.
  Its `recording` status and all raw artifacts remain unmodified.

## Observed behavior and limits

J1 read Cobalt, asked the saved date-range/comparison-period question, then read
and CAS-updated the explicit user pause. The arithmetic turn produced exactly
`13.` with no tools or renewed cue; both items were recently reviewed. Prefix,
coordinator identity, real runtime settlement and lifecycle checks passed. It
used a prose question, so live structured reply remains unexercised; the actual
structured protocol passed the deterministic file-backed test. J1 used six
requests, 999,096 Debug bytes, 16,502 reported input tokens, 74,112 cache-read
tokens and 405 output tokens.

J2 fetched one bounded next page, found the original omitted Harbor record and
read it through the real native tool. That result contained the exact target,
original assistant checkpoint and full saved stopping question. It then tried
`search_conversations` with `Harbor keyboard focus dialog` to recover source
context. The guard wrongly returned its external/effectful denial. This was a
**harness capability rejection**, not an attempted authority breach or external
action; the actual search implementation was not exercised. The final response
asked the specific focus-destination question. It still used verbose general
keyboard-dialog framing rather than a concise explicit Harbor refresher.
There was no due cue or duplicate capture. J2 used four requests, 640,488 Debug
bytes, 14,703 input tokens, 41,600 cache-read tokens and 499 output tokens.
The final guard assertion stopped execution before J2's later prefix/authority
assertions; those later assertions are not claimed as passed.

The run took 53.54 seconds: ten requests, 1,639,584 Debug bytes, 31,205 reported
input tokens, 115,712 cache-read tokens and 904 output tokens. Every returned
response fit the unchanged observed-output/time/request limits. Requested output
limits are not a Codex wire-level generation/billing cap. No model retry or
external-effect attempt occurred.

## Narrow harness correction and scope

The guard now permits the actual registered local read-only
`search_conversations`, preserving explicit remote/action guards. The
file-backed deterministic fixture must verify actual search returns an ordinary
local source's exact transcript/message identities, and keeps Coordinator-chain
content excluded. No canned search result or production search change is added.

Global keyword search discovers ordinary-conversation messages and structurally
excludes the Coordinator chain. It cannot find the Harbor primary in this seed.
Saved own-history intent uses `tracking` plus its exact pinned
`read_conversation` source pointer. A real no-match search in the unchanged live
case is therefore legitimate; it must not be manufactured into a Harbor hit.

The next complete qualification requires a new exact binary, original-seed
repreparation, independent manifest approval and both unchanged journeys under
the same limits. This partial run is retained as evidence rather than rewritten
as a fully passing test. Scanner composition, final integration and deployment
remain separate gates.
