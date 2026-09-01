import Foundation
import Observation

@Observable
@MainActor
final class ProductConversationDetailModel {
    let aggregateId: String

    private let connectivity: ConnectivityMonitor
    private let createSession: (String) -> ConversationSession?
    private let existingSession: (String) -> ConversationSession?
    private let loadSnapshot: (String, String?) async throws -> ProductConversationSnapshot
    private let didStartRefresh: @MainActor (ProductConversationRefreshCause) -> Void

    private(set) var snapshot: ProductConversationSnapshot?
    private(set) var loading = false
    private(set) var loadingOlder = false
    private(set) var loadError: String?
    private(set) var selectedTranscriptRowId: String?
    private(set) var actionTranscriptRowId: String?
    private(set) var lastDelegatedError: String?
    private(set) var lastDelegatedConnection: ConversationSession.ConnectionState = .idle
    private(set) var observedSessionIds: Set<String> = []

    private var retainedOlderPages: [ProductConversationSnapshot] = []
    private var startedTranscriptRowId: String?
    private var isActive = false
    private var loadTask: Task<Void, Never>?
    private var loadGeneration = 0
    private var pendingLoad: PendingLoad?
    private var observerGeneration = 0

    init(
        aggregateId: String,
        api: PhoenixAPI,
        connectivity: ConnectivityMonitor,
        sessionProvider: @escaping (String) -> ConversationSession?,
        existingSession: @escaping (String) -> ConversationSession? = { _ in nil },
        loadSnapshot: ((String, String?) async throws -> ProductConversationSnapshot)? = nil,
        didStartRefresh: @escaping @MainActor (ProductConversationRefreshCause) -> Void = { _ in }
    ) {
        self.aggregateId = aggregateId
        self.connectivity = connectivity
        self.createSession = sessionProvider
        self.existingSession = existingSession
        self.loadSnapshot = loadSnapshot ?? { id, before in
            try await api.getProductConversation(id: id, before: before)
        }
        self.didStartRefresh = didStartRefresh
    }

    var lifecycle: ProductConversationOrdinaryLifecycle? {
        snapshot?.ordinary_lifecycle
    }

    var isHistoryReadOnly: Bool {
        snapshot?.ordinary_lifecycle == .history
    }

    var canSendChat: Bool {
        !isHistoryReadOnly && writableTranscriptRowId != nil
    }

    var canMutateLifecycle: Bool {
        !isHistoryReadOnly && lifecycleSession?.acceptsConversationActions == true
    }

    var selectedTranscriptSession: ConversationSession? {
        guard let transcriptRowId = selectedTranscriptRowId else { return nil }
        if isHistoryReadOnly {
            return existingSession(transcriptRowId)
        }
        return sessionForSelection(transcriptRowId)
    }

    var actionSession: ConversationSession? {
        guard canSendChat, let transcriptRowId = actionTranscriptRowId else { return nil }
        return createSession(transcriptRowId)
    }

    var lifecycleSession: ConversationSession? {
        guard let transcriptRowId = latestTranscriptRowId else { return nil }
        return isHistoryReadOnly ? existingSession(transcriptRowId) : createSession(transcriptRowId)
    }

    var writableTranscriptRowId: String? {
        snapshot?.writable_transcript_row_id
    }

    var latestTranscriptRowId: String? {
        snapshot?.latest_transcript_row_id
    }

    var hasOlder: Bool { snapshot?.has_older == true }
    var olderCursor: String? { snapshot?.before }

    var transcriptItems: [ProductConversationTranscriptItem] {
        mergedSegments().flatMap(\.items)
    }

    var displayTitle: String {
        if let snapshot {
            return snapshot.canonical_root.title ?? snapshot.canonical_root.slug ?? snapshot.product_conversation_id
        }
        return aggregateId
    }

    var stateDetailSession: ConversationSession? {
        if canSendChat, let actionSession { return actionSession }
        return lifecycleSession
    }

    var segments: [ProductConversationSegment] {
        (snapshot?.segments ?? []).sorted(by: { $0.segment_ordinal < $1.segment_ordinal })
    }

