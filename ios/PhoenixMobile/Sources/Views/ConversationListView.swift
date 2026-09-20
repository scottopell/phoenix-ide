import SwiftUI

/// Conversation list backed by the disk cache: renders instantly (and fully
/// navigable) with no connectivity; pull-to-refresh or foregrounding
/// refreshes from the server.
struct ConversationListView: View {
    @Environment(AppModel.self) private var model
    @State private var showNewConversation = false
    @State private var showSettings = false
    @State private var navPath: [String] = []
    @State private var openingCoordinator = false

    var body: some View {
        NavigationStack(path: $navPath) {
            VStack(spacing: 0) {
                OfflineBanner()
                list
            }
            .navigationTitle("Conversations")
            .navigationDestination(for: String.self) { conversationId in
                if model.deletedProductHistoryIds.contains(conversationId) {
                    ContentUnavailableView(
                        "Conversation deleted",
                        systemImage: "trash",
                        description: Text("This Product History is no longer available."))
                } else if let history = model.listStore.conversations.first(where: {
                    $0.archived == true && $0.aggregateIdentity == conversationId
                }) {
                    ProductHistoryView(productConversationId: history.aggregateIdentity)
                } else if let session = model.session(for: conversationId) {
                    ConversationView(session: session)
                } else {
                    Text("Configure a server first")
                }
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    // The fleet Coordinator: one conversation that answers
                    // questions about all the others. Ordinary conversation
                    // underneath, so it inherits offline caching and the
                    // outbox for free. Enabled offline once its snapshot exists.
                    Button {
                        guard !openingCoordinator else { return }
                        openingCoordinator = true
                        Task {
                            defer { openingCoordinator = false }
                            if let id = await model.openCoordinator() {
                                navPath.append(id)
                            }
                        }
                    } label: {
                        if openingCoordinator {
                            ProgressView().controlSize(.small)
                        } else {
                            Image(systemName: "globe")
                        }
                    }
                    .disabled(
                        openingCoordinator
                            || (!model.connectivity.isOnline
                                && !model.coordinatorAvailableOffline))
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showNewConversation = true
                    } label: {
                        Image(systemName: "plus")
                    }
                    .disabled(!model.connectivity.isOnline)
                    .accessibilityIdentifier("conversationList.new")
                }
            }
            .sheet(isPresented: $showNewConversation) {
                NewConversationView()
            }
            .sheet(isPresented: $showSettings) {
                SettingsView()
            }
            .alert(
                "Action failed",
                isPresented: Binding(
                    get: { model.lastActionError != nil },
                    set: { if !$0 { model.lastActionError = nil } })
            ) {
                Button("OK") { model.lastActionError = nil }
            } message: {
                Text(model.lastActionError ?? "")
            }
            .task {
                consumePendingNavigation()
                await model.refreshList()
            }
            .onChange(of: model.pendingOpenConversationId) {
                consumePendingNavigation()
            }
        }
    }

    @ViewBuilder
    private var list: some View {
        if model.listStore.conversations.isEmpty {
            if let error = model.listStore.lastError {
                ContentUnavailableView {
                    Label("Couldn't load conversations", systemImage: "exclamationmark.icloud")
                } description: {
                    Text(error)
                } actions: {
                    Button("Retry") {
                        Task { await model.refreshList() }
                    }
                    .disabled(!model.connectivity.isOnline || model.listStore.isRefreshing)
                }
            } else {
                ContentUnavailableView {
                    Label("No conversations", systemImage: "bubble.left.and.bubble.right")
                } description: {
                    Text(
                        model.connectivity.isOnline
                            ? "Start one with the + button."
                            : "Nothing cached yet — connect once to populate the list.")
                }
            }
        } else {
            List {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    if let stale = staleness(at: context.date) {
                        Text(stale)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .listRowSeparator(.hidden)
                    }
                }
                ForEach(model.listStore.conversations, id: \.aggregateIdentity) { conversation in
                    let transcriptRowId = conversation.transcriptRowIdentity
                    let navigationId = conversation.archived == true
                        ? conversation.aggregateIdentity
                        : model.navigationConversationId(for: conversation)
                    let isCoordinator = conversation.isCoordinator
                        || transcriptRowId == model.coordinatorConversationId
                    NavigationLink(value: navigationId) {
                        ConversationRow(
                            conversation: conversation,
                            isCoordinator: isCoordinator)
                    }
                    .accessibilityIdentifier("conversationList.row.\(conversation.aggregateIdentity)")
                    .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                        if !isCoordinator && conversation.archived == true {
                            Button(role: .destructive) {
                                Task { await model.deleteHistoryConversation(conversation) }
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                            .disabled(!model.connectivity.isOnline)
                        } else if !isCoordinator && conversation.product_close_action == .available
                            && conversation.presentation_mode != "working" {
                            Button {
                                Task { await model.closeProductConversation(conversation) }
                            } label: {
                                Label("Close", systemImage: "archivebox")
                            }
                            .tint(.orange)
                            .disabled(!model.connectivity.isOnline)
                        }
                    }
                }
            }
            .listStyle(.plain)
            .refreshable {
                await model.refreshList()
            }
        }
    }

    /// Notification tap → navigate to the conversation (set by
    /// NotificationRouter; works from cold launch via .task and warm via
    /// onChange).
    private func consumePendingNavigation() {
        guard let id = model.pendingOpenConversationId else { return }
        model.pendingOpenConversationId = nil
        navPath = [model.notificationNavigationId(for: id)]
    }

    /// Freshness note shown only when the cache is meaningfully stale.
    private func staleness(at now: Date) -> String? {
        guard let refreshed = model.listStore.lastRefreshed else { return nil }
        let age = now.timeIntervalSince(refreshed)
        guard age > 120 else { return nil }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        let rel = formatter.localizedString(for: refreshed, relativeTo: now)
        return "Updated \(rel)"
    }
}

