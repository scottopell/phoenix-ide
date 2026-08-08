import Foundation

/// One server-sent event frame: the `event:` name plus the joined `data:`
/// payload. Comment lines (`:` prefix — the server's keep-alives) never
/// surface as frames.
struct SSEFrame: Sendable {
    var event: String
    var data: String
}

/// Incremental SSE frame parser. Byte-based rather than line-convenience
/// APIs because frame boundaries are *empty* lines, which line sequences
/// tend to swallow.
struct SSEParser: Sendable {
    private var lineBuffer: [UInt8] = []
    private var eventName = ""
    private var dataLines: [String] = []

    /// Feed one byte; returns a completed frame when a blank line closes one.
    mutating func consume(_ byte: UInt8) -> SSEFrame? {
        if byte == 0x0A {  // \n
            let line = String(decoding: lineBuffer, as: UTF8.self)
            lineBuffer.removeAll(keepingCapacity: true)
            return consumeLine(line)
        }
        if byte != 0x0D {  // strip \r
            lineBuffer.append(byte)
        }
        return nil
    }

    private mutating func consumeLine(_ line: String) -> SSEFrame? {
        if line.isEmpty {
            // Blank line: dispatch the accumulated frame, if any.
            guard !dataLines.isEmpty || !eventName.isEmpty else { return nil }
            let frame = SSEFrame(
                event: eventName.isEmpty ? "message" : eventName,
                data: dataLines.joined(separator: "\n"))
            eventName = ""
            dataLines = []
            return frame
        }
        if line.hasPrefix(":") { return nil }  // comment / keep-alive

        let field: Substring
        let value: Substring
        if let colon = line.firstIndex(of: ":") {
            field = line[line.startIndex..<colon]
            var v = line[line.index(after: colon)...]
            if v.hasPrefix(" ") { v = v.dropFirst() }
            value = v
        } else {
            field = line[...]
            value = ""
        }

        switch field {
        case "event": eventName = String(value)
        case "data": dataLines.append(String(value))
        default: break  // id/retry unused
        }
        return nil
    }
}

// MARK: - Decoded Phoenix events

/// The subset of the Phoenix SSE wire protocol this client consumes.
/// (See specs/sse_wire/sse_wire.allium for the full server contract.)
/// Events the app doesn't render map to `.other` so the reducer can still
/// advance its sequence floor.
enum PhoenixEvent: Sendable {
    case initSnapshot(InitSnapshot)
    case message(seq: Int64, message: Message)
    case messageUpdated(
        seq: Int64, messageId: String, content: JSONValue?, displayData: JSONValue?,
        durationMs: Double?, transcriptGeneration: Int64?)
    case stateChange(seq: Int64, state: JSONValue, presentationMode: String?)
    case token(seq: Int64, text: String, requestId: String)
    case agentDone(seq: Int64)
    case conversationUpdate(seq: Int64, conversation: JSONValue)
    case steerMessageQueued(seq: Int64, messageId: String)
    case errorEvent(seq: Int64, message: String)
    case conversationBecameTerminal(seq: Int64)
    case other(type: String, seq: Int64?)

    struct InitSnapshot: Sendable {
        enum TranscriptCoverage: String, Sendable {
            case complete
            case tail
            case preserve
        }

        var conversation: Conversation
        var messages: [Message]
        var agentWorking: Bool
        var presentationMode: String?
        var lastSequenceId: Int64
        /// Sequence floor for replaying `pendingEvents`: the seq of the last
        /// persisted message at subscribe time. Ring entries are strictly
        /// above this and at or below `lastSequenceId`.
        var pendingAnchorSequenceId: Int64
        /// Replay-ring contents (each entry is a full wire event JSON with a
        /// `type` discriminator). Replayed through the normal reducer after
        /// the snapshot lands so a reconnect mid-turn keeps the in-flight view.
        var pendingEvents: [JSONValue]
        var pendingTruncated: Bool
        var transcriptGeneration: Int64
        var transcriptCoverage: TranscriptCoverage
    }

