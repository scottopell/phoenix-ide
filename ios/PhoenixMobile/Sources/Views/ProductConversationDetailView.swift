import SwiftUI

struct ProductConversationDetailView: View {
    @Environment(AppModel.self) private var model
    @State private var detailModel: ProductConversationDetailModel
    @State private var draft = ""

    init(aggregateId: String, model: AppModel) {
        _detailModel = State(initialValue: model.productConversationDetailModel(for: aggregateId))
    }

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner()
            if let error = detailModel.loadError, detailModel.snapshot == nil {
                ContentUnavailableView {
                    Label("Unable to load conversation", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                }
                .frame(maxHeight: .infinity)
            } else if detailModel.loading && detailModel.snapshot == nil {
                ProgressView("Loading…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProductConversationTranscriptView(items: detailModel.transcriptItems, toolIndex: toolIndex, streamingText: streamingText, outboxSession: outboxSession)
                if let session = detailModel.stateDetailSession {
                    ProductConversationStateDetailView(session: session, readOnly: detailModel.isReadOnly)
                }
                ProductConversationSegmentPicker(model: detailModel)
                if detailModel.isReadOnly {
                    ProductConversationReadOnlyFooter()
                } else if let session = detailModel.actionSession {
                    ComposerView(session: session, draft: $draft)
                        .accessibilityIdentifier("conversation.productComposer")
                }
            }
        }
        .navigationTitle(detailModel.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .task { await detailModel.start() }
        .onDisappear { detailModel.stop() }
    }

    private var outboxSession: ConversationSession? {
        guard !detailModel.isReadOnly else { return nil }
        return detailModel.actionSession
    }

    private var streamingText: String {
        detailModel.actionSession?.streamingText ?? ""
    }

    private var toolIndex: [String: ToolUseRef] {
        detailModel.actionSession?.toolUseIndex ?? [:]
    }
}

struct ProductConversationStateDetailView: View {
    let session: ConversationSession
    let readOnly: Bool

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
            StateDetailView(session: session)
        }
    }
}

struct ProductConversationTranscriptView: View {
    let items: [ProductConversationTranscriptItem]
    let toolIndex: [String: ToolUseRef]
    let streamingText: String
    var outboxSession: ConversationSession?

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                        switch item {
                        case .message(let message):
                            MessageView(message: message, toolIndex: toolIndex)
                                .id(message.message_id)
                        case .handoff(let handoff):
                            ProductConversationHandoffView(handoff: handoff)
                        }
                    }
                    if !streamingText.isEmpty {
                        StreamingBubble(text: streamingText)
                            .id("streaming")
                    }
                    if let outboxSession {
                        OutboxSection(session: outboxSession)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .defaultScrollAnchor(.bottom)
            .onChange(of: items.count) {
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .onChange(of: streamingText) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
        .accessibilityIdentifier("conversation.transcript")
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
