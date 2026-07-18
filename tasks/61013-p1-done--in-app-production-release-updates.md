# In-app production release updates

Turn the transactional production-deployment engine into a Phoenix product journey. Production users should discover, review, approve, initiate, and monitor a published Phoenix release update from the app, with durable progress and terminal outcome restored after Phoenix restarts or the browser reconnects.

## Product boundary

- Reuse the existing launchd one-shot helper, systemd transient root unit, and bare-Linux supervisor for disruptive activation, exact verification, and rollback.
- Phoenix UI/API own release discovery, immutable candidate preview, explicit operator approval, durable progress presentation, and reconnect recovery.
- The running Phoenix server must never directly replace or restart itself; activation remains independently owned.
- Limit the in-app surface to checksummed published releases. Keep local-HEAD builds, first install/bootstrap, manual migration, offline repair, status, and emergency recovery in `dev.py`.
- Do not infer success from reconnect, PID, or port response. Render only backend-owned durable status and exact runtime identity.

## Incremental user journeys

1. **Discover and preview** — show current exact identity, backend, available stable release, resolved immutable commit, platform asset, checksum verification, and release notes without mutating production.
2. **Approve and hand off** — require explicit same-host authorized approval, prepare the immutable candidate, hand activation to the existing backend owner, and return a durable transaction ID before disruption.
3. **Reconnect and resume** — after browser disconnect or Phoenix restart, hydrate the active transaction from durable backend status and show validating, preparing, handed-off, activating, verifying, rolling-back, and terminal phases.
4. **Trust the outcome** — distinguish committed, precondition failure, verified rollback, and rollback failure; show exact installed/restored identity and actionable offline recovery guidance.

## Delivery constraints

- Define timeless spEARS requirements and an Allium lifecycle spec before implementation; add an ADR for the in-app control-plane/independent activation-owner boundary.
- Use typed API contracts and normalized persisted state; do not mirror backend status into competing representations.
- Gate host-local mutation with the existing structural same-host boundary and server-side authorization. Remote browsers may view portable status but must not receive host-local activation authority unless a separate secure authorization design is approved.
- Build the first vertical slice as read-only update discovery/preview, then approval/handoff, then reconnect-safe progress and terminal history.
- Add deterministic fixture journeys and disposable native-backend acceptance. No live production update without separate immediate approval.

## Acceptance

- An operator can preview an available release and exact immutable candidate in-app.
- An authorized same-host operator can explicitly approve a release update and receive a durable transaction ID.
- The update completes independently through launchd, systemd, or bare supervisor ownership.
- Reload/reconnect restores current durable progress and terminal outcome.
- Failed activation shows verified rollback identity; rollback failure remains visibly unresolved with its claim retained.
- `dev.py` remains a fully functional bootstrap and offline recovery surface over the same transaction protocol.
