Child of task 92010.

Concern:
- Replace ADR-025 as needed for the settled lifecycle specification direction and make the shared ADR chain and index internally consistent.

Settled terminology guardrails:
- ADR wording must reuse the settled lifecycle and authority terminology from the normative baseline.
- Describe replaced decisions as superseded or replaced ADR history, not as still-current requirements language.
- Keep “authority”, “provenance”, and “legacy mapping” distinct so the ADR does not blur normative concepts.
- Use the ADR chain to explain why the terminology was chosen; do not create a second normative vocabulary inside ADR prose.

Done evidence:
- The replacement/superseding ADR is present with clear rationale and consequences.
- ADR-025 handling is consistent with project ADR conventions and the README/index references are correct.
- Any backlinks or references from affected specs point to the right ADR identifiers.
- A grep/inspection pass confirms there is no stale ADR numbering or contradictory index entry left behind.