    var outboxProjections: [ProductConversationOutboxProjection] {
        visibleSessionIdsForProjection.flatMap { transcriptRowId in
            guard let session = existingSession(transcriptRowId) else { return [ProductConversationOutboxProjection]() }
            let policy: ProductConversationOutboxProjection.ActionPolicy = isHistoryReadOnly
                ? .readOnly
                : .interactive(session: session)
            return session.outbox.visibleEntries.map {
                ProductConversationOutboxProjection(
                    transcriptRowId: transcriptRowId,
                    entry: $0,
                    actionPolicy: policy)
            }
        }
    }

    var composedToolUseIndex: [String: ToolUseRef] {
        var index: [String: ToolUseRef] = [:]
        for item in transcriptItems {
            guard case .message(let message) = item else { continue }
            mergeToolUseIndex(from: message, into: &index)
        }
        return index
    }

    var delegatedConnectivityAllowsActions: Bool {
        connectivity.isOnline && lastDelegatedConnection != .offline
    }

    func start() async {
        isActive = true
        await enqueueLoad(.refresh(.initial))
    }

    func stop() {
        isActive = false
        observerGeneration &+= 1
        loadGeneration &+= 1
        loadTask?.cancel()
        loadTask = nil
        pendingLoad = nil
        clearObservers()
        if let startedTranscriptRowId {
            existingSession(startedTranscriptRowId)?.closeView()
            self.startedTranscriptRowId = nil
        }
        lastDelegatedConnection = .idle
        lastDelegatedError = nil
    }

    func refresh(cause: ProductConversationRefreshCause = .manual) async {
        await enqueueLoad(.refresh(cause))
    }

    func loadOlder() async {
        guard let before = olderCursor else { return }
        await enqueueLoad(.older(before: before))
    }

    func selectTranscriptRow(id: String) {
        guard segments.contains(where: { $0.transcript_row_id == id }) else { return }
        selectedTranscriptRowId = id
        syncObserversAndSessions()
    }

    func dismissDelegatedError() {
        actionSession?.clearErrorToast()
        lastDelegatedError = nil
    }

    func applyForTesting(_ snapshot: ProductConversationSnapshot) {
        applySnapshot(snapshot, resetRetainedPages: false)
    }

    func invalidateAggregateTopologyForTesting(_ invalidation: ProductConversationTopologyInvalidation) {
        if invalidatesThisAggregate(invalidation) {
            pendingLoad = pendingLoad?.merged(with: .refresh(.delegateConversationChanged))
                ?? .refresh(.delegateConversationChanged)
            if loadTask == nil {
                loadGeneration &+= 1
                let generation = loadGeneration
                loadTask = Task { [weak self] in
                    await self?.runLoadLoop(generation: generation)
                }
            }
        }
    }

    func invalidatesAggregateForTesting(_ invalidation: ProductConversationTopologyInvalidation) -> Bool {
        invalidatesThisAggregate(invalidation)
    }

    func handleSessionEvent(transcriptRowId: String, generation: Int, event: ProductConversationSessionEvent) {
        guard generation == observerGeneration else { return }
        guard observedSessionIds.contains(transcriptRowId) else { return }
        switch event {
        case .aggregateTopologyInvalidated(let invalidation):
            if invalidatesThisAggregate(invalidation) {
                Task { await self.enqueueLoad(.refresh(.delegateConversationChanged)) }
            }
        case .connectionChanged(let connection):
            if transcriptRowId == actionTranscriptRowId {
                lastDelegatedConnection = connection
            }
        case .messagesChanged:
            break
        case .outboxChanged:
            break
        case .errorToastChanged(let message):
            if transcriptRowId == actionTranscriptRowId {
                lastDelegatedError = message
            }
        }
    }

    private func enqueueLoad(_ request: PendingLoad) async {
        switch request {
        case .refresh(let cause):
            loading = true
            if cause != .initial { loadError = nil }
        case .older:
            loadingOlder = true
            loadError = nil
        }
        if let pendingLoad {
            self.pendingLoad = pendingLoad.merged(with: request)
        } else {
            pendingLoad = request
        }
        if loadTask == nil {
            loadGeneration &+= 1
            let generation = loadGeneration
            loadTask = Task { [weak self] in
                await self?.runLoadLoop(generation: generation)
            }
        }
        await loadTask?.value
    }

