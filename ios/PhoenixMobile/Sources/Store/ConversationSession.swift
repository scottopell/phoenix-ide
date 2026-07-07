import Foundation
import Observation

/// Live model for one open conversation: cached snapshot + SSE reducer +
/// outbox. Owns the stream lifecycle (connect, reconnect with backoff,
/// resync via init snapshots) and drains the outbox whenever sending might
/// newly succeed (connectivity restored, stream reconnected, app foregrounded).
@MainActor
@Observable
final class ConversationSession {
    enum ConnectionState: Equatable {
        case idle
        case connecting
        case live
        /// Disconnected; next attempt at the associated time.
        case waitingToRetry(nextAttempt: Date)
        case offline
    }

    let conversationId: String
    let outbox: Outbox

    private(set) var conversation: Conversation?
    private(set) var messages: [Message] = []
    private(set) var agentWorking = false
    private(set) var presentationMode: String?
    private(set) var convState: JSONValue?
    private(set) var connection: ConnectionState = .idle
    /// In-flight LLM text accumulated from token events; cleared when the
    /// finalized message arrives or the turn ends.
    private(set) var streamingText = ""
    private(set) var lastErrorToast: String?
    /// tool_use_id -> the invoking block's tool name + input. Lets a tool
    /// result message (which carries only `tool_use_id`) find its native
    /// renderer. Rebuilt on message changes, not per render.
    private(set) var toolUseIndex: [String: ToolUseRef] = [:]

    private var lastSequenceId: Int64 = 0
    private var streamingRequestId: String?
    private var api: PhoenixAPI
    private let connectivity: ConnectivityMonitor
    private var connectivityToken: UUID?
    private var streamTask: Task<Void, Never>?
    private var drainTask: Task<Void, Never>?
    private var staleCheckTask: Task<Void, Never>?
    /// localIds with a POST in flight — prevents duplicate concurrent sends
    /// of one entry (resending a *different* entry is always safe).
    private var inFlight: Set<String> = []
    private var retryDelay: TimeInterval = 1

    private var snapshotName: String { "conv-\(conversationId)" }

    private struct Snapshot: Codable {
        var conversation: Conversation?
        var messages: [Message]
        var lastSequenceId: Int64
    }

    init(conversationId: String, api: PhoenixAPI, connectivity: ConnectivityMonitor) {
        self.conversationId = conversationId
        self.api = api
        self.connectivity = connectivity
        self.outbox = Outbox(conversationId: conversationId)

        // Cached snapshot renders immediately; the stream refreshes it.
        if let snap = DiskStore.load(Snapshot.self, name: snapshotName) {
            conversation = snap.conversation
            messages = snap.messages
            lastSequenceId = snap.lastSequenceId
            rebuildToolUseIndex()
        }
    }

    func start() {
        guard streamTask == nil else { return }
        connectivityToken = connectivity.addRestoreObserver { [weak self] in
            self?.connectivityRestored()
        }
        streamTask = Task { await streamLoop() }
        staleCheckTask = Task { await staleCheckLoop() }
    }

    func stop() {
        streamTask?.cancel()
        streamTask = nil
        drainTask?.cancel()
        drainTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        if let token = connectivityToken {
            connectivity.removeRestoreObserver(token)
            connectivityToken = nil
        }
        connection = .idle
        persistSnapshot()
    }

    /// Flush cached state at a navigation boundary. The session itself stays
    /// alive (AppModel owns it) so the outbox keeps draining off-screen.
    func persistOnNavigate() {
        persistSnapshot()
    }

    /// Called on scenePhase -> .active: the stream task was likely torn down
    /// while backgrounded; restart it and drain anything queued.
    func resyncAfterForeground() {
        if streamTask == nil {
            start()
        }
        drainOutbox()
    }

    private func connectivityRestored() {
        // Wake the stream loop out of its backoff sleep by restarting it.
        streamTask?.cancel()
        streamTask = Task { await streamLoop() }
        drainOutbox()
    }

    private func persistSnapshot() {
        DiskStore.save(
            Snapshot(
                conversation: conversation, messages: messages,
                lastSequenceId: lastSequenceId),
            name: snapshotName)
    }

    // MARK: - Sending

    /// Optimistic enqueue-then-send. The entry is persisted before the POST
    /// leaves the device; if the network is down the send is deferred, not
    /// failed.
    func send(text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        _ = outbox.enqueue(text: trimmed)
        drainOutbox()
    }

