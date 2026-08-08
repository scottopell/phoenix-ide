import SwiftUI

/// Conversation list backed by the disk cache: renders instantly (and fully
/// navigable) with no connectivity; pull-to-refresh or foregrounding
/// refreshes from the server.
struct ConversationListView: View {
    @Environment(AppModel.self) private var model
    @State private var showNewConversation = false
    @State private var showSettings = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                OfflineBanner()
                list
            }
            .navigationTitle("Conversations")
            .navigationDestination(for: String.self) { conversationId in
                if let session = model.session(for: conversationId) {
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
                    Button {
                        showNewConversation = true
                    } label: {
                        Image(systemName: "plus")
                    }
                    .disabled(!model.connectivity.isOnline)
                }
            }
            .sheet(isPresented: $showNewConversation) {
                NewConversationView()
            }
            .sheet(isPresented: $showSettings) {
                SettingsView()
            }
            .task {
                await model.refreshList()
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
                if let stale = staleness {
                    Text(stale)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .listRowSeparator(.hidden)
                }
                ForEach(model.listStore.conversations) { conversation in
                    NavigationLink(value: conversation.id) {
                        ConversationRow(conversation: conversation)
                    }
                }
            }
            .listStyle(.plain)
            .refreshable {
                await model.refreshList()
            }
        }
    }

    /// Freshness note shown only when the cache is meaningfully stale.
    private var staleness: String? {
        guard let refreshed = model.listStore.lastRefreshed else { return nil }
        let age = Date().timeIntervalSince(refreshed)
        guard age > 120 else { return nil }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        let rel = formatter.localizedString(for: refreshed, relativeTo: Date())
        return "Updated \(rel)"
    }
}

struct ConversationRow: View {
    let conversation: Conversation

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                StateDot(
                    presentationMode: conversation.presentation_mode,
                    requiresAction: conversation.requires_action ?? false,
                    stateType: conversation.stateType)
                Text(conversation.displayTitle)
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
                Text(conversation.slug)
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
        case "awaiting_user_response", "awaiting_task_approval",
             "awaiting_commission_review_approval", "awaiting_recovery":
            return .blue
        case "terminal", "context_exhausted", "handed_off": return .gray
        case nil: return .gray
        default: return .orange  // any in-flight state
        }
    }
}