    /// Decode from an SSE frame. `frame.event` carries the type name; the
    /// data payload is the tagged wire JSON.
    static func decode(frame: SSEFrame) -> PhoenixEvent? {
        guard let data = frame.data.data(using: .utf8),
              let json = try? JSONDecoder().decode(JSONValue.self, from: data)
        else { return nil }
        return decode(type: frame.event, json: json, rawData: data)
    }

    /// Decode a replay-ring entry (init `pending_events`), whose type lives
    /// in the payload's `type` field.
    static func decode(pendingEntry: JSONValue) -> PhoenixEvent? {
        guard let type = pendingEntry["type"]?.stringValue,
              let data = try? JSONEncoder().encode(pendingEntry)
        else { return nil }
        return decode(type: type, json: pendingEntry, rawData: data)
    }

    private static func decode(type: String, json: JSONValue, rawData: Data) -> PhoenixEvent? {
        let seq = json["sequence_id"]?.numberValue.map { Int64($0) } ?? 0

        switch type {
        case "init":
            struct InitWire: Codable {
                var conversation: Conversation
                var messages: [Message]
                var agent_working: Bool?
                var presentation_mode: String?
                var last_sequence_id: Int64?
                var pending_anchor_sequence_id: Int64?
                var pending_events: [JSONValue]?
                var pending_truncated: Bool?
                var transcript_generation: Int64?
                var transcript_coverage: String?
            }
            guard let wire = try? JSONDecoder().decode(InitWire.self, from: rawData) else {
                return nil
            }
            let lastSeq = wire.last_sequence_id ?? seq
            return .initSnapshot(
                InitSnapshot(
                    conversation: wire.conversation,
                    messages: wire.messages,
                    agentWorking: wire.agent_working ?? false,
                    presentationMode: wire.presentation_mode,
                    lastSequenceId: lastSeq,
                    pendingAnchorSequenceId: wire.pending_anchor_sequence_id ?? lastSeq,
                    pendingEvents: wire.pending_events ?? [],
                    pendingTruncated: wire.pending_truncated ?? false,
                    transcriptGeneration: wire.transcript_generation
                        ?? wire.conversation.transcript_generation ?? 1,
                    transcriptCoverage: InitSnapshot.TranscriptCoverage(
                        rawValue: wire.transcript_coverage ?? "complete") ?? .complete))

        case "message":
            struct MessageWire: Codable {
                var sequence_id: Int64
                var message: Message
            }
            guard let wire = try? JSONDecoder().decode(MessageWire.self, from: rawData) else {
                return nil
            }
            return .message(seq: wire.sequence_id, message: wire.message)

        case "message_updated":
            guard let messageId = json["message_id"]?.stringValue else { return nil }
            return .messageUpdated(
                seq: seq, messageId: messageId,
                content: json["content"], displayData: json["display_data"],
                durationMs: json["duration_ms"]?.numberValue,
                transcriptGeneration: json["transcript_generation"]?.numberValue.map(Int64.init))

        case "state_change":
            return .stateChange(
                seq: seq,
                state: json["state"] ?? .null,
                presentationMode: json["presentation_mode"]?.stringValue)

        case "token":
            guard let text = json["text"]?.stringValue,
                  let requestId = json["request_id"]?.stringValue
            else { return nil }
            return .token(seq: seq, text: text, requestId: requestId)

        case "agent_done":
            return .agentDone(seq: seq)

        case "conversation_update":
            return .conversationUpdate(seq: seq, conversation: json["conversation"] ?? .null)

        case "steer_message_queued":
            guard let messageId = json["message_id"]?.stringValue else { return nil }
            return .steerMessageQueued(seq: seq, messageId: messageId)

        case "error":
            return .errorEvent(
                seq: seq, message: json["message"]?.stringValue ?? "Unknown error")

        case "conversation_became_terminal":
            return .conversationBecameTerminal(seq: seq)

        default:
            return .other(type: type, seq: seq)
        }
    }
}