    func retryEntry(_ localId: String) {
        outbox.retry(localId)
        drainOutbox()
    }

    func dismissEntry(_ localId: String) {
        outbox.dismiss(localId)
    }

    /// Attempt delivery of every sendable entry, oldest first. Safe to call
    /// eagerly and repeatedly: entry-level `inFlight` guards duplicate
    /// concurrent POSTs, and the server's message_id idempotency makes
    /// genuine resends no-ops.
    func drainOutbox() {
        guard drainTask == nil else { return }
        drainTask = Task {
            defer { drainTask = nil }
            // Loop until no sendable entries remain, so a message enqueued
            // while a drain is already running is picked up by this pass
            // instead of waiting for the next trigger.
            while !Task.isCancelled {
                let sendable = outbox.entries.filter {
                    $0.status == .pending && !$0.acceptedByServer
                        && !inFlight.contains($0.localId)
                }
                guard let entry = sendable.first else { return }
                inFlight.insert(entry.localId)
                defer { inFlight.remove(entry.localId) }
                outbox.markAttempted(entry.localId)
                do {
                    let response = try await api.sendChat(
                        conversationId: conversationId,
                        text: entry.text,
                        images: entry.images,
                        messageId: entry.localId)
                    outbox.markAccepted(entry.localId, steering: response.steering ?? false)
                } catch let error as APIError where error.isTransport {
                    // Offline or unreachable: stay pending. The next drain
                    // trigger (connectivity restore, reconnect, foreground)
                    // retries automatically.
                    return
                } catch {
                    outbox.markFailed(
                        entry.localId,
                        error: (error as? APIError)?.errorDescription
                            ?? error.localizedDescription)
                }
            }
        }
    }

    func cancelAgent() {
        Task {
            do {
                _ = try await api.cancel(conversationId: conversationId)
            } catch {
                lastErrorToast = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
            }
        }
    }

    func clearErrorToast() {
        lastErrorToast = nil
    }

    // MARK: - Stream lifecycle

    private func streamLoop() async {
        retryDelay = 1
        while !Task.isCancelled {
            if !connectivity.isOnline {
                connection = .offline
                // No point burning retries with no path; the connectivity
                // observer restarts this loop the moment a path appears.
                // Meanwhile poll slowly in case the monitor is wrong.
                try? await Task.sleep(for: .seconds(30))
                continue
            }

            connection = .connecting
            do {
                let (bytes, _) = try await api.openStream(conversationId: conversationId)
                connection = .live
                retryDelay = 1
                // Every (re)connect is a full resync: init replaces the
                // snapshot and replays the server's pending-events ring.
                var parser = SSEParser()
                for try await byte in bytes {
                    if Task.isCancelled { return }
                    if let frame = parser.consume(byte),
                       let event = PhoenixEvent.decode(frame: frame) {
                        apply(event)
                    }
                }
                // Server closed the stream (e.g. broadcast lag): reconnect
                // promptly — the next init resyncs any missed state.
            } catch {
                if Task.isCancelled { return }
            }

            persistSnapshot()
            let jitter = Double.random(in: 0...0.3) * retryDelay
            connection = .waitingToRetry(nextAttempt: Date().addingTimeInterval(retryDelay + jitter))
            try? await Task.sleep(for: .seconds(retryDelay + jitter))
            retryDelay = min(retryDelay * 2, 30)
        }
    }

