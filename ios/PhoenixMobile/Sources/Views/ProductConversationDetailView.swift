import SwiftUI

struct ProductConversationDetailView: View {
    @Environment(AppModel.self) private var model
    let aggregateId: String
    let initialTranscriptRowId: String?
    @State private var detailModel: ProductConversationDetailModel
    @State private var draft = ""

    init(aggregateId: String, initialTranscriptRowId: String? = nil, model: AppModel) {
        self.aggregateId = aggregateId
        self.initialTranscriptRowId = initialTranscriptRowId
        _detailModel = State(initialValue: model.productConversationDetailModel(
            for: aggregateId,
            initialTranscriptRowId: initialTranscriptRowId))
    }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner()
            if let error = detailModel.loadError, detailModel.snapshot == nil, detailModel.fallbackSession == nil {
                ContentUnavailableView {
                    Label("Unable to load conversation", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                }
                .frame(maxHeight: .infinity)
            } else if detailModel.loading && detailModel.snapshot == nil && detailModel.fallbackSession == nil {
                ProgressView("Loading…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                if detailModel.hasOlder {
                    Button(detailModel.loadingOlder ? "Loading older…" : "Load older messages") {
                        Task { await detailModel.loadOlder() }
                    }
                    .disabled(detailModel.loadingOlder)
                    .padding(.top, 8)
                    .accessibilityIdentifier("productConversation.loadOlder")
                }
                ProductConversationTranscriptView(
                    items: detailModel.transcriptItems,
                    toolIndex: detailModel.composedToolUseIndex,
                    streamingText: streamingText,
                    outboxProjections: detailModel.outboxProjections,
                    transcriptMutation: detailModel.transcriptMutation)
                if let session = detailModel.stateDetailSession {
                    ProductConversationStateDetailView(
                        session: session,
                        readOnly: detailModel.isHistoryReadOnly,
                        isOnline: detailModel.delegatedConnectivityAllowsActions)
                }
                ProductConversationSegmentPicker(model: detailModel)
                if let session = detailModel.currentOwnerSession, !session.outbox.persistenceHealthy {
                    HStack(spacing: 6) {
                        Image(systemName: "externaldrive.badge.exclamationmark")
                            .foregroundStyle(.orange)
                        Text("Queued message storage failed — keep this screen open until you retry or discard pending messages.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 4)
                    .accessibilityIdentifier("productConversation.outboxPersistenceWarning")
                }
                if let error = detailModel.currentOwnerSession?.lastErrorToast {
                    InlineErrorBanner(message: error) { detailModel.dismissDelegatedError() }
                }
                if let session = detailModel.currentOwnerSession, !detailModel.isHistoryReadOnly {
                    ConnectionStateBar(session: session)
                }
                if detailModel.canSendChat, let session = detailModel.actionSession {
                    ComposerView(session: session, draft: $draft)
                        .accessibilityIdentifier("conversation.productComposer")
                } else if detailModel.isHistoryReadOnly {
                    ProductConversationReadOnlyFooter()
                } else {
                    ProductConversationNoWritableChatFooter()
                }
            }
        }
        .navigationTitle(detailModel.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: model.configurationIdentity) {
            let current = detailModel
            detailModel = model.productConversationDetailModel(
                for: aggregateId,
                initialTranscriptRowId: initialTranscriptRowId)
            if current !== detailModel {
                current.stop()
            }
            await detailModel.start()
        }
        .onDisappear { detailModel.stop() }
    }

    private var streamingText: String {
        detailModel.actionSession?.streamingText ?? ""
    }
}

struct ProductConversationStateDetailView: View {
    let session: ConversationSession
    let readOnly: Bool
    let isOnline: Bool

