# Cover direct-turn reacceptance in E2E

The continuation E2E stopped after observing the second response, so it did not prove that durable ownership was released and a later message could be accepted. Add a low-wall-time acceptance probe to the existing continuation scenario and cancel only if the probe starts runtime work.

## Acceptance criteria
- A completed follow-up is immediately followed by another public `POST /chat` assertion.
- HTTP 409 `conversation_busy` fails the scenario.
- The probe does not wait for a full additional LLM response.
