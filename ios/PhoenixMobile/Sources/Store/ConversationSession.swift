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
    /// Typed decode of convState, kept in lockstep by the reducer (and
    /// seeded from the cached conversation on cold open). Views render
    /// from this; the raw JSONValue exists only for persistence.
    private(set) var typedState: ConversationState = .unknown
    private(set) var connection: ConnectionState = .idle
    /// In-flight LLM text accumulated from token events; cleared when the
    /// finalized message arrives or the turn ends.
    private(set) var streamingText = ""
    private(set) var lastErrorToast: String?
    private(set) var isHardDeleted = false
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
    private let onHardDeleted: (String) -> Void

    private struct PendingMessagePatch {
        var content: JSONValue?
        var displayData: JSONValue?
    }

    /// `message_updated` can precede its eager `message` during replay.
    /// Retain the newest fields until the identity-bearing message arrives.
    private var pendingMessagePatches: [String: PendingMessagePatch] = [:]

    private var snapshotName: String { "conv-\(conversationId)" }

    /// Bump when Snapshot's persisted shape changes incompatibly (DiskStore
    /// versioning rule). Additive-optional fields (savedAt) need no bump.
    private static let snapshotSchemaVersion = 1

    private struct Snapshot: Codable {
        var conversation: Conversation?
        var messages: [Message]
        var lastSequenceId: Int64
        // Additive-optional: snapshots persisted before this field decode
        // as nil (age simply unknown — no note shown).
        var savedAt: Date?
    }

    /// When the cached snapshot was last written; drives the offline
    /// cache-age note (REQ-IOS-001).
    private(set) var snapshotSavedAt: Date?

    init(
        conversationId: String,
        api: PhoenixAPI,
        connectivity: ConnectivityMonitor,
        onHardDeleted: @escaping (String) -> Void = { _ in }
    ) {
        self.conversationId = conversationId
        self.api = api
        self.connectivity = connectivity
        self.onHardDeleted = onHardDeleted
        self.outbox = Outbox(conversationId: conversationId)

        // Cached snapshot renders immediately; the stream refreshes it.
        if let snap = DiskStore.loadVersioned(
            Snapshot.self, name: snapshotName, version: Self.snapshotSchemaVersion)
        {
            conversation = snap.conversation
            messages = snap.messages
            lastSequenceId = snap.lastSequenceId
            typedState = ConversationState.parse(snap.conversation?.state)
            presentationMode = snap.conversation?.presentation_mode
            // Busy flag follows the cached mode the same way live
            // state_change events derive it — a snapshot taken mid-turn
            // must not open looking idle.
            agentWorking = presentationMode == "working"
            snapshotSavedAt = snap.savedAt
            rebuildToolUseIndex()
            // A crash between persistSnapshot and the outbox prune leaves
            // both files claiming the same message — reconciling here keeps
            // the union-without-duplicates rule on cold offline opens.
            reconcileOutbox()
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
        guard !isHardDeleted else { return }
        let now = Date()
        snapshotSavedAt = now
        DiskStore.saveVersioned(
            Snapshot(
                conversation: conversation, messages: messages,
                lastSequenceId: lastSequenceId, savedAt: now),
            name: snapshotName, version: Self.snapshotSchemaVersion)
    }

    // MARK: - Sending

    /// Optimistic enqueue-then-send. The entry is persisted before the POST
    /// leaves the device; if the network is down the send is deferred, not
    /// failed. Images ride the same outbox path as text — same durability,
    /// same idempotent delivery.
    func send(text: String, images: [ImagePayload] = []) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (!trimmed.isEmpty || !images.isEmpty),
              !isHardDeleted,
              typedState.acceptsChatMessage
        else { return }
        _ = outbox.enqueue(text: trimmed, images: images)
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
        guard drainTask == nil, !isHardDeleted else { return }
        drainTask = Task {
            defer { drainTask = nil }
            // Loop until no sendable entries remain, so a message enqueued
            // while a drain is already running is picked up by this pass
            // instead of waiting for the next trigger.
            while !Task.isCancelled {
                // Never POST an entry whose durable copy is missing. This
                // retries the persistence point on every delivery trigger.
                guard outbox.prepareForDelivery() else { return }
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
                    // A completion racing stop() (cache clear, sign-out)
                    // must not mutate — and re-persist — the outbox after
                    // its files were deleted.
                    guard !Task.isCancelled else { return }
                    outbox.markAccepted(entry.localId, steering: response.steering ?? false)
                } catch let error as APIError where error.isTransport {
                    // Offline or unreachable: stay pending. The next drain
                    // trigger (connectivity restore, reconnect, foreground)
                    // retries automatically.
                    return
                } catch {
                    guard !Task.isCancelled else { return }
                    outbox.markFailed(
                        entry.localId,
                        error: (error as? APIError)?.errorDescription
                            ?? error.localizedDescription)
                }
            }
        }
    }

    /// The action currently being executed, or nil. Views use this to
    /// disable controls and show progress — approval buttons especially
    /// must not double-fire.
    private(set) var actionInFlight: ConversationAction?

    /// Execute a session-scoped action per its declared delivery policy
    /// (ConversationAction). Online-only actions fail fast with a toast
    /// when offline — deliberately not queued, see the policy doc.
    func perform(_ action: ConversationAction) {
        guard actionInFlight == nil else { return }
        switch action.policy {
        case .onlineOnly:
            guard connectivity.isOnline else {
                lastErrorToast = "This action needs a connection — it can't be queued."
                return
            }
        case .outboxed:
            break  // never blocked on connectivity by definition
        }
        actionInFlight = action
        Task {
            defer { actionInFlight = nil }
            do {
                switch action {
                case .cancel:
                    _ = try await api.cancel(conversationId: conversationId)
                case .dismissError:
                    try await api.dismissError(conversationId: conversationId)
                case .approveTask:
                    try await api.approveTask(conversationId: conversationId)
                case .rejectTask:
                    try await api.rejectTask(conversationId: conversationId)
                case .provideTaskFeedback(let annotations):
                    try await api.sendTaskFeedback(
                        conversationId: conversationId, annotations: annotations)
                case .respondToQuestions(let answers):
                    try await api.respondToQuestion(
                        conversationId: conversationId, answers: answers)
                case .dismissQuestion:
                    try await api.dismissQuestion(conversationId: conversationId)
                }
                // Success needs no local state change: the server emits the
                // resulting state_change over SSE and the reducer applies it.
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
                        receive(event)
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

    func receive(_ event: PhoenixEvent) {
        guard !isHardDeleted else { return }
        switch event {
        case .initSnapshot(let snap):
            conversation = snap.conversation
            convState = snap.conversation.state
            typedState = ConversationState.parse(snap.conversation.state)
            messages = snap.messages.sorted { $0.sequence_id < $1.sequence_id }
            agentWorking = snap.agentWorking
            presentationMode = snap.presentationMode
            // Init carries presentation_mode as a top-level field, not on
            // the conversation record — fold it in so the persisted
            // snapshot preserves it for offline cold opens (the
            // needs-action gating reads it).
            if let mode = snap.presentationMode {
                conversation?.presentation_mode = mode
            }
            streamingText = ""
            streamingRequestId = nil
            pendingMessagePatches.removeAll()
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
            // Persist the authoritative snapshot BEFORE reconciling: the
            // outbox prune must never become durable while the message
            // snapshot that justifies it is still memory-only — a crash
            // between the two writes would lose the user's text from both.
            persistSnapshot()
            reconcileOutbox()
            drainOutbox()

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
            // Snapshot before outbox prune — see the init branch.
            persistSnapshot()
            reconcileOutbox()

        case .messageUpdated(let seq, let messageId, let content, let displayData):
            // Stale guard applies here too: a replayed update from before
            // the floor must not clobber content a newer update already set.
            guard applyIfNewer(seq) else { return }
            guard let idx = messages.firstIndex(where: { $0.message_id == messageId }) else {
                var patch = pendingMessagePatches[messageId]
                    ?? PendingMessagePatch(content: nil, displayData: nil)
                if let content, content != .null { patch.content = content }
                if let displayData, displayData != .null { patch.displayData = displayData }
                pendingMessagePatches[messageId] = patch
                return
            }
            if let content, content != .null { messages[idx].content = content }
            if let displayData, displayData != .null { messages[idx].display_data = displayData }
            if messages[idx].message_type == "agent" { rebuildToolUseIndex() }
            persistSnapshot()

        case .stateChange(let seq, let state, let mode):
            guard applyIfNewer(seq) else { return }
            convState = state
            typedState = ConversationState.parse(state)
            if let mode { presentationMode = mode }
            // Fold into the persisted conversation too: the offline view
            // is parsed from conversation.state on cold open, so a live
            // needs-action/error transition must survive a restart.
            conversation?.state = state
            if let mode { conversation?.presentation_mode = mode }
            persistSnapshot()
            if let mode {
                // The server's presentation_mode (idle | working |
                // needs_action | error | done) is authoritative and covers
                // state variants this client predates.
                agentWorking = mode == "working"
            } else {
                // Fallback: states where the agent is waiting on the user
                // (or finished), mirroring ConversationState in ui/src/api.ts.
                let type = state.stringValue ?? state["type"]?.stringValue
                let restingStates: Set<String> = [
                    "idle", "error", "terminal", "context_exhausted", "handed_off",
                    "awaiting_user_response", "awaiting_task_approval",
                    "awaiting_commission_review_approval", "awaiting_recovery",
                ]
                agentWorking = type.map { !restingStates.contains($0) } ?? false
            }

        case .token(let seq, let text, let requestId):
            guard applyIfNewer(seq) else { return }
            // A late/replayed token after the turn closed would recreate a
            // ghost bubble below the finalized message — only accumulate
            // while a turn is actually running.
            guard agentWorking else { return }
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
            // agent_done can close a turn without a trailing idle
            // state_change; leave resting/needs-action states alone but
            // clear in-flight ones so the spinner doesn't outlive the turn.
            if presentationMode == "working"
                || (presentationMode == nil && typedState.isKnownWorkingState)
            {
                typedState = .idle
                convState = .string("idle")
                conversation?.state = .string("idle")
                // The mode must move with the state, or the snapshot
                // persists idle-with-working-mode and a cold reopen seeds
                // the spinner back for a turn that already ended.
                presentationMode = "idle"
                conversation?.presentation_mode = "idle"
            }
            // Turn boundary: steering-queued entries should now be in
            // history; also a natural moment to send anything pending.
            // Snapshot first — same ordering rule as the message branch.
            persistSnapshot()
            reconcileOutbox()
            drainOutbox()

        case .conversationUpdate(let seq, let update):
            guard applyIfNewer(seq) else { return }
            // Shallow-merge the partial metadata payload (cwd, branch,
            // title, mode label after e.g. task approval) onto the local
            // conversation — this event exists precisely so clients don't
            // need a reconnect to see it.
            if var conv = conversation {
                if let v = update["cwd"]?.stringValue { conv.cwd = v }
                if let v = update["branch_name"]?.stringValue { conv.branch_name = v }
                if let v = update["task_title"]?.stringValue { conv.task_title = v }
                if let v = update["conv_mode_label"]?.stringValue { conv.conv_mode_label = v }
                if let v = update["slug"]?.stringValue { conv.slug = v }
                conversation = conv
                persistSnapshot()
            }

        case .steerMessageQueued(let seq, let messageId):
            guard applyIfNewer(seq) else { return }
            outbox.markAccepted(messageId, steering: true)

        case .errorEvent(let seq, let message):
            guard applyIfNewer(seq) else { return }
            lastErrorToast = message

        case .conversationBecameTerminal(let seq):
            _ = applyIfNewer(seq)

        case .conversationHardDeleted(let seq, let deletedConversationId):
            guard deletedConversationId == conversationId, applyIfNewer(seq) else { return }
            drainTask?.cancel()
            drainTask = nil
            inFlight.removeAll()
            isHardDeleted = true
            conversation = nil
            messages = []
            convState = nil
            typedState = .terminal
            presentationMode = "done"
            agentWorking = false
            streamingText = ""
            streamingRequestId = nil
            pendingMessagePatches.removeAll()
            toolUseIndex = [:]
            DiskStore.remove(name: snapshotName)
            outbox.clear()
            onHardDeleted(conversationId)

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
        var message = message
        if let patch = pendingMessagePatches.removeValue(forKey: message.message_id) {
            if let content = patch.content { message.content = content }
            if let displayData = patch.displayData { message.display_data = displayData }
        }
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
