import Foundation
import UserNotifications

/// Pure logic + side-effect wrapper for background attention nudges.
@MainActor
final class AttentionMonitor {
    struct Entry: Codable, Equatable {
        let mode: String
        let title: String
    }

    enum Event: Equatable {
        case needsAction(aggregateId: String, transcriptRowId: String, title: String)
        case errored(aggregateId: String, transcriptRowId: String, title: String)
        case finished(aggregateId: String, transcriptRowId: String, title: String)

        var aggregateId: String {
            switch self {
            case .needsAction(let id, _, _), .errored(let id, _, _), .finished(let id, _, _):
                return id
            }
        }

        var transcriptRowId: String {
            switch self {
            case .needsAction(_, let id, _), .errored(_, let id, _), .finished(_, let id, _):
                return id
            }
        }
    }

    private static let schemaVersion = 2
    private static let storeName = "attention-snapshot"

    private struct Store: Codable, Equatable {
        var aggregateSnapshot: [String: Entry]
        var quarantinedLegacyTranscriptSnapshot: [String: Entry]
    }

    private(set) var snapshot: [String: Entry]
    private var quarantinedLegacyTranscriptSnapshot: [String: Entry]

    init(currentConversations: [Conversation] = [], transcriptToAggregate: [String: String] = [:]) {
        if let current = DiskStore.loadVersioned(
            Store.self, name: Self.storeName, version: Self.schemaVersion)
        {
            snapshot = current.aggregateSnapshot
            quarantinedLegacyTranscriptSnapshot = current.quarantinedLegacyTranscriptSnapshot
            return
        }
        let legacy = DiskStore.loadVersioned(
            [String: Entry].self, name: Self.storeName, version: 1) ?? [:]
        let remapped = Self.remapLegacySnapshot(
            legacy,
            currentConversations: currentConversations,
            transcriptToAggregate: transcriptToAggregate)
        snapshot = remapped.resolved
        quarantinedLegacyTranscriptSnapshot = remapped.unresolved
        persist()
    }

    // MARK: - Pure contract (tested)

    static func entries(from conversations: [Conversation]) -> [String: Entry] {
        var result: [String: Entry] = [:]
        for conversation in conversations where conversation.archived != true {
            result[conversation.aggregateIdentity] = Entry(
                mode: conversation.presentation_mode ?? "",
                title: conversation.displayTitle)
        }
        return result
    }

    static func remapLegacySnapshot(
        _ legacy: [String: Entry],
        currentConversations: [Conversation],
        transcriptToAggregate: [String: String]
    ) -> (resolved: [String: Entry], unresolved: [String: Entry]) {
        guard !legacy.isEmpty else { return ([:], [:]) }
        var authoritativeMap = transcriptToAggregate
        for conversation in currentConversations {
            authoritativeMap[conversation.transcriptRowIdentity] = conversation.aggregateIdentity
        }
        var resolved: [String: Entry] = [:]
        var unresolved: [String: Entry] = [:]
        for (legacyTranscriptId, entry) in legacy {
            if let key = authoritativeMap[legacyTranscriptId] {
                resolved[key] = entry
            } else {
                unresolved[legacyTranscriptId] = entry
            }
        }
        return (resolved, unresolved)
    }

    /// Transitions worth a nudge. Rules:
    /// - a conversation absent from `previous` never notifies (first sight
    ///   seeds silently — no burst on first run or fresh installs);
    /// - entering `needs_action` notifies;
    /// - entering `error` notifies;
    /// - `working -> idle|done` notifies (covers agent finished and user
    ///   presumably waiting on completed).
    static func diff(previous: [String: Entry], current: [Conversation]) -> [Event] {
        var events: [Event] = []
        for conversation in current where conversation.archived != true {
            guard let before = previous[conversation.aggregateIdentity] else { continue }
            let mode = conversation.presentation_mode ?? ""
            let title = conversation.displayTitle
            if mode == "needs_action", before.mode != "needs_action" {
                events.append(
                    .needsAction(
                        aggregateId: conversation.aggregateIdentity,
                        transcriptRowId: conversation.transcriptRowIdentity,
                        title: title))
            } else if mode == "error", before.mode != "error" {
                events.append(
                    .errored(
                        aggregateId: conversation.aggregateIdentity,
                        transcriptRowId: conversation.transcriptRowIdentity,
                        title: title))
            } else if (mode == "idle" || mode == "done"), before.mode == "working" {
                events.append(
                    .finished(
                        aggregateId: conversation.aggregateIdentity,
                        transcriptRowId: conversation.transcriptRowIdentity,
                        title: title))
            }
        }
        return events
    }
    static func requestAuthorization() async -> Bool {
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral:
            return true
        case .notDetermined:
            return (try? await center.requestAuthorization(options: [.alert, .badge, .sound])) == true
        case .denied:
            return false
        @unknown default:
            return false
        }
    }

    func checkAndNotify(
        _ conversations: [Conversation],
        enabled: @escaping @MainActor () -> Bool
    ) async -> Bool {
        let events = Self.diff(previous: snapshot, current: conversations)
        await refreshAndNotifyIfNeeded(
            from: conversations,
            transcriptToAggregate: [:],
            isCurrent: enabled)
        return !events.isEmpty
    }

    func seed(with conversations: [Conversation], transcriptToAggregate: [String: String] = [:]) {
        snapshot = Self.entries(from: conversations)
        persist()
    }

    /// Refresh using the latest list and emit notifications for transitions.
    /// No-op unless the user enabled background nudges.
    func refreshAndNotifyIfNeeded(
        from conversations: [Conversation],
        transcriptToAggregate: [String: String] = [:],
        isCurrent: @escaping @MainActor () -> Bool
    ) async {
        let current = Self.entries(from: conversations)
        let events = Self.diff(previous: snapshot, current: conversations)
        guard await isCurrent() else { return }
        snapshot = current
        persist()
        guard await isCurrent() else { return }

        let center = UNUserNotificationCenter.current()
        for event in events {
            guard await isCurrent() else { return }
            let settings = await center.notificationSettings()
            guard await isCurrent() else { return }
            guard settings.authorizationStatus == .authorized else { return }
            let content = UNMutableNotificationContent()
            switch event {
            case .needsAction(_, _, let title):
                content.title = "Agent needs your attention"
                content.body = title
            case .errored(_, _, let title):
                content.title = "Agent hit an error"
                content.body = title
            case .finished(_, _, let title):
                content.title = "Agent finished"
                content.body = title
            }
            content.sound = .default
            content.userInfo = ["conversationId": event.transcriptRowId]
            let request = UNNotificationRequest(
                identifier: "attention-\(event.aggregateId)",
                content: content,
                trigger: nil)
            try? await center.add(request)
            guard await isCurrent() else {
                center.removeDeliveredNotifications(withIdentifiers: [request.identifier])
                center.removePendingNotificationRequests(withIdentifiers: [request.identifier])
                return
            }
        }
    }

    func reset() {
        snapshot = [:]
        quarantinedLegacyTranscriptSnapshot = [:]
        DiskStore.remove(name: Self.storeName)
    }

    private func persist() {
        DiskStore.saveVersioned(
            Store(
                aggregateSnapshot: snapshot,
                quarantinedLegacyTranscriptSnapshot: quarantinedLegacyTranscriptSnapshot),
            name: Self.storeName,
            version: Self.schemaVersion)
    }
}
