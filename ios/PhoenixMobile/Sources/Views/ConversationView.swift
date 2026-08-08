import SwiftUI

/// One conversation: cached history + live SSE updates + outbox chips +
/// composer. Fully readable offline; sends queue while disconnected.
struct ConversationView: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession

    @State private var draft = ""

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner()
            ConnectionStateBar(session: session)
            if session.isHardDeleted {
                ContentUnavailableView(
                    "Conversation deleted",
                    systemImage: "trash",
                    description: Text("This conversation was deleted on the server."))
            } else {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    if let staleness = cacheAgeNote(at: context.date) {
                        Text(staleness)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 2)
                            .background(.thinMaterial)
                    }
                }
                if !session.outbox.persistenceHealthy {
                    Label(
                        "Storage write failed — queued messages may not survive a restart",
                        systemImage: "externaldrive.badge.exclamationmark")
                        .font(.caption2)
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                        .background(.red.gradient)
                }
                if isUncachedOffline {
                    ContentUnavailableView {
                        Label("Not cached on this device", systemImage: "icloud.slash")
                    } description: {
                        Text("Open this conversation once while connected to read it offline.")
                    }
                    .frame(maxHeight: .infinity)
                } else {
                    messageList
                }
                StateDetailView(session: session)
                ComposerView(session: session, draft: $draft)
            }
        }
        .navigationTitle(session.conversation?.displayTitle ?? "Conversation")
        .navigationBarTitleDisplayMode(.inline)
        .alert(
            "Server error",
            isPresented: Binding(
                get: { session.lastErrorToast != nil },
                set: { if !$0 { session.clearErrorToast() } })
        ) {
            Button("OK") { session.clearErrorToast() }
        } message: {
            Text(session.lastErrorToast ?? "")
        }
        .onAppear { session.start() }
        .onDisappear { session.closeView() }
    }

    /// Cache-age note while disconnected (REQ-IOS-001): only shown when
    /// the snapshot is meaningfully stale, mirroring the list's threshold.
    private func cacheAgeNote(at now: Date) -> String? {
        let serverUnavailable: Bool
        if case .waitingToRetry = session.connection {
            serverUnavailable = true
        } else {
            serverUnavailable = false
        }
        guard (!model.connectivity.isOnline || serverUnavailable),
              session.connection != .live,
              let syncedAt = session.snapshotSyncedAt,
              now.timeIntervalSince(syncedAt) > 120
        else { return nil }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        let rel = formatter.localizedString(for: syncedAt, relativeTo: now)
        return "Cached \(rel)"
    }

    /// Offline with nothing cached and nothing queued: an empty transcript
    /// would read as data loss — name the actual situation instead.
    private var isUncachedOffline: Bool {
        !model.connectivity.isOnline
            && session.conversation == nil
            && session.messages.isEmpty
            && session.outbox.visibleEntries.isEmpty
    }

    private var messageList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(session.messages) { message in
                        MessageView(message: message, toolIndex: session.toolUseIndex)
                            .id(message.message_id)
                    }
                    if !session.streamingText.isEmpty {
                        StreamingBubble(text: session.streamingText)
                            .id("streaming")
                    }
                    OutboxSection(session: session)
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .defaultScrollAnchor(.bottom)
            .onChange(of: session.messages.count) {
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .onChange(of: session.streamingText) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
    }
}

/// Inline connection state, shown only when not live — quiet when healthy.
struct ConnectionStateBar: View {
    let session: ConversationSession

    var body: some View {
        switch session.connection {
        case .live, .idle:
            EmptyView()
        case .connecting:
            bar { Text("Connecting…") }
        case .offline:
            EmptyView()  // OfflineBanner and the conversation cache note cover this.
        case .waitingToRetry:
            bar { Text("Connection lost — reconnecting…") }
        }
    }

    private func bar(@ViewBuilder _ content: () -> Text) -> some View {
        content()
            .font(.caption2)
            .foregroundStyle(.orange)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 3)
            .background(.thinMaterial)
    }
}

/// In-flight LLM text from token events.
struct StreamingBubble: View {
    let text: String

    var body: some View {
        HStack {
            Text(text)
                .font(.body)
                .padding(10)
                .background(Color(.secondarySystemBackground))
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 40)
        }
        .opacity(0.85)
    }
}

/// Queued/failed local messages rendered after authoritative history, per
/// the union-without-duplicates rule: entries disappear the moment the
/// server's copy of the same message_id lands.
struct OutboxSection: View {
    let session: ConversationSession

    var body: some View {
        ForEach(session.outbox.visibleEntries) { entry in
            OutboxEntryView(entry: entry, session: session)
        }
    }
}

struct OutboxEntryView: View {
    let entry: OutboxEntry
    let session: ConversationSession

    var body: some View {
        VStack(alignment: .trailing, spacing: 3) {
            HStack {
                Spacer(minLength: 40)
                Text(entry.text)
                    .font(.body)
                    .padding(10)
                    .background(bubbleColor)
                    .foregroundStyle(.white)
                    .clipShape(RoundedRectangle(cornerRadius: 12))
            }
            statusLine
        }
    }

    private var bubbleColor: Color {
        switch entry.status {
        case .failed, .recoverableInconsistency: return .red.opacity(0.75)
        default: return .accentColor.opacity(0.65)
        }
    }

    @ViewBuilder
    private var statusLine: some View {
        switch entry.status {
        case .pending:
            Label(
                entry.acceptedByServer ? "Sent — awaiting confirmation" : "Queued — will send",
                systemImage: entry.acceptedByServer ? "checkmark.circle" : "clock")
                .font(.caption2)
                .foregroundStyle(.secondary)
        case .steeringQueued:
            Label("Queued for after current turn", systemImage: "text.append")
                .font(.caption2)
                .foregroundStyle(.secondary)
        case .failed, .recoverableInconsistency:
            HStack(spacing: 12) {
                if let err = entry.lastError {
                    Text(err)
                        .font(.caption2)
                        .foregroundStyle(.red)
                        .lineLimit(1)
                }
                Button("Retry") { session.retryEntry(entry.localId) }
                    .font(.caption.bold())
                Button("Discard", role: .destructive) {
                    session.dismissEntry(entry.localId)
                }
                .font(.caption)
            }
        case .reconciled, .dismissed:
            EmptyView()
        }
    }
}
