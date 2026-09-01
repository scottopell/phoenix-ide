import Foundation
import Observation

@Observable
@MainActor
final class ProductConversationDetailModel {
    let aggregateId: String

    private let api: PhoenixAPI
    private let connectivity: ConnectivityMonitor
    private let sessionProvider: (String) -> ConversationSession?

    private(set) var snapshot: ProductConversationSnapshot?
    private(set) var loading = false
    private(set) var loadError: String?
    private(set) var selectedTranscriptRowId: String?
    private(set) var actionTranscriptRowId: String?
    private var startedTranscriptRowId: String?
    private var isActive = false

    init(
        aggregateId: String,
        api: PhoenixAPI,
        connectivity: ConnectivityMonitor,
        sessionProvider: @escaping (String) -> ConversationSession?
    ) {
        self.aggregateId = aggregateId
        self.api = api
        self.connectivity = connectivity
        self.sessionProvider = sessionProvider
    }

    var lifecycle: ProductConversationOrdinaryLifecycle? {
        snapshot?.ordinary_lifecycle
    }

    var isReadOnly: Bool {
        guard let snapshot else { return false }
        return snapshot.ordinary_lifecycle == .history || snapshot.writable_transcript_row_id == nil
    }

    var selectedTranscriptSession: ConversationSession? {
        guard let transcriptRowId = selectedTranscriptRowId else { return nil }
        return sessionProvider(transcriptRowId)
    }

    var actionSession: ConversationSession? {
        guard let transcriptRowId = actionTranscriptRowId else { return nil }
        return sessionProvider(transcriptRowId)
    }

    var writableTranscriptRowId: String? {
        snapshot?.writable_transcript_row_id
    }

    var latestTranscriptRowId: String? {
        snapshot?.latest_transcript_row_id
    }

    var transcriptItems: [ProductConversationTranscriptItem] {
        guard let snapshot else { return [] }
        var items: [ProductConversationTranscriptItem] = []
        for segment in segments {
            let messages: [Message]
            if segment.transcript_row_id == actionTranscriptRowId,
               let liveMessages = actionSession?.messages,
               !liveMessages.isEmpty {
                messages = liveMessages
            } else {
                messages = segment.messages
            }
            items.append(contentsOf: messages.map(ProductConversationTranscriptItem.message))
            if let handoff = segment.handoff {
                items.append(.handoff(handoff))
            }
        }
        return items
    }

    var displayTitle: String {
        if let snapshot {
            return snapshot.canonical_root.title ?? snapshot.canonical_root.slug ?? snapshot.product_conversation_id
        }
        return aggregateId
    }

    var stateDetailSession: ConversationSession? {
        actionSession ?? latestTranscriptRowId.flatMap(sessionProvider)
    }

    var segments: [ProductConversationSegment] {
        (snapshot?.segments ?? []).sorted(by: { $0.segment_ordinal < $1.segment_ordinal })
    }

    func start() async {
        isActive = true
        await refresh()
        syncStartedActionSession()
    }

    func stop() {
        isActive = false
        if let startedTranscriptRowId {
            sessionProvider(startedTranscriptRowId)?.closeView()
            self.startedTranscriptRowId = nil
        }
    }

    func refresh() async {
        guard connectivity.isOnline else { return }
        loading = true
        defer { loading = false }
        do {
            let fresh = try await api.getProductConversation(id: aggregateId)
            apply(snapshot: fresh)
            loadError = nil
        } catch {
            loadError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    func apply(snapshot: ProductConversationSnapshot) {
        self.snapshot = snapshot
        let preferredReadable = snapshot.latest_transcript_row_id
        let preferredAction = snapshot.writable_transcript_row_id ?? snapshot.latest_transcript_row_id
        let available = Set(snapshot.segments.map(\.transcript_row_id))
        if selectedTranscriptRowId == nil || !(selectedTranscriptRowId.map(available.contains) ?? false) {
            selectedTranscriptRowId = preferredReadable
        }
        actionTranscriptRowId = preferredAction
        if selectedTranscriptRowId == nil {
            selectedTranscriptRowId = preferredReadable
        }
        syncStartedActionSession()
    }

    func selectTranscriptRow(id: String) {
        guard segments.contains(where: { $0.transcript_row_id == id }) else { return }
        selectedTranscriptRowId = id
    }

    private func syncStartedActionSession() {
        guard isActive, !isReadOnly else {
            if let startedTranscriptRowId {
                sessionProvider(startedTranscriptRowId)?.closeView()
                self.startedTranscriptRowId = nil
            }
            return
        }
        guard let actionTranscriptRowId else { return }
        if startedTranscriptRowId == actionTranscriptRowId { return }
        if let startedTranscriptRowId {
            sessionProvider(startedTranscriptRowId)?.closeView()
        }
        sessionProvider(actionTranscriptRowId)?.start()
        startedTranscriptRowId = actionTranscriptRowId
    }
}