    private func staleCheckLoop() async {
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(20))
            if connection == .live {
                outbox.surfaceStaleAcceptedEntries()
            }
        }
    }

    // MARK: - Reducer

    private func apply(_ event: PhoenixEvent) {
        switch event {
        case .initSnapshot(let snap):
            conversation = snap.conversation
            convState = snap.conversation.state
            messages = snap.messages.sorted { $0.sequence_id < $1.sequence_id }
            agentWorking = snap.agentWorking
            presentationMode = snap.presentationMode
            streamingText = ""
            streamingRequestId = nil
            // Replay the ring through the same reducer so an in-flight turn
            // (streaming text, tool phase) survives the reconnect. The
            // replay floor is the ring anchor — ring entries sit in
            // (anchor, last_sequence_id], so anchoring at the tip would
            // silently drop the whole replay.
            lastSequenceId = snap.pendingAnchorSequenceId
            if !snap.pendingTruncated {
                for entry in snap.pendingEvents {
                    if let pending = PhoenixEvent.decode(pendingEntry: entry) {
                        applyLive(pending)
                    }
                }
            }
            lastSequenceId = max(lastSequenceId, snap.lastSequenceId)
            rebuildToolUseIndex()
            reconcileOutbox()
            drainOutbox()
            persistSnapshot()

        default:
            applyLive(event)
        }
    }

    /// Sequence-guarded application of non-init events. Events at or below
    /// the current floor were already absorbed via a snapshot — drop them.
    private func applyLive(_ event: PhoenixEvent) {
        switch event {
        case .initSnapshot:
            return  // handled by apply()

        case .message(let seq, let message):
            guard applyIfNewer(seq) else { return }
            upsert(message)
            if message.message_type == "agent" {
                streamingText = ""
                streamingRequestId = nil
                rebuildToolUseIndex()
            }
            reconcileOutbox()
            persistSnapshot()

        case .messageUpdated(let seq, let messageId, let content, let displayData):
            _ = applyIfNewer(seq)
            guard let idx = messages.firstIndex(where: { $0.message_id == messageId }) else {
                return  // update for an unknown target is a silent no-op
            }
            if let content, content != .null { messages[idx].content = content }
            if let displayData, displayData != .null { messages[idx].display_data = displayData }
            if messages[idx].message_type == "agent" { rebuildToolUseIndex() }
            persistSnapshot()

        case .stateChange(let seq, let state, let mode):
            guard applyIfNewer(seq) else { return }
            convState = state
            if let mode { presentationMode = mode }
            let type = state.stringValue ?? state["type"]?.stringValue
            // States where the agent is waiting on the user (or finished),
            // mirroring the ConversationState union in ui/src/api.ts.
            let restingStates: Set<String> = [
                "idle", "error", "terminal", "context_exhausted", "handed_off",
                "awaiting_user_response", "awaiting_task_approval",
                "awaiting_commission_review_approval", "awaiting_recovery",
            ]
            agentWorking = type.map { !restingStates.contains($0) } ?? false

        case .token(let seq, let text, let requestId):
            guard applyIfNewer(seq) else { return }
            if streamingRequestId != requestId {
                streamingRequestId = requestId
                streamingText = ""
            }
            streamingText += text

        case .agentDone(let seq):
            guard applyIfNewer(seq) else { return }
            streamingText = ""
            streamingRequestId = nil
            agentWorking = false
            // Turn boundary: steering-queued entries should now be in
            // history; also a natural moment to send anything pending.
            reconcileOutbox()
            drainOutbox()
            persistSnapshot()

        case .conversationUpdate(let seq, _):
            _ = applyIfNewer(seq)

        case .steerMessageQueued(let seq, let messageId):
            _ = applyIfNewer(seq)
            outbox.markAccepted(messageId, steering: true)

        case .errorEvent(let seq, let message):
            guard applyIfNewer(seq) else { return }
            lastErrorToast = message

        case .conversationBecameTerminal(let seq):
            _ = applyIfNewer(seq)

        case .other(_, let seq):
            if let seq { _ = applyIfNewer(seq) }
        }
    }

    private func applyIfNewer(_ seq: Int64) -> Bool {
        guard seq > lastSequenceId else { return false }
        lastSequenceId = seq
        return true
    }

    private func upsert(_ message: Message) {
        if let idx = messages.firstIndex(where: { $0.message_id == message.message_id }) {
            // Eager (in-flight) messages are later re-broadcast persisted
            // with the same message_id; the second arrival refreshes fields.
            messages[idx] = message
        } else {
            messages.append(message)
            messages.sort { $0.sequence_id < $1.sequence_id }
        }
    }

    private func reconcileOutbox() {
        outbox.reconcile(
            authoritativeMessageIds: Set(messages.map(\.message_id)))
    }

    private func rebuildToolUseIndex() {
        var index: [String: ToolUseRef] = [:]
        for message in messages where message.message_type == "agent" {
            guard let blocks = message.content.arrayValue else { continue }
            for block in blocks where block["type"]?.stringValue == "tool_use" {
                guard let id = block["id"]?.stringValue,
                      let name = block["name"]?.stringValue
                else { continue }
                index[id] = ToolUseRef(name: name, input: block["input"])
            }
        }
        toolUseIndex = index
    }
}

/// The identity of a tool invocation, joined from an agent message's
/// tool_use block to the tool result that answers it.
struct ToolUseRef: Equatable {
    var name: String
    var input: JSONValue?
}
