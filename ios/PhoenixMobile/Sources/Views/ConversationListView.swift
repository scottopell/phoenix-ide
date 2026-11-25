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

    static func ordinaryRows(from conversations: [Conversation], coordinatorConversationId: String?) -> [Conversation] {
        conversations.filter {
            !$0.isCoordinator && $0.transcriptRowIdentity != coordinatorConversationId
        }
    }

    private var coordinatorUnread: ProductConversationUnreadReason? {
        model.listStore.coordinatorReadState?.read_state.unreadReason
    }

    private var emberAccessibilityLabel: String {
        if openingCoordinator { return "Opening Ember, Global coordinator" }
        guard let coordinatorUnread else { return "Open Ember, Global coordinator" }
        return "Open Ember, Global coordinator, " + UnreadDot.label(for: coordinatorUnread)
    }

    var body: some View {
        NavigationStack(path: $navPath) {
            VStack(spacing: 0) {
                OfflineBanner()
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
                    HStack(spacing: 10) {
                        Image(systemName: "sparkle")
                            .foregroundStyle(.tint)
                            .frame(width: 32, height: 32)
                            .background(.tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Ember").font(.subheadline.weight(coordinatorUnread == nil ? .semibold : .bold))
                            Text("Global coordinator").font(.caption).foregroundStyle(.secondary)
                        }
                        if let coordinatorUnread {
                            UnreadDot(reason: coordinatorUnread)
                        }
                        Spacer()
                        if openingCoordinator { ProgressView().controlSize(.small) }
                        else { Image(systemName: "arrow.right").foregroundStyle(.secondary) }
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(emberAccessibilityLabel)
                .accessibilityHint(openingCoordinator ? "Opening the Global coordinator" : (model.connectivity.isOnline || model.coordinatorAvailableOffline ? "Open the Global coordinator" : "Connect to open Ember"))
                .disabled(openingCoordinator || (!model.connectivity.isOnline && !model.coordinatorAvailableOffline))
                .accessibilityIdentifier("conversationList.ember")
                .contextMenu {
                    if let coordinatorId = model.listStore.coordinatorReadState?.product_conversation_id {
                        Button {
                            Task { await model.setUnread(coordinatorUnread == nil, productConversationId: coordinatorId) }
                        } label: {
                            coordinatorUnread == nil
                                ? Label("Mark as Unread", systemImage: "circle.fill")
                                : Label("Mark as Read", systemImage: "circle")
                        }
                        .disabled(!model.connectivity.isOnline)
                    }
                }
                list
            }
            .navigationTitle("Conversations")
            .navigationDestination(for: String.self) { conversationId in
                if model.deletedProductHistoryIds.contains(conversationId) {
                    ContentUnavailableView(
                        "Conversation deleted",
                        systemImage: "trash",
                        description: Text("This Product History is no longer available."))
                } else if let conversation = model.listStore.conversations.first(where: {
                    $0.aggregateIdentity == conversationId
                        && $0.product_conversation_id != nil
                        && !$0.isCoordinator
                }) {
                    ProductConversationDetailView(
                        productConversationId: conversation.aggregateIdentity,
                        historyOnly: conversation.archived == true,
                        fallbackTranscriptRowId: model.navigationConversationId(for: conversation))
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
            .confirmationDialog(
                closeConfirmationTitle,
                isPresented: Binding(
                    get: { model.pendingProductCloseConfirmation?.kind != nil },
                    set: { _ in })
            ) {
                Button(closeConfirmationActionTitle, role: closeConfirmationActionRole) {
                    Task { await model.resolvePendingProductCloseConfirmation(confirm: true) }
                }
                .disabled(closeConfirmationActionDisabled)
                Button(closeConfirmationCancelTitle, role: .cancel) {
                    Task { await model.resolvePendingProductCloseConfirmation(confirm: false) }
                }
                .disabled(model.isResolvingPendingProductClose)
            } message: {
                Text(closeConfirmationMessage)
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

    private var closeConfirmationTitle: String {
        switch model.pendingProductCloseConfirmation?.kind {
        case .stopWork: "Stop work and close?"
        case .losses: "Confirm Close losses?"
        case .repair: "Close needs repair"
        case nil: ""
        }
    }

    private var closeConfirmationActionTitle: String {
        switch model.pendingProductCloseConfirmation?.kind {
        case .stopWork: "Stop Work"
        case .losses: "Accept Losses"
        case .repair: "Retry Retirement"
        case nil: "Close"
        }
    }

    private var closeConfirmationActionRole: ButtonRole? {
        model.pendingProductCloseConfirmation?.kind == .repair ? nil : .destructive
    }

    private var closeConfirmationCancelTitle: String {
        model.pendingProductCloseConfirmation?.kind == .repair ? "Not Now" : "Cancel Close"
    }

    private var closeConfirmationActionDisabled: Bool {
        guard !model.isResolvingPendingProductClose else { return true }
        guard let pending = model.pendingProductCloseConfirmation else { return true }
        if pending.kind == .losses {
            return !ProductCloseLossInventory.isComplete(pending.close.losses)
                || pending.close.confirmation_snapshot == nil
        }
        return false
    }

    private var closeConfirmationMessage: String {
        switch model.pendingProductCloseConfirmation?.kind {
        case .stopWork:
            return "This conversation is still working. Stop its active work before Close continues."
        case .losses:
            guard let losses = model.pendingProductCloseConfirmation?.close.losses,
                  ProductCloseLossInventory.isComplete(losses)
            else {
                return "The server did not provide the exact loss inventory. Refresh before accepting losses."
            }
            return "Close will permanently discard these exact items:\n\n"
                + ProductCloseLossInventory.message(losses)
        case .repair:
            return "Resource retirement did not finish. Retry retirement for this exact Close attempt after repairing the reported resource problem."
        case nil:
            return ""
        }
    }

    @ViewBuilder
    private var list: some View {
        if Self.ordinaryRows(from: model.listStore.conversations, coordinatorConversationId: model.coordinatorConversationId).isEmpty {
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
                ForEach(Self.ordinaryRows(from: model.listStore.conversations, coordinatorConversationId: model.coordinatorConversationId), id: \.aggregateIdentity) { conversation in
                    let transcriptRowId = conversation.transcriptRowIdentity
                    let navigationId = conversation.product_conversation_id != nil
                        && !conversation.isCoordinator
                        ? conversation.aggregateIdentity
                        : model.navigationConversationId(for: conversation)
                    let isCoordinator = conversation.isCoordinator
                        || transcriptRowId == model.coordinatorConversationId
                    NavigationLink(value: navigationId) {
                        ConversationRow(
                            conversation: conversation,
                            isCoordinator: isCoordinator,
                            readState: model.listStore.readState(forAggregateId: conversation.aggregateIdentity),
                            cachedPr: model.listStore.cachedPr(forAggregateId: conversation.aggregateIdentity))
                    }
                    .accessibilityIdentifier("conversationList.row.\(conversation.aggregateIdentity)")
                    .swipeActions(edge: .leading, allowsFullSwipe: true) {
                        if let productConversationId = conversation.product_conversation_id {
                            let unread = model.listStore.readState(forAggregateId: productConversationId) != .read
                            Button {
                                Task { await model.setUnread(!unread, productConversationId: productConversationId) }
                            } label: {
                                unread
                                    ? Label("Read", systemImage: "circle")
                                    : Label("Unread", systemImage: "circle.fill")
                            }
                            .tint(.blue)
                            .disabled(!model.connectivity.isOnline)
                        }
                    }
                    .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                        if !isCoordinator && conversation.archived == true {
                            Button(role: .destructive) {
                                Task { await model.deleteHistoryConversation(conversation) }
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                            .disabled(!model.connectivity.isOnline)
                        } else if !isCoordinator && conversation.product_close_action == .available {
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
    var readState: ProductConversationReadState = .read
    var cachedPr: ProductConversationPr?

    var displayTitle: String { isCoordinator ? "Ember" : conversation.displayTitle }

    /// One line: state, title, unread, PR. Only when the PR badge cannot fit
    /// beside the title does it move to a second line.
    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 6) {
                leading
                if let cachedPr { PrBadge(pr: cachedPr) }
                Spacer(minLength: 0)
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    leading
                    Spacer(minLength: 0)
                }
                if let cachedPr { PrBadge(pr: cachedPr) }
            }
        }
        .padding(.vertical, 2)
    }

    @ViewBuilder private var leading: some View {
        StateDot(
            presentationMode: conversation.presentation_mode,
            requiresAction: conversation.requires_action ?? false,
            stateType: conversation.stateType)
        if isCoordinator {
            Image(systemName: "globe")
                .font(.caption)
                .foregroundStyle(.tint)
        }
        Text(displayTitle)
            .font(readState == .read ? .body : .body.weight(.semibold))
            .lineLimit(1)
            .truncationMode(.tail)
            // Ideal width is what ViewThatFits measures: a title may truncate
            // down to this before the PR badge is pushed to a second line.
            .frame(idealWidth: 96, alignment: .leading)
        if let reason = readState.unreadReason {
            UnreadDot(reason: reason)
        }
    }
}

/// Compact cached-PR badge: number plus state for non-open PRs.
struct PrBadge: View {
    let pr: ProductConversationPr

    static func label(for pr: ProductConversationPr) -> String {
        switch pr.display_state {
        case .open: "#\(pr.number)"
        case .draft: "#\(pr.number) draft"
        case .merged: "#\(pr.number) merged"
        case .closed: "#\(pr.number) closed"
        }
    }

    static func accessibilityLabel(for pr: ProductConversationPr) -> String {
        var parts = ["PR \(pr.number)"]
        if pr.display_state != .open { parts.append(pr.display_state.rawValue) }
        switch pr.feedback_status {
        case .open: break
        case .inProgress: parts.append("feedback in progress")
        case .approved: parts.append("feedback approved")
        }
        return parts.joined(separator: ", ")
    }

    private var tint: Color {
        switch pr.display_state {
        case .open: .secondary
        case .draft: .orange
        case .merged: .purple
        case .closed: .red
        }
    }

    var body: some View {
        Text(Self.label(for: pr))
            .font(.caption2.monospacedDigit().weight(.semibold))
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .foregroundStyle(tint)
            .background(tint.opacity(0.15), in: Capsule())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Self.accessibilityLabel(for: pr))
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

struct ProductHistoryCachePresentation {
    static func shouldShowAge(
        isOnline: Bool,
        onlineRefreshSucceeded: Bool,
        fetchedAt: Date,
        now: Date,
        stalenessThreshold: TimeInterval = 120
    ) -> Bool {
        (!isOnline || !onlineRefreshSucceeded)
            && now.timeIntervalSince(fetchedAt) > stalenessThreshold
    }
}

struct ProductHistoryLoadKey: Equatable {
    var productConversationId: String
    var isOnline: Bool
    var fallbackTranscriptRowId = ""
    var historyOnly = false
    var delegateTranscriptGeneration: Int64? = nil
    var fallbackSessionIsLive = false
}

enum ProductConversationDetailPolicy {
    static func delegateTranscriptRowId(
        snapshot: ProductConversationSnapshot,
        historyOnly: Bool
    ) -> String? {
        guard !historyOnly, snapshot.ordinary_lifecycle == .open else { return nil }
        return snapshot.writable_transcript_row_id ?? snapshot.latest_transcript_row_id
    }
}

private struct ProductConversationDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    let productConversationId: String
    let historyOnly: Bool
    let fallbackTranscriptRowId: String
    @State private var cached: CachedProductHistory?
    @State private var error: ProductHistoryLoadError?
    @State private var onlineRefreshSucceeded = false
    @State private var draft = ""
    @State private var acknowledgedProductId: String?
    @State private var readAdvanceInFlight = false
    @State private var readAdvancePending = false
    @State private var readVisitGeneration = 0
    @State private var isVisible = false

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
                    let delegate = delegateSession(for: cached.snapshot)
                    ProductConversationTranscript(
                        snapshot: cached.snapshot,
                        delegate: delegate)
                } else if !historyOnly,
                          !fallbackTranscriptRowId.isEmpty,
                          let session = model.session(for: fallbackTranscriptRowId) {
                    ConversationView(
                        session: session,
                        showsComposer: false,
                        advancesRead: false)
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
            if let session = composerSession {
                ComposerView(session: session, draft: $draft)
            }
        }
        .task(id: ProductHistoryLoadKey(
            productConversationId: productConversationId,
            isOnline: model.connectivity.isOnline,
            fallbackTranscriptRowId: fallbackTranscriptRowId,
            historyOnly: historyOnly,
            delegateTranscriptGeneration: currentDelegateGeneration,
            fallbackSessionIsLive: fallbackSessionIsLive)
        ) {
            if cached == nil,
               let stored = model.cachedProductHistory(productConversationId: productConversationId) {
                cached = stored
            }
            error = nil
            if !model.connectivity.isOnline {
                onlineRefreshSucceeded = false
            }
            do {
                cached = try await model.loadProductHistory(
                    productConversationId: productConversationId)
                onlineRefreshSucceeded = model.connectivity.isOnline
            } catch is CancellationError {
                return
            } catch let loadError as ProductHistoryLoadError {
                onlineRefreshSucceeded = false
                if loadError == .notFound { cached = nil }
                if cached == nil {
                    error = loadError
                }
            } catch {
                onlineRefreshSucceeded = false
                if cached == nil {
                    self.error = .emptyResponse
                }
            }
        }
        .onAppear {
            isVisible = true
            readVisitGeneration += 1
            advanceRead()
        }
        .onDisappear {
            isVisible = false
            readVisitGeneration += 1
            readAdvancePending = false
            acknowledgedProductId = nil
        }
        .onChange(of: productConversationId) { _, _ in
            readVisitGeneration += 1
            readAdvancePending = false
            acknowledgedProductId = nil
            advanceRead()
        }
        .onChange(of: composerSession?.messages.last?.message_id) { _, _ in
            advanceRead()
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active {
                advanceRead()
            }
        }
        .onChange(of: model.connectivity.isOnline) { _, _ in
            advanceRead()
        }
    }

    private var composerSession: ConversationSession? {
        if let cached {
            return delegateSession(for: cached.snapshot)
        }
        guard !historyOnly, !fallbackTranscriptRowId.isEmpty else { return nil }
        return model.session(for: fallbackTranscriptRowId)
    }

    private var fallbackSessionIsLive: Bool {
        guard cached == nil,
              !historyOnly,
              !fallbackTranscriptRowId.isEmpty,
              let session = model.session(for: fallbackTranscriptRowId)
        else { return false }
        return session.connection == .live
    }

    private var currentDelegateGeneration: Int64? {
        guard let cached else { return nil }
        return delegateSession(for: cached.snapshot)?.transcriptGeneration
    }

    private func advanceRead() {
        guard isVisible, scenePhase == .active else { return }
        if readAdvanceInFlight {
            readAdvancePending = true
            return
        }
        readAdvanceInFlight = true
        let visitGeneration = readVisitGeneration
        let productId = productConversationId
        let advance = ProductConversationReadPolicy.advance(
            productConversationId: productId,
            acknowledgedProductId: acknowledgedProductId)
        Task {
            let advanced = await model.advanceRead(
                productConversationId: productId,
                advance: advance)
            readAdvanceInFlight = false
            if isVisible, visitGeneration == readVisitGeneration,
               advanced, advance == .acknowledged
            {
                acknowledgedProductId = productId
            }
            if isVisible, readAdvancePending {
                readAdvancePending = false
                advanceRead()
            }
        }
    }

    private func delegateSession(for snapshot: ProductConversationSnapshot) -> ConversationSession? {
        guard let transcriptRowId = ProductConversationDetailPolicy.delegateTranscriptRowId(
            snapshot: snapshot,
            historyOnly: historyOnly)
        else { return nil }
        return model.session(for: transcriptRowId)
    }

    private func cacheAgeNote(at now: Date) -> String? {
        guard let cached,
              ProductHistoryCachePresentation.shouldShowAge(
                  isOnline: model.connectivity.isOnline,
                  onlineRefreshSucceeded: onlineRefreshSucceeded,
                  fetchedAt: cached.fetchedAt,
                  now: now)
        else { return nil }
        let fetchedAt = cached.fetchedAt
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return "Cached \(formatter.localizedString(for: fetchedAt, relativeTo: now))"
    }
}

/// Unread indicator: solid for new activity, hollow for an explicit
/// "mark unread to revisit later".
struct UnreadDot: View {
    let reason: ProductConversationUnreadReason

    static func label(for reason: ProductConversationUnreadReason) -> String {
        switch reason {
        case .marked: "Marked unread"
        case .newActivity: "New activity"
        }
    }

    var body: some View {
        Group {
            switch reason {
            case .newActivity:
                Circle().fill(Color.accentColor)
            case .marked:
                Circle().strokeBorder(Color.accentColor, lineWidth: 1.5)
            }
        }
        .frame(width: 8, height: 8)
        .accessibilityLabel(Self.label(for: reason))
    }
}

struct ProductConversationTranscript: View {
    @Environment(AppModel.self) private var model
    let snapshot: ProductConversationSnapshot
    let delegate: ConversationSession?

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                List {
                    ForEach(snapshot.segments, id: \.segment_ordinal) { segment in
                        Section(segment.title ?? segment.slug ?? "Conversation") {
                            let messages = messages(for: segment)
                            if messages.isEmpty {
                                Text("No messages")
                                    .foregroundStyle(.secondary)
                            } else {
                                let toolIndex = ConversationSession.toolUseIndex(for: messages)
                                ForEach(messages) { message in
                                    MessageView(message: message, toolIndex: toolIndex)
                                }
                            }
                            if segment.transcript_row_id == delegate?.conversationId,
                               let delegate,
                               !delegate.streamingText.isEmpty {
                                StreamingBubble(text: delegate.streamingText)
                            }
                            if let handoff = segment.handoff {
                                ProductHistoryHandoffView(handoff: handoff)
                            }
                        }
                    }
                    if let delegate {
                        OutboxSection(
                            session: delegate,
                            authoritativeMessageIds: authoritativeMessageIds(for: delegate))
                    }
                    Color.clear.frame(height: 1).id(ScrollTarget.bottom)
                }
                .listStyle(.plain)
                .defaultScrollAnchor(.bottom)
                .onChange(of: renderedMessageCount) {
                    withAnimation(.easeOut(duration: 0.2)) {
                        proxy.scrollTo(ScrollTarget.bottom, anchor: .bottom)
                    }
                }
                .onChange(of: delegate?.streamingText) {
                    proxy.scrollTo(ScrollTarget.bottom, anchor: .bottom)
                }
                .accessibilityIdentifier("product-conversation-history")
            }

            if let delegate {
                StateDetailView(session: delegate)
            }
        }
        .navigationTitle(snapshot.canonical_root.title ?? "Conversation")
        .navigationBarTitleDisplayMode(.inline)
        .alert("Action failed", isPresented: Binding(
            get: { delegate?.lastErrorToast != nil },
            set: { if !$0 { delegate?.clearErrorToast() } }
        )) {
            Button("OK") { delegate?.clearErrorToast() }
        } message: {
            Text(delegate?.lastErrorToast ?? "")
        }
        .onChange(of: delegate?.conversationId, initial: true) { oldId, newId in
            if oldId != newId, let oldId {
                model.session(for: oldId)?.closeView()
            }
            delegate?.start()
        }
        .onDisappear { delegate?.closeView() }
    }

    private enum ScrollTarget: Hashable {
        case bottom
    }

    private var renderedMessageCount: Int {
        snapshot.segments.reduce(into: 0) { count, segment in
            count += messages(for: segment).count
        }
    }

    private func authoritativeMessageIds(for delegate: ConversationSession) -> Set<String> {
        let snapshotIds = snapshot.segments
            .first(where: { $0.transcript_row_id == delegate.conversationId })?
            .messages.map(\.message_id) ?? []
        return Set(snapshotIds).union(delegate.messages.map(\.message_id))
    }

    private func messages(for segment: ProductConversationSegment) -> [Message] {
        guard segment.transcript_row_id == delegate?.conversationId, let delegate else {
            return segment.messages
        }
        return AppModel.composedMessages(
            snapshot: segment.messages,
            snapshotGeneration: segment.transcript_generation,
            live: delegate.messages,
            liveGeneration: delegate.transcriptGeneration)
    }
}