struct ConversationRow: View {
    let conversation: Conversation
    var isCoordinator = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                StateDot(
                    presentationMode: conversation.presentation_mode,
                    requiresAction: conversation.requires_action ?? false,
                    stateType: conversation.stateType)
                if isCoordinator {
                    Image(systemName: "globe")
                        .font(.caption)
                        .foregroundStyle(.tint)
                }
                Text(isCoordinator ? "Coordinator" : conversation.displayTitle)
                    .font(.body)
                    .lineLimit(1)
                Spacer()
                if let date = conversation.updatedAtDate {
                    Text(date, format: .relative(presentation: .named))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            HStack(spacing: 6) {
                Text(conversation.displaySlug)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if let branch = conversation.branch_name, !branch.isEmpty {
                    Label(branch, systemImage: "arrow.triangle.branch")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
        .padding(.vertical, 2)
    }
}

/// Inline state indicator per the Phoenix feedback pattern: green = idle/ok,
/// orange = working, blue = needs the user, red = error, gray = done/unknown.
/// The server's `presentation_mode`/`requires_action` are authoritative when
/// present (they encode judgments the raw state type can't, e.g. whether a
/// context_exhausted conversation was already continued); the state-type
/// switch is the fallback for cached rows from older snapshots.
struct StateDot: View {
    var presentationMode: String?
    var requiresAction = false
    var stateType: String?

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 8, height: 8)
    }

    private var color: Color {
        if requiresAction { return .blue }
        switch presentationMode {
        case "idle": return .green
        case "working": return .orange
        case "needs_action": return .blue
        case "error": return .red
        case "done": return .gray
        default: break  // absent — fall back to the state type
        }
        switch stateType {
        case "idle": return .green
        case "error": return .red
        case "awaiting_user_response", "awaiting_task_approval", "awaiting_recovery":
            return .blue
        case "terminal", "context_exhausted", "handed_off": return .gray
        case nil: return .gray
        default: return .orange  // any in-flight state
        }
    }
}

extension ProductConversationHandoff {
    var displaySummary: String {
        switch self {
        case .completed(_, _, _, _, let summary),
             .historical(_, _, _, let summary):
            summary
        }
    }
}

private struct ProductHistoryHandoffView: View {
    let handoff: ProductConversationHandoff

    var body: some View {
        Label {
            Text(handoff.displaySummary)
        } icon: {
            Image(systemName: "arrow.down.right.circle")
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .accessibilityIdentifier("productHistory.handoff")
    }
}

private struct ProductHistoryView: View {
    @Environment(AppModel.self) private var model
    let productConversationId: String
    @State private var cached: CachedProductHistory?
    @State private var error: ProductHistoryLoadError?

    var body: some View {
        VStack(spacing: 0) {
            OfflineBanner()
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
            Group {
                if let cached {
                    List(cached.snapshot.segments, id: \.segment_ordinal) { segment in
                        Section(segment.title ?? segment.slug ?? "Conversation") {
                            ForEach(segment.messages, id: \.id) { message in
                                MessageView(message: message)
                            }
                            if let handoff = segment.handoff {
                                ProductHistoryHandoffView(handoff: handoff)
                            }
                        }
                    }
                } else if error == .notFound {
                    ContentUnavailableView(
                        "Conversation deleted",
                        systemImage: "trash",
                        description: Text("This Product History is no longer available."))
                } else if let error {
                    ContentUnavailableView(
                        "Unable to load history",
                        systemImage: "exclamationmark.triangle",
                        description: Text(error.localizedDescription))
                } else {
                    ProgressView("Loading history…")
                }
            }
        }
        .task(id: productConversationId) {
            cached = model.cachedProductHistory(productConversationId: productConversationId)
            error = nil
            do {
                cached = try await model.loadProductHistory(productConversationId: productConversationId)
            } catch is CancellationError {
                return
            } catch let loadError as ProductHistoryLoadError {
                if loadError == .notFound { cached = nil }
                if cached == nil { error = loadError }
            } catch {
                if cached == nil { self.error = .emptyResponse }
            }
        }
    }

    private func cacheAgeNote(at now: Date) -> String? {
        guard !model.connectivity.isOnline,
              let cached,
              now.timeIntervalSince(cached.fetchedAt) > 120
        else { return nil }
        let fetchedAt = cached.fetchedAt
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return "Cached \(formatter.localizedString(for: fetchedAt, relativeTo: now))"
    }
}