    var body: some View {
        if readOnly {
            StateDetailBody(
                state: session.typedState,
                presentationMode: session.presentationMode ?? "idle",
                agentWorking: session.agentWorking,
                isOnline: false,
                acceptsActions: false,
                busy: session.actionInFlight != nil,
                convState: session.convState,
                resolveNavigation: { $0 },
                onAction: { _ in })
        } else {
            StateDetailBody(
                state: session.typedState,
                presentationMode: session.presentationMode ?? "idle",
                agentWorking: session.agentWorking,
                isOnline: isOnline,
                acceptsActions: session.acceptsConversationActions,
                busy: session.actionInFlight != nil,
                convState: session.convState,
                resolveNavigation: { $0 },
                onAction: { session.perform($0) })
        }
    }
}

struct ProductConversationTranscriptView: View {
    let items: [ProductConversationTranscriptItem]
    let toolIndex: [String: ToolUseRef]
    let streamingText: String
    let outboxProjections: [ProductConversationOutboxProjection]
    let transcriptMutation: TranscriptMutation

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(items) { item in
                        switch item {
                        case .message(let message):
                            MessageView(message: message, toolIndex: toolIndex)
                        case .handoff(let handoff):
                            ProductConversationHandoffView(handoff: handoff)
                        }
                        Color.clear.frame(height: 0).id(item.id)
                    }
                    if !streamingText.isEmpty {
                        StreamingBubble(text: streamingText)
                            .id("streaming")
                    }
                    if !outboxProjections.isEmpty {
                        ProductConversationOutboxList(projections: outboxProjections)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .defaultScrollAnchor(.bottom)
            .onChange(of: transcriptMutation) {
                switch transcriptMutation {
                case .appendedLive:
                    withAnimation(.easeOut(duration: 0.2)) {
                        proxy.scrollTo("bottom", anchor: .bottom)
                    }
                case .prependedOlder, .unknown, .unchanged:
                    break
                }
            }
            .onChange(of: items) { oldItems, _ in
                if transcriptMutation == .prependedOlder,
                   let anchorId = oldItems.first?.id
                {
                    proxy.scrollTo(anchorId, anchor: .top)
                }
            }
            .onChange(of: streamingText) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
        .accessibilityIdentifier("conversation.transcript")
    }
}


struct ProductConversationOutboxList: View {
    let projections: [ProductConversationOutboxProjection]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(projections) { projection in
                VStack(alignment: .leading, spacing: 6) {
                    Text("Pending in \(projection.transcriptRowId)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                    switch projection.actionPolicy {
                    case .readOnly:
                        OutboxReadOnlyView(entry: projection.entry)
                    case .interactive(let session):
                        OutboxEntryView(entry: projection.entry, session: session)
                    }
                }
            }
        }
    }
}

struct OutboxReadOnlyView: View {
    let entry: OutboxEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(entry.text)
                .font(.body)
            Text(statusText)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.secondary.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .accessibilityIdentifier("productConversation.readOnlyOutbox")
    }

    private var statusText: String {
        switch entry.status {
        case .pending:
            "Pending"
        case .steeringQueued:
            "Queued"
        case .recoverableInconsistency:
            "Pending server reconciliation"
        case .failed:
            "Failed"
        case .reconciled:
            "Reconciled"
        case .dismissed:
            "Dismissed"
        }
    }
}

struct ProductConversationHandoffView: View {
    let handoff: ProductConversationHandoff

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: "arrow.left.arrow.right.circle")
                .font(.caption.bold())
            Text(summary)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.secondary.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .accessibilityIdentifier("productConversation.handoff")
    }

    private var title: String {
        switch handoff {
        case .completed:
            "Work continued in a newer conversation"
        case .historical:
            "Historical handoff"
        }
    }

    private var summary: String {
        switch handoff {
        case .completed(_, _, _, _, let summary):
            summary
        case .historical(_, _, _, let summary):
            summary
        }
    }
}

struct ProductConversationSegmentPicker: View {
    @Bindable var model: ProductConversationDetailModel

    var body: some View {
        if model.segments.count > 1 {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(model.segments, id: \.transcript_row_id) { segment in
                        Button {
                            model.selectTranscriptRow(id: segment.transcript_row_id)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(segment.title ?? segment.slug ?? segment.transcript_row_id)
                                    .font(.caption.bold())
                                Text(segment.transcript_row_id)
                                    .font(.caption2.monospaced())
                                    .foregroundStyle(.secondary)
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .background(model.selectedTranscriptRowId == segment.transcript_row_id ? Color.accentColor.opacity(0.15) : Color.secondary.opacity(0.08))
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("productConversation.segment.\(segment.transcript_row_id)")
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        }
    }
}

struct ProductConversationReadOnlyFooter: View {
    var body: some View {
        Text("History is read-only in the iOS client.")
            .font(.caption2)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .background(.bar)
            .accessibilityIdentifier("conversation.readOnlyFooter")
    }
}

struct ProductConversationNoWritableChatFooter: View {
    var body: some View {
        Text("Chat is unavailable for this conversation, but state actions may still be available.")
            .font(.caption2)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .background(.bar)
            .accessibilityIdentifier("conversation.noWritableChatFooter")
    }
}

struct InlineErrorBanner: View {
    let message: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(message)
                .font(.caption)
            Spacer()
            Button("Dismiss", action: onDismiss)
                .font(.caption.bold())
        }
        .padding(10)
        .background(Color.orange.opacity(0.1))
        .accessibilityIdentifier("productConversation.inlineError")
    }
}
