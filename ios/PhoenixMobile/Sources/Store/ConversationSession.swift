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
    private var onConversationUpdate: ((Conversation) -> Void)?
    private var onHardDeleted: (String) -> Void
    private var viewIsActive = false
    private var replayFromPendingAnchor = false
    private var streamBlockedUntilConfigurationChange = false
    /// Init's persisted-message anchor. Live messages above it may be eager
    /// assistant output, so snapshot persistence excludes them until resync.
    private var durableMessageSequenceCeiling: Int64 = 0

    private var snapshotName: String { "conv-\(conversationId)" }

    private struct Snapshot: Codable {
        var conversation: Conversation?
        var messages: [Message]
        var lastSequenceId: Int64
        /// Missing only in snapshots written before transcript generations
        /// were part of the iOS cache; nil forces replacement on next init.
        var transcriptGeneration: Int64?
        /// Missing in snapshots written before cache freshness was tracked.
        var syncedAt: Date?
    }

    private var transcriptGeneration: Int64?
    private(set) var snapshotSyncedAt: Date?

    init(
        conversationId: String,
        api: PhoenixAPI,
        connectivity: ConnectivityMonitor,
        onConversationUpdate: ((Conversation) -> Void)? = nil,
        onHardDeleted: @escaping (String) -> Void = { _ in }
    ) {
        self.conversationId = conversationId
        self.api = api
        self.connectivity = connectivity
        self.onConversationUpdate = onConversationUpdate
        self.onHardDeleted = onHardDeleted
        self.outbox = Outbox(conversationId: conversationId)

        // Cached snapshot renders immediately; the stream refreshes it.
        if let snap = DiskStore.load(Snapshot.self, name: snapshotName) {
            conversation = snap.conversation
            messages = snap.messages
            durableMessageSequenceCeiling = snap.messages.map(\.sequence_id).max() ?? 0
            lastSequenceId = snap.lastSequenceId
            transcriptGeneration = snap.transcriptGeneration
            snapshotSyncedAt = snap.syncedAt
            replayFromPendingAnchor = true
            typedState = ConversationState.parse(snap.conversation?.state)
            presentationMode = snap.conversation?.presentation_mode
            // Busy flag follows the cached mode the same way live
            // state_change events derive it — a snapshot taken mid-turn
            // must not open looking idle.
            agentWorking = presentationMode == "working"
            rebuildToolUseIndex()
            // A prior crash can leave the authoritative snapshot durable but
            // the matching outbox row not yet pruned. Reconcile at load so the
            // same user message never renders twice while offline.
            reconcileOutbox()
        }
    }

    func start() {
        guard !isHardDeleted else { return }
        viewIsActive = true
        if connectivityToken == nil {
            connectivityToken = connectivity.addPathObserver(
                onRestore: { [weak self] in self?.connectivityRestored() },
                onLoss: { [weak self] in self?.connectivityLost() })
        }
        resumeLiveTasks()
    }

    func replaceAPI(_ api: PhoenixAPI) {
        self.api = api
        streamBlockedUntilConfigurationChange = false
        if viewIsActive {
            streamTask?.cancel()
            streamTask = nil
            connection = .idle
            resumeLiveTasks()
        }
        drainOutbox()
    }

    func adoptOpenOwnership(
        onConversationUpdate: @escaping (Conversation) -> Void,
        onHardDeleted: @escaping (String) -> Void
    ) {
        self.onConversationUpdate = onConversationUpdate
        self.onHardDeleted = onHardDeleted
    }

    func stop() {
        viewIsActive = false
        pauseLiveTasks()
        drainTask?.cancel()
        drainTask = nil
        if let token = connectivityToken {
            connectivity.removePathObserver(token)
            connectivityToken = nil
        }
    }

    /// End the opened-view stream while retaining this session as the owner
    /// of its disk-backed outbox.
    func closeView() {
        viewIsActive = false
        pauseLiveTasks()
    }

    /// Background suspension preserves whether the view is open so a later
    /// foreground transition resumes only that conversation's live stream.
    func pauseForBackground() {
        pauseLiveTasks()
    }

    private func pauseLiveTasks() {
        streamTask?.cancel()
        streamTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        connection = .idle
        persistSnapshot()
    }

    private func resumeLiveTasks() {
        guard viewIsActive, !streamBlockedUntilConfigurationChange else { return }
        if streamTask == nil {
            streamTask = Task { await streamLoop() }
        }
        if staleCheckTask == nil {
            staleCheckTask = Task { await staleCheckLoop() }
        }
    }

    /// Called on scenePhase -> .active: the stream task was likely torn down
    /// while backgrounded; restart it and drain anything queued.
    func resyncAfterForeground() {
        resumeLiveTasks()
        drainOutbox()
    }

    private func connectivityRestored() {
        if viewIsActive, !streamBlockedUntilConfigurationChange {
            // Wake the stream loop out of its backoff sleep by restarting it.
            streamTask?.cancel()
            streamTask = Task { await streamLoop() }
        }
        drainOutbox()
    }

    private func connectivityLost() {
        streamTask?.cancel()
        streamTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        connection = .offline
    }

    @discardableResult
    private func persistSnapshot(authoritative: Bool = false) -> Bool {
        guard !isHardDeleted else { return false }
        let syncedAt = authoritative ? Date() : snapshotSyncedAt
        let didSave = DiskStore.save(
            Snapshot(
                conversation: conversation,
                messages: Self.durableMessages(
                    messages, through: durableMessageSequenceCeiling),
                lastSequenceId: lastSequenceId,
                transcriptGeneration: transcriptGeneration,
                syncedAt: syncedAt),
            name: snapshotName)
        if didSave, authoritative {
            snapshotSyncedAt = syncedAt
        }
        return didSave
    }

    // MARK: - Sending

    /// Optimistic enqueue-then-send. The entry is persisted before the POST
    /// leaves the device; if the network is down the send is deferred, not
    /// failed.
    @discardableResult
    func send(text: String) -> Bool {
        guard !isHardDeleted else { return false }
        guard ClientOperation.chat.policy == .outboxed else { return false }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, typedState.acceptsChatMessage else { return false }
        guard outbox.enqueue(text: trimmed) != nil else {
            lastErrorToast = "Message could not be saved on this device. Free storage and try again."
            return false
        }
        drainOutbox()
        return true
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
        guard !isHardDeleted else { return }
        guard drainTask == nil else { return }
        drainTask = Task {
            defer { drainTask = nil }
            // Loop until no sendable entries remain, so a message enqueued
            // while a drain is already running is picked up by this pass
            // instead of waiting for the next trigger.
            while !Task.isCancelled {
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
                    if response.already_persisted == true {
                        await reconcileAlreadyPersisted(entry.localId)
                    }
                } catch let error as APIError where error.isRetryableChatDeliveryFailure {
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
        switch ClientOperation.conversationAction(action).policy {
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
            do {
                switch action {
                case .cancel:
                    _ = try await api.cancel(conversationId: conversationId)
                case .dismissError:
                    try await api.dismissError(conversationId: conversationId)
                case .approveTask(let handoff):
                    try await api.approveTask(
                        conversationId: conversationId, handoff: handoff)
                case .rejectTask:
                    try await api.rejectTask(conversationId: conversationId)
                case .provideTaskFeedback(let annotations):
                    try await api.sendTaskFeedback(
                        conversationId: conversationId, annotations: annotations)
                }
                // Success needs no local state change: the server emits the
                // resulting state_change over SSE and the reducer applies it.
                if !action.waitsForAuthoritativeStateChange {
                    actionInFlight = nil
                }
            } catch {
                actionInFlight = nil
                lastErrorToast = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
            }
        }
    }

    func clearErrorToast() {
        lastErrorToast = nil
    }

    // MARK: - Stream lifecycle

    /// URLSession.AsyncBytes is intentionally parsed in a detached producer.
    /// ConversationSession is MainActor-isolated, so iterating and JSON-
    /// decoding a multi-megabyte init here directly would freeze input and
    /// scrolling. Only decoded events cross back to the reducer.
    private nonisolated static func decodedEvents(
        from bytes: URLSession.AsyncBytes
    ) -> AsyncThrowingStream<PhoenixEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task.detached(priority: .utility) {
                do {
                    var parser = SSEParser()
                    for try await byte in bytes {
                        if Task.isCancelled { break }
                        if let frame = parser.consume(byte),
                           let event = PhoenixEvent.decode(frame: frame) {
                            continuation.yield(event)
                        }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }

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
                for try await event in Self.decodedEvents(from: bytes) {
                    if Task.isCancelled { return }
                    apply(event)
                }
                // Server closed the stream (e.g. broadcast lag): reconnect
                // promptly — the next init resyncs any missed state.
            } catch let error as APIError {
                if Task.isCancelled { return }
                if case .certificatePinMismatch = error {
                    lastErrorToast = error.errorDescription
                    connection = .idle
                    return
                }
                if error.isNotFound {
                    handleHardDeletion()
                    return
                }
                if error.isPermanentStreamAuthenticationFailure {
                    streamBlockedUntilConfigurationChange = true
                    lastErrorToast = error.errorDescription
                    connection = .idle
                    return
                }
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
        guard !isHardDeleted else { return }
        switch event {
        case .initSnapshot(let snap):
            retryDelay = 1
            let previousSequenceFloor = lastSequenceId
            let generationMatches = transcriptGeneration == snap.transcriptGeneration
            let mustReplayFromAnchor = replayFromPendingAnchor
            replayFromPendingAnchor = false
            conversation = snap.conversation
            conversation?.transcript_generation = snap.transcriptGeneration
            conversation?.presentation_mode = snap.presentationMode
            if let mode = snap.presentationMode {
                conversation?.requires_action = mode == "needs_action"
            }
            convState = snap.conversation.state
            typedState = ConversationState.parse(snap.conversation.state)
            clearResolvedActionIfStateAdvanced()
            messages = Self.reconcileTranscript(
                existing: messages,
                incoming: snap.messages,
                coverage: snap.transcriptCoverage,
                generationMatches: generationMatches)
            transcriptGeneration = snap.transcriptGeneration
            durableMessageSequenceCeiling = Self.durableCeilingAfterInit(
                anchor: snap.pendingAnchorSequenceId,
                messages: snap.messages)
            agentWorking = snap.agentWorking
            presentationMode = snap.presentationMode
            if mustReplayFromAnchor || previousSequenceFloor == 0 || !generationMatches
                || !snap.agentWorking || snap.pendingTruncated {
                streamingText = ""
                streamingRequestId = nil
            }
            // Replay the ring through the same reducer so an in-flight turn
            // (streaming text, tool phase) survives the reconnect. The
            // replay floor is the ring anchor — ring entries sit in
            // (anchor, last_sequence_id], so anchoring at the tip would
            // silently drop the whole replay.
            lastSequenceId = Self.replayFloor(
                previous: previousSequenceFloor,
                anchor: snap.pendingAnchorSequenceId,
                serverTip: snap.lastSequenceId,
                generationMatches: generationMatches,
                restoredFromDisk: mustReplayFromAnchor)
            if !snap.pendingTruncated {
                for entry in snap.pendingEvents {
                    if let pending = PhoenixEvent.decode(pendingEntry: entry) {
                        applyLive(pending)
                    }
                }
            }
            lastSequenceId = max(lastSequenceId, snap.lastSequenceId)
            rebuildToolUseIndex()
            if let conversation {
                onConversationUpdate?(conversation)
            }
            // Persist the authoritative snapshot BEFORE reconciling: the
            // outbox prune must never become durable while the message
            // snapshot that justifies it is still memory-only — a crash
            // between the two writes would lose the user's text from both.
            outbox.suppress(authoritativeMessageIds: Set(snap.messages.map(\.message_id)))
            if persistSnapshot(authoritative: true) {
                reconcileOutbox()
            }
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
            durableMessageSequenceCeiling = Self.durableCeilingAfterLiveMessage(
                current: durableMessageSequenceCeiling,
                message: message)
            outbox.suppress(authoritativeMessageIds: [message.message_id])
            if let createdAt = message.created_at,
               let messageDate = message.createdAtDate,
               var conversation,
               conversation.updatedAtDate.map({ messageDate > $0 }) ?? true {
                conversation.updated_at = createdAt
                self.conversation = conversation
                onConversationUpdate?(conversation)
            }
            if message.message_type == "agent" {
                streamingText = ""
                streamingRequestId = nil
                rebuildToolUseIndex()
            }
            // Snapshot before outbox prune — see the init branch.
            if persistSnapshot(authoritative: true) {
                reconcileOutbox()
            }

        case .messageUpdated(
            let seq, let messageId, let content, let displayData, let durationMs,
            let updatedGeneration):
            // Stale guard applies here too: a replayed update from before
            // the floor must not clobber content a newer update already set.
            guard applyIfNewer(seq) else { return }
            guard let idx = messages.firstIndex(where: { $0.message_id == messageId }) else {
                return  // update for an unknown target is a silent no-op
            }
            if let content, content != .null { messages[idx].content = content }
            if let displayData, displayData != .null {
                messages[idx].display_data = Self.mergeDisplayData(
                    existing: messages[idx].display_data,
                    patch: displayData)
            }
            if let durationMs {
                messages[idx].display_data = Self.mergeDisplayData(
                    existing: messages[idx].display_data,
                    patch: .object(["duration_ms": .number(durationMs)]))
            }
            if let updatedGeneration {
                transcriptGeneration = updatedGeneration
                conversation?.transcript_generation = updatedGeneration
            }
            if messages[idx].message_type == "agent" { rebuildToolUseIndex() }
            persistSnapshot(authoritative: true)

        case .stateChange(let seq, let state, let mode, let stateUpdatedAt):
            guard applyIfNewer(seq) else { return }
            convState = state
            typedState = ConversationState.parse(state)
            clearResolvedActionIfStateAdvanced()
            if let mode { presentationMode = mode }
            if var conversation {
                conversation.state = state
                if let stateUpdatedAt { conversation.state_updated_at = stateUpdatedAt }
                if let mode {
                    conversation.presentation_mode = mode
                    conversation.requires_action = mode == "needs_action"
                }
                self.conversation = conversation
                snapshotSyncedAt = Date()
                persistSnapshot(authoritative: true)
                onConversationUpdate?(conversation)
            }
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
            // agent_done follows the turn's final commit, so all transcript
            // rows observed before this boundary are durable.
            durableMessageSequenceCeiling = max(durableMessageSequenceCeiling, seq)
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
            if persistSnapshot(authoritative: true) {
                reconcileOutbox()
            }
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
                if let v = update["title"]?.stringValue { conv.title = v }
                if let v = update["updated_at"]?.stringValue { conv.updated_at = v }
                conversation = conv
                persistSnapshot(authoritative: true)
                onConversationUpdate?(conv)
            }

        case .steerMessageQueued(let seq, let messageId):
            _ = applyIfNewer(seq)
            outbox.markAccepted(messageId, steering: true)

        case .errorEvent(let seq, let message):
            guard applyIfNewer(seq) else { return }
            lastErrorToast = message

        case .conversationBecameTerminal(let seq):
            _ = applyIfNewer(seq)

        case .conversationHardDeleted(let seq, let deletedConversationId):
            guard deletedConversationId == conversationId, applyIfNewer(seq) else { return }
            handleHardDeletion()

        case .other(_, let seq):
            if let seq { _ = applyIfNewer(seq) }
        }
    }

    private func handleHardDeletion() {
        streamTask?.cancel()
        streamTask = nil
        drainTask?.cancel()
        drainTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        if let token = connectivityToken {
            connectivity.removePathObserver(token)
            connectivityToken = nil
        }
        inFlight.removeAll()
        isHardDeleted = true
        conversation = nil
        messages = []
        durableMessageSequenceCeiling = 0
        convState = nil
        presentationMode = "done"
        agentWorking = false
        streamingText = ""
        streamingRequestId = nil
        toolUseIndex = [:]
        connection = .idle
        DiskStore.remove(name: snapshotName)
        outbox.clear()
        onHardDeleted(conversationId)
    }

    private func applyIfNewer(_ seq: Int64) -> Bool {
        guard seq > lastSequenceId else { return false }
        lastSequenceId = seq
        return true
    }

    nonisolated static func mergeDisplayData(existing: JSONValue?, patch: JSONValue) -> JSONValue {
        guard case .object(var merged) = existing,
              case .object(let patchObject) = patch
        else { return patch }

        for (key, value) in patchObject {
            if key == "tool_starts",
               case .object(var starts) = merged[key],
               case .object(let newStarts) = value {
                starts.merge(newStarts) { _, latest in latest }
                merged[key] = .object(starts)
            } else {
                merged[key] = value
            }
        }
        return .object(merged)
    }

    private func clearResolvedActionIfStateAdvanced() {
        guard let action = actionInFlight, action.waitsForAuthoritativeStateChange else {
            return
        }
        if case .awaitingTaskApproval = typedState { return }
        actionInFlight = nil
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

    private func reconcileAlreadyPersisted(_ localId: String) async {
        do {
            let response = try await api.reconcileAcceptedMessages(
                conversationId: conversationId, messageIds: [localId])
            guard !Task.isCancelled,
                  let result = response.entries.first(where: { $0.message_id == localId })
            else { return }
            switch result.status {
            case .persisted:
                guard let message = result.message else { return }
                upsert(message)
                durableMessageSequenceCeiling = max(
                    durableMessageSequenceCeiling, message.sequence_id)
                lastSequenceId = max(lastSequenceId, message.sequence_id)
                rebuildToolUseIndex()
                if persistSnapshot(authoritative: true) {
                    reconcileOutbox()
                }
            case .steeringQueued:
                outbox.markAccepted(localId, steering: true)
            case .absent:
                // The POST response and exact reconciliation disagree. Keep
                // the accepted outbox row visible and force the live stream
                // through a fresh authoritative init rather than guessing.
                restartStreamForResync()
            }
        } catch {
            if !Task.isCancelled {
                restartStreamForResync()
            }
        }
    }

    private func restartStreamForResync() {
        guard streamTask != nil, !streamBlockedUntilConfigurationChange else { return }
        streamTask?.cancel()
        streamTask = Task { await streamLoop() }
    }

    nonisolated static func reconcileTranscript(
        existing: [Message],
        incoming: [Message],
        coverage: PhoenixEvent.InitSnapshot.TranscriptCoverage,
        generationMatches: Bool
    ) -> [Message] {
        guard generationMatches else {
            return incoming.sorted { $0.sequence_id < $1.sequence_id }
        }
        switch coverage {
        case .complete:
            return incoming.sorted { $0.sequence_id < $1.sequence_id }
        case .preserve:
            return existing.sorted { $0.sequence_id < $1.sequence_id }
        case .tail:
            var byId = Dictionary(uniqueKeysWithValues: existing.map { ($0.message_id, $0) })
            for message in incoming {
                byId[message.message_id] = message
            }
            return byId.values.sorted { $0.sequence_id < $1.sequence_id }
        }
    }

    nonisolated static func replayFloor(
        previous: Int64,
        anchor: Int64,
        serverTip: Int64,
        generationMatches: Bool,
        restoredFromDisk: Bool
    ) -> Int64 {
        if restoredFromDisk || previous == 0 || !generationMatches || serverTip < previous {
            return anchor
        }
        return max(previous, anchor)
    }

    nonisolated static func durableMessages(
        _ messages: [Message], through sequenceCeiling: Int64
    ) -> [Message] {
        messages.filter { $0.sequence_id <= sequenceCeiling }
    }

    nonisolated static func durableCeilingAfterInit(
        anchor: Int64, messages: [Message]
    ) -> Int64 {
        max(anchor, messages.map(\.sequence_id).max() ?? 0)
    }

    nonisolated static func durableCeilingAfterLiveMessage(
        current: Int64, message: Message
    ) -> Int64 {
        // The wire shares one event shape for eager and committed assistant
        // messages. Every other message type is emitted only after commit.
        guard message.message_type != "agent" else { return current }
        return max(current, message.sequence_id)
    }

    private func reconcileOutbox() {
        outbox.reconcile(
            authoritativeMessageIds: Set(
                Self.durableMessages(
                    messages, through: durableMessageSequenceCeiling
                ).map(\.message_id)))
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
