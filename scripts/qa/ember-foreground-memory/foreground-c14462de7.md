# Foreground qualification: quiet pass, contextual failure

One approved run of both frozen journeys was performed at source
`c14462de76e941b5efb9edef31ca8cd9fa7b57fd`. Independent full-trace review accepted
J1 and confirmed J2 as a genuine failure. No retry, fixture/oracle change,
threshold relaxation or fallback was used. This is not complete foreground
qualification or a deployment claim.

## Frozen configuration and evidence

- Discovered model `gpt-6-astra`, actual outbound model `gpt-6-astra`, high effort,
  Codex connection, PhoenixNative and the authenticated coordinator skill catalog.
  Every observed request carried `Explicit(High)`.
- Normal assembled surface: 29 tools. Definition SHA256
  `be42ab762e8f1347502189808de9760ba2664fb64789cca82c098e6a3815ed37`.
- Retained test binary SHA256
  `bb65ff2c32e930a94c1ec6b4c4df3d277811bb7261f1a7825682f52e8dcd3187`.
- Approved prepared-manifest SHA256
  `14a05618eb40408c2abfd0d8f1f046ca76ad51f200506b0d1e0ce3da2b49cea8`.
- Endpoint-configuration SHA256
  `1e44b44ac8908fd78e1e50d1f68a2a0dde267444ecc3c778484e1517da82876d`.
- J1 static system SHA256
  `746b1eb5800151bfc864af511bc7a8827c4f569f189788bbf6fb73d8ba5d7e8a`;
  closed seed SHA256
  `70a92156712555226779d1f3db6c5d9836149ab07da16d44d44729b0006312dd`.
- J2 static system SHA256
  `7670cc98185d51943cc3f132a49594a5afed728096d7d8166fbf6de9bef53b52`;
  closed seed SHA256
  `192773ad71719dab50623246ef189110512f9e766943307197bf62f5bbb3dc64`.

The exact [predeclared journeys, limits and run procedure](README.md) were
reviewed before inference. Preparation used consistent SQLite snapshots before
mock probes, real native record/update execution and actual source receipts.
Probe inputs were absent from live seeds. The real structured-reply/file-read
fixture and the existing pending-assistant checkpoint regression both passed.
No compiler ran during the model trial.

## J1: accepted post-review quiet

The greeting selected due Cobalt while Sage was recently reviewed. Ember read
the actual Cobalt record, updated it through CAS and asked its specific saved
date-range/comparison-period question. The user then paused it until explicitly
saying resume. Ember read the current record, revision-updated the pause and
settled without asking again. The following arithmetic input produced exactly
`13.` with zero tools and no renewed cue. Both requested items were recently
reviewed; earlier prefixes, coordinator identity and restrictions were retained.
All visible prose was inspected, including the greeting before tools.

The live model used a prose question in Idle; structured `respond_to_question`
was exercised by the separate deterministic actual-protocol fixture, not by
this live trace. No real healthy worker existed, so this does not qualify worker
health detection or actual subscription delivery.

J1 used seven requests and 1,177,104 accumulated request Debug bytes. Reported
usage: 17,148 input tokens, 89,600 cache-read tokens and 490 output tokens.
Its private full trace SHA256 is
`9c1a386bb49a49dc9da971f4f7b316e9e8e4f1aeabb44567558e13db98318e3a`.

## J2: genuine hidden-request recovery failure

The prepared seed held ten meaningful requests across three actual transcript
identities. Both handoffs omitted them. Harbor was selected from the two items
omitted from the first eight index entries before inference; the current prompt
named keyboard access for a form dialog, never the saved title, identifier or
owed question. All items were recently reviewed and there was no due cue.

Ember made no tool calls and returned general keyboard-dialog advice. It
mentioned focus on exit as one generic recommendation, but never retrieved the
Harbor item, original assistant checkpoint or saved question about which control
should regain focus. The unchanged actual-retrieval oracle failed. Coincidental
topic overlap is not recovery.

J2 used one request and 156,412 Debug bytes. Reported usage: 13,571 input tokens,
zero cache-read tokens and 425 output tokens. Its preserved private full trace
SHA256 is
`7ae8ce37a3c7db111dc42d79705118142ed0f1bd2bd524032333996bbc1c760c`.
The failure stopped execution before J2's later prefix/authority assertions, so
those assertions are not claimed as having passed in this journey.

## Bounds, result and correction

The entire run took 45.36 seconds and made eight requests totaling 1,333,516 Debug
bytes, 30,719 reported input tokens, 89,600 cache-read tokens and 915 output
tokens. Output usage was known; no response exceeded 2,048 reported output
tokens or 16 KiB serialized content. These are returned-output checks, not a
Codex wire-level generation/billing cap. Per-request/per-turn time and request
limits were respected. No model retries or external-effect attempts occurred.

The original live manifest remains `recording` because the assertion stopped
the test; its unchanged SHA256 is
`fe5f17a750f1728bf5b2d49ab791503de2602d768ea3b3ea366e081d41cc4d53`.
Both actual journeys and their raw artifacts remain preserved, rather than
rewriting the failed run as accepted.

The targeted correction makes static guidance require bounded relevant
saved-intent retrieval before answering current work-planning/next-step
questions. A partial index does not establish absence. It separates that
current-request retrieval from the 24-hour unsolicited-review cooldown and
keeps neutral arithmetic/greetings quiet absent an actual due cue. No new tool,
ranker, model, wakeup or lifecycle is introduced. The same fixtures/oracles and
budgets require a new prepared manifest, independent approval and full model run
before the correction can be claimed effective. Scanner lifecycle/composition,
final inventory and deployment remain separate gates.
