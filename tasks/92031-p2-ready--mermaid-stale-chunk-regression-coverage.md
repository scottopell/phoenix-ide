# Add production regression coverage for stale Mermaid chunks

## Context

Phoenix lazy-loads Mermaid and Mermaid diagram types as content-hashed Vite chunks. A tab kept open across deployment can request an old chunk hash after the new binary has replaced the embedded asset generation.

PR #636 added the product remediation at the deployment boundary:

- SPA entry documents revalidate;
- content-hashed assets remain immutable;
- Vite preload failures log the rejected module and show one dismissible, manual **Reload Phoenix** notice;
- reload remains user-controlled so unsent attachments are not discarded automatically;
- Shiki grammar-pruning configuration participates in chunk hashes.

The remaining gap is production-build regression evidence. Unit tests mock Mermaid and therefore cannot prove emitted chunk availability, preload behavior, or recovery across asset generations.

## Scope

Add a production-build browser test using the existing mock-model `[[scenario:mermaid]]` response.

1. Start an isolated Phoenix server with production-built embedded assets and the mock model enabled.
2. Send `[[scenario:mermaid]]` and verify its flowchart and sequence diagram render without module-loader console errors.
3. Simulate a missing or stale Mermaid chunk at the browser/server boundary.
4. Verify Phoenix logs the failed module detail and shows one manual reload notice without automatically reloading.
5. Restore a coherent asset generation, choose **Reload Phoenix**, and verify both diagrams render.
6. Verify a malformed Mermaid fence remains a parse/render error with source fallback and does not trigger deployment recovery.

## Acceptance evidence

- The test exercises emitted production hashes rather than mocked `import('mermaid')` behavior.
- A coherent production build renders both mock diagrams.
- A controlled missing chunk produces one actionable manual-reload notice and no reload loop.
- Recovery into a coherent generation renders both diagrams.
- Mermaid syntax failure remains distinct from module acquisition failure.
- `./dev.py check` passes.

## Risk and priority

This is P2 regression coverage, not an open P1 product defect. The failure requires a deployment, a tab running the previous generation, and a newly requested lazy chunk that was not already cached. When it occurs, Mermaid source remains readable and the merged notice provides manual recovery.

## Non-goals

- Do not add a service worker or retain historical asset generations.
- Do not add general React error boundaries or per-viewer recovery machinery.
- Do not make Mermaid eager without measured bundle evidence.
- Do not automatically reload pages with in-progress composition.