    private func runLoadLoop(generation: Int) async {
        while !Task.isCancelled, generation == loadGeneration, let request = pendingLoad {
            pendingLoad = nil
            switch request {
            case .refresh(let cause):
                await performRefresh(cause: cause, generation: generation)
            case .older(let before):
                await performLoadOlder(before: before, generation: generation)
            }
        }
        guard generation == loadGeneration else { return }
        loadTask = nil
        loading = false
        loadingOlder = false
    }

    private func performRefresh(cause: ProductConversationRefreshCause, generation: Int) async {
        guard connectivity.isOnline else {
            if generation == loadGeneration { loading = false }
            return
        }
        do {
            didStartRefresh(cause)
            let fresh = try await loadSnapshot(aggregateId, nil)
            guard generation == loadGeneration, !Task.isCancelled else { return }
            applyRefreshedSnapshot(fresh)
            loadError = nil
        } catch {
            guard generation == loadGeneration, !Task.isCancelled else { return }
            loadError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        if generation == loadGeneration { loading = false }
    }

    private func performLoadOlder(before: String, generation: Int) async {
        guard connectivity.isOnline else {
            if generation == loadGeneration { loadingOlder = false }
            return
        }
        do {
            let older = try await loadSnapshot(aggregateId, before)
            guard generation == loadGeneration, !Task.isCancelled else { return }
            retainedOlderPages.append(older)
            let newestPage = snapshot ?? older
            var composed = composeSnapshot(currentPage: newestPage, retainedOlderPages: retainedOlderPages)
            composed.before = older.before
            composed.has_older = older.has_older
            applySnapshot(composed, resetRetainedPages: false)
            loadError = nil
        } catch {
            guard generation == loadGeneration, !Task.isCancelled else { return }
            loadError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        if generation == loadGeneration { loadingOlder = false }
    }

    private func applyRefreshedSnapshot(_ fresh: ProductConversationSnapshot) {
        let resetRetainedPages = snapshot?.product_conversation_id != fresh.product_conversation_id
        let composed = composeSnapshot(currentPage: fresh, retainedOlderPages: resetRetainedPages ? [] : retainedOlderPages)
        if resetRetainedPages { retainedOlderPages.removeAll() }
        applySnapshot(composed, resetRetainedPages: resetRetainedPages)
    }

    private func applySnapshot(_ newSnapshot: ProductConversationSnapshot, resetRetainedPages: Bool) {
        if resetRetainedPages { retainedOlderPages.removeAll() }
        snapshot = newSnapshot
        let available = Set(newSnapshot.segments.map(\.transcript_row_id))
        let preferredReadable = newSnapshot.latest_transcript_row_id
        let newActionTranscriptRowId = newSnapshot.writable_transcript_row_id ?? newSnapshot.latest_transcript_row_id

        if selectedTranscriptRowId == nil || !(selectedTranscriptRowId.map(available.contains) ?? false) {
            selectedTranscriptRowId = preferredReadable
        }
        if selectedTranscriptRowId == nil {
            selectedTranscriptRowId = preferredReadable
        }

        actionTranscriptRowId = newActionTranscriptRowId
        syncObserversAndSessions()
        projectDelegatedStateFromCurrentOwner()
    }

    private func composeSnapshot(
        currentPage: ProductConversationSnapshot,
        retainedOlderPages: [ProductConversationSnapshot]
    ) -> ProductConversationSnapshot {
        var byOrdinal: [Int64: ProductConversationSegment] = [:]
        for snapshot in retainedOlderPages + [currentPage] {
            for segment in snapshot.segments {
                if var existing = byOrdinal[segment.segment_ordinal] {
                    existing.messages = dedupeMessages(existing.messages + segment.messages)
                    if existing.handoff == nil { existing.handoff = segment.handoff }
                    byOrdinal[segment.segment_ordinal] = existing
                } else {
                    byOrdinal[segment.segment_ordinal] = segment
                }
            }
        }
        let deepestCursorSource = retainedOlderPages.last ?? currentPage
        return ProductConversationSnapshot(
            product_conversation_id: currentPage.product_conversation_id,
            close: currentPage.close,
            canonical_route: currentPage.canonical_route,
            requested_transcript_row_id: currentPage.requested_transcript_row_id,
            canonical_root: currentPage.canonical_root,
            ordinary_lifecycle: currentPage.ordinary_lifecycle,
            latest_transcript_row_id: currentPage.latest_transcript_row_id,
            writable_transcript_row_id: currentPage.writable_transcript_row_id,
            updated_at: currentPage.updated_at,
            presentation: currentPage.presentation,
            work_identity: currentPage.work_identity,
            source: currentPage.source,
            chain_qa_compatibility: currentPage.chain_qa_compatibility,
            segments: byOrdinal.values.sorted(by: { $0.segment_ordinal < $1.segment_ordinal }),
            before: deepestCursorSource.before,
            has_older: deepestCursorSource.has_older)
    }

    private func mergedSegments() -> [MergedSegment] {
        segments.map { segment in
            let renderedMessages = overlayMessages(for: segment)
            return MergedSegment(
                transcriptRowId: segment.transcript_row_id,
                items: renderedMessages.map(ProductConversationTranscriptItem.message)
                    + (segment.handoff.map { [.handoff($0)] } ?? []))
        }
    }

    private func overlayMessages(for segment: ProductConversationSegment) -> [Message] {
        guard segment.transcript_row_id == actionTranscriptRowId,
              let liveSession = existingSession(segment.transcript_row_id)
        else { return segment.messages }
        let baseIds = Set(segment.messages.map(\.message_id))
        let liveById = Dictionary(uniqueKeysWithValues: liveSession.messages.map { ($0.message_id, $0) })
        var rendered = segment.messages.compactMap { liveById[$0.message_id] ?? $0 }
        let suppressedBoundaryIds = suppressedBoundaryMessageIds(for: segment)
        for live in liveSession.messages where baseIds.contains(live.message_id) == false {
            if suppressedBoundaryIds.contains(live.message_id) { continue }
            rendered.append(live)
        }
        return dedupeMessages(rendered)
    }

    private func suppressedBoundaryMessageIds(for segment: ProductConversationSegment) -> Set<String> {
        var ids: Set<String> = []
        if let handoff = segment.handoff {
            switch handoff {
            case .completed(_, _, _, let openingMessageId, _):
                ids.insert(openingMessageId)
            case .historical(_, _, let continuationMessageId, _):
                ids.insert(continuationMessageId)
            }
        }
        if let predecessor = segments.last(where: { $0.segment_ordinal == segment.segment_ordinal - 1 }),
           let handoff = predecessor.handoff {
            switch handoff {
            case .completed(_, _, _, let openingMessageId, _):
                ids.insert(openingMessageId)
            case .historical(_, _, let continuationMessageId, _):
                ids.insert(continuationMessageId)
            }
        }
        return ids
    }

    private var visibleSessionIdsForProjection: Set<String> {
        var ids: Set<String> = []
        if canSendChat, let actionTranscriptRowId { ids.insert(actionTranscriptRowId) }
        if let selectedTranscriptRowId, !isHistoryReadOnly { ids.insert(selectedTranscriptRowId) }
        if let latestTranscriptRowId, !canSendChat, !isHistoryReadOnly { ids.insert(latestTranscriptRowId) }
        for segment in segments {
            if let session = existingSession(segment.transcript_row_id), !session.outbox.visibleEntries.isEmpty {
                ids.insert(segment.transcript_row_id)
            }
        }
        return ids
    }

    private func sessionForSelection(_ transcriptRowId: String) -> ConversationSession? {
        if transcriptRowId == actionTranscriptRowId {
            return actionSession
        }
        if isHistoryReadOnly {
            return existingSession(transcriptRowId)
        }
        return createSession(transcriptRowId)
    }

    private func syncObserversAndSessions() {
        guard isActive else { return }
        observerGeneration &+= 1
        let generation = observerGeneration
        let desired = visibleSessionIdsForProjection
        for transcriptRowId in observedSessionIds.subtracting(desired) {
            existingSession(transcriptRowId)?.setSessionEventObserver(nil)
        }
        for transcriptRowId in desired {
            let session: ConversationSession?
            if transcriptRowId == actionTranscriptRowId, canSendChat {
                session = createSession(transcriptRowId)
            } else if transcriptRowId == latestTranscriptRowId, !isHistoryReadOnly {
                session = createSession(transcriptRowId)
            } else if transcriptRowId == selectedTranscriptRowId, !isHistoryReadOnly {
                session = createSession(transcriptRowId)
            } else {
                session = existingSession(transcriptRowId)
            }
            session?.setSessionEventObserver { [weak self] event in
                self?.handleSessionEvent(transcriptRowId: transcriptRowId, generation: generation, event: event)
            }
        }
        observedSessionIds = desired
        syncStartedActionSession()
    }

    private func syncStartedActionSession() {
        let desiredStarted: String?
        if canSendChat {
            desiredStarted = actionTranscriptRowId
        } else if canMutateLifecycle {
            desiredStarted = latestTranscriptRowId
        } else {
            desiredStarted = nil
        }
        if let startedTranscriptRowId, startedTranscriptRowId != desiredStarted {
            existingSession(startedTranscriptRowId)?.closeView()
            self.startedTranscriptRowId = nil
        }
        guard let desiredStarted else { return }
        if startedTranscriptRowId == desiredStarted { return }
        let session = createSession(desiredStarted)
        session?.start()
        startedTranscriptRowId = desiredStarted
    }

    private func projectDelegatedStateFromCurrentOwner() {
        if canSendChat, let actionSession {
            lastDelegatedConnection = actionSession.connection
            lastDelegatedError = actionSession.lastErrorToast
        } else if let lifecycleSession {
            lastDelegatedConnection = lifecycleSession.connection
            lastDelegatedError = lifecycleSession.lastErrorToast
        } else {
            lastDelegatedConnection = .idle
            lastDelegatedError = nil
        }
    }

    private func clearObservers() {
        for transcriptRowId in observedSessionIds {
            existingSession(transcriptRowId)?.setSessionEventObserver(nil)
        }
        observedSessionIds.removeAll()
    }

    private func invalidatesThisAggregate(_ invalidation: ProductConversationTopologyInvalidation) -> Bool {
        switch invalidation.reason {
        case .aggregateIdentityChanged(let previous, let current):
            return previous == aggregateId || current == aggregateId
        default:
            return invalidation.aggregateIdentity == aggregateId
        }
    }

    private func dedupeMessages(_ messages: [Message]) -> [Message] {
        var byId: [String: Message] = [:]
        for message in messages { byId[message.message_id] = message }
        return byId.values.sorted(by: { lhs, rhs in
            if lhs.sequence_id != rhs.sequence_id { return lhs.sequence_id < rhs.sequence_id }
            return lhs.message_id < rhs.message_id
        })
    }

    private func mergeToolUseIndex(from message: Message, into index: inout [String: ToolUseRef]) {
        guard let blocks = message.content.arrayValue else { return }
        for block in blocks where block["type"]?.stringValue == "tool_use" {
            guard let id = block["id"]?.stringValue,
                  let name = block["name"]?.stringValue
            else { continue }
            index[id] = ToolUseRef(name: name, input: block["input"])
        }
    }
}

private struct MergedSegment {
    let transcriptRowId: String
    let items: [ProductConversationTranscriptItem]
}

enum ProductConversationRefreshCause: Equatable {
    case initial
    case manual
    case delegateConversationChanged
}

private enum PendingLoad: Equatable {
    case refresh(ProductConversationRefreshCause)
    case older(before: String)

    func merged(with newer: PendingLoad) -> PendingLoad {
        switch (self, newer) {
        case (_, .refresh(let cause)):
            .refresh(cause)
        case (.refresh(let cause), .older):
            .refresh(cause)
        case (.older, .older(let before)):
            .older(before: before)
        }
    }
}
