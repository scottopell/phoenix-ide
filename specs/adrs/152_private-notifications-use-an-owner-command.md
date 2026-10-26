# ADR-152: Private notifications use an owner command

- **Status:** Accepted
- **Date:** 2026-10-23
- **Affects:** REQ-SG-015; private notification returns

## Context

A self-authored Slack self-DM does not establish a device notification. A
personal notifier may require credentials and routing specific to its owner.
Those details do not belong in the gateway's public implementation.

## Options considered

1. Embed one provider's authentication and workflow API in Phoenix.
2. Treat posting a self-DM as confirmed notification delivery.
3. Use a provider-neutral local executable with explicit submission and proof.

## Decision

Use a versioned stdin/stdout JSON command protocol, with fixed owner-configured
absolute executable and arguments. Submit generic text and a Phoenix link;
retain an opaque receipt. Freeze the command before submission and use that
same command and receipt for read-only status inspection after settings change.
Require exact notification, owner and receipt correlation plus nonempty proof
of recipient delivery. Provider acceptance alone is not proof. Bound execution
and output; discard stderr. Ambiguous submission is never automatically retried.

## Consequences

- Provider credentials and routing remain in private local configuration.
- Owners supply their own notifier and must validate its recipient evidence.
- Delivery confirmation does not establish that a device displayed an alert;
  live acceptance includes human confirmation of that behavior.
- Frozen argv does not freeze executable file contents. The owner controls and
  must retain a compatible adapter for outstanding receipt inspection.

## References

- `docs/slack-gateway-setup.md` notification protocol
- `notification::notify`, `notification::observe`, `slack_begin_notification_return`
