# Upstream kache socket and hash-reuse patches

Create three concise upstream problem issues and three linked draft PRs for configurable daemon sockets, restored-artifact hash reuse, and fresh-output hash reuse. Bodies reserve an empty human context section and provide a clearly labeled AI summary with explicit disclosure. Publish focused branches to the authenticated user fork.

## Published upstream

Each issue and draft PR reserves an empty `Human context` section for the submitter, followed by a complete `AI summary` and explicit AI-assistance disclosure.

| Problem | Issue | Draft PR | Branch |
|---|---|---|---|
| daemon socket exceeds Unix path limits | [#539](https://github.com/kunobi-ninja/kache/issues/539) | [#542](https://github.com/kunobi-ninja/kache/pull/542) | `scottopell:fix/configurable-daemon-socket` |
| exact restored artifacts are rehashed | [#540](https://github.com/kunobi-ninja/kache/issues/540) | [#543](https://github.com/kunobi-ninja/kache/pull/543) | `scottopell:perf/reuse-restored-artifact-hashes` |
| fresh compiler outputs are rehashed downstream | [#541](https://github.com/kunobi-ninja/kache/issues/541) | [#544](https://github.com/kunobi-ninja/kache/pull/544) | `scottopell:perf/reuse-fresh-output-hashes` |

All PRs are independently reviewable against upstream `main` and use closing issue references. PR #542 includes the required configuration and daemon documentation. `just check` passes on each focused branch; PR #544 additionally reran its renamed targeted test after the final wording correction.
