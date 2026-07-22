# iOS durable message submission handoff

Phoenix exposes one single-target submit-and-observe contract for browser, iOS,
Coordinator, and automation clients. `message_id` is the client-visible durable
handle; workflow and effect identifiers remain server-internal.

## Submit

```http
POST /api/conversations/{conversation_id}/chat
Content-Type: application/json
```

The client generates and durably stores `message_id` before sending. A successful
response means Phoenix durably owns the exact prepared message. It does not mean
the recipient runtime or LLM has completed.

```json
{
  "message_id": "B761AA5E-236C-4E6A-B567-37A44C4C24A7",
  "request_result": "created",
  "disposition": "pending_runtime"
}
```

```swift
enum MessageRequestResult: String, Codable {
    case created
    case replayed
}

enum MessageAcceptanceDisposition: String, Codable {
    case pendingRuntime = "pending_runtime"
    case runtimeAccepted = "runtime_accepted"
    case queuedSteering = "queued_steering"
    case cancelledSteering = "cancelled_steering"
}

struct MessageAcceptance: Codable {
    let messageId: String
    let requestResult: MessageRequestResult
    let disposition: MessageAcceptanceDisposition

    enum CodingKeys: String, CodingKey {
        case messageId = "message_id"
        case requestResult = "request_result"
        case disposition
    }
}
```

`created` means this request created the durable acceptance. `replayed` means the
same target-bound `message_id` and payload were accepted previously. The client
must treat both as success and must not create a new ID.

A `409 Conflict` means the same target-bound `message_id` was previously bound to
a different prepared payload. It is not safe to overwrite or silently retry that
ID. A retryable transport or server error does not prove non-acceptance.

## Reconcile after an ambiguous response or restart

```http
POST /api/conversations/{conversation_id}/messages/reconcile
Content-Type: application/json
```

```json
{
  "message_ids": ["B761AA5E-236C-4E6A-B567-37A44C4C24A7"]
}
```

Each result reports acceptance independently from transcript materialization:

```json
{
  "conversation_idle": false,
  "entries": [
    {
      "message_id": "B761AA5E-236C-4E6A-B567-37A44C4C24A7",
      "acceptance": "runtime_accepted",
      "materialization": {
        "status": "persisted",
        "message": {}
      }
    }
  ]
}
```

`acceptance` is nullable. `null` plus `materialization.status == "not_persisted"`
is the only authoritative result showing that Phoenix has neither accepted nor
materialized that ID. An accepted message may legitimately be
`not_persisted` while runtime delivery remains owed.

```swift
enum MessageMaterialization: Codable {
    case persisted(Message)
    case notPersisted

    private enum CodingKeys: String, CodingKey { case status, message }
    private enum Status: String, Codable { case persisted, notPersisted = "not_persisted" }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(Status.self, forKey: .status) {
        case .persisted:
            self = .persisted(try values.decode(Message.self, forKey: .message))
        case .notPersisted:
            self = .notPersisted
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .persisted(let message):
            try values.encode(Status.persisted, forKey: .status)
            try values.encode(message, forKey: .message)
        case .notPersisted:
            try values.encode(Status.notPersisted, forKey: .status)
        }
    }
}

struct MessageReconciliationEntry: Codable {
    let messageId: String
    let acceptance: MessageAcceptanceDisposition?
    let materialization: MessageMaterialization

    enum CodingKeys: String, CodingKey {
        case messageId = "message_id"
        case acceptance
        case materialization
    }
}
```

## Recovery algorithm

1. Persist the target conversation, generated `message_id`, and original request
   locally before submitting.
2. On a typed successful response, mark the local operation accepted. Treat
   `pending_runtime` and `runtime_accepted` as normal sent states;
   `queued_steering` may use a queued indicator. A `cancelled_steering` replay
   removes any optimistic queued item and must not resubmit or recreate the
   cancelled steer.
3. On timeout, disconnect, cancellation, backgrounding, or app termination, keep
   the same unresolved local operation. Do not generate a replacement ID.
4. On reconnect or relaunch, batch unresolved IDs through reconciliation.
5. If `acceptance` is non-null, Phoenix owns the operation. Wait for ordinary
   transcript synchronization; do not resubmit under a new ID.
6. If materialization is `persisted`, merge the authoritative message and remove
   the optimistic copy.
7. If acceptance is null and materialization is `not_persisted`, retry the
   original POST with the same ID and same content. A matching retry converges as
   `replayed`; changed content returns conflict.

Conversation SSE remains useful for live transcript updates, but it is not the
acceptance authority and is not required for cold-start recovery.

## Compatibility

This API intentionally replaces the former `queued`, `steering`, and
`already_persisted` response booleans. Decode the typed fields above atomically
with the server deployment. Coordinator output uses the same `request_result`
and `disposition` values under its accepted result.
