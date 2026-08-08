import Foundation
import UserNotifications

/// Best-effort local nudges driven by opportunistic background fetches
/// (BGAppRefresh, OS-controlled cadence, typically ≥15 min and never
/// guaranteed). Diffs the freshly fetched conversation list against the
/// last-seen snapshot and fires a local notification for conversations that
/// newly need the user (needs_action, error) or just finished a turn.
/// Foreground refreshes re-seed the snapshot silently so the user is never
/// nudged about things they already saw.
@MainActor
final class AttentionMonitor {
    struct Entry: Codable, Equatable {
        var mode: String
        var title: String
    }

    enum Event: Equatable {
        case needsAction(conversationId: String, title: String)
        case errored(conversationId: String, title: String)
        case finished(conversationId: String, title: String)

        var conversationId: String {
            switch self {
            case .needsAction(let id, _), .errored(let id, _), .finished(let id, _):
                return id
            }
        }
    }

    /// DiskStore versioning rule applies (see DiskStore header).
    private static let schemaVersion = 1
    private static let storeName = "attention-snapshot"

    private(set) var snapshot: [String: Entry]

    init() {
        snapshot = DiskStore.loadVersioned(
            [String: Entry].self, name: Self.storeName, version: Self.schemaVersion) ?? [:]
    }

    // MARK: - Pure contract (tested)

    static func entries(from conversations: [Conversation]) -> [String: Entry] {
        var result: [String: Entry] = [:]
        for conversation in conversations {
            result[conversation.id] = Entry(
                mode: conversation.presentation_mode ?? "",
                title: conversation.displayTitle)
        }
        return result
    }

    /// Transitions worth a nudge. Rules:
    /// - a conversation absent from `previous` never notifies (first sight
    ///   seeds silently — no burst on first run or fresh installs);
    /// - entering needs_action or error notifies once per entry;
    /// - working -> idle/done notifies "finished" (a turn the user was
    ///   presumably waiting on completed).
    static func diff(previous: [String: Entry], current: [Conversation]) -> [Event] {
        var events: [Event] = []
        for conversation in current {
            guard let before = previous[conversation.id] else { continue }
            let mode = conversation.presentation_mode ?? ""
            let title = conversation.displayTitle
            if mode == "needs_action", before.mode != "needs_action" {
                events.append(.needsAction(conversationId: conversation.id, title: title))
            } else if mode == "error", before.mode != "error" {
                events.append(.errored(conversationId: conversation.id, title: title))
            } else if (mode == "idle" || mode == "done"), before.mode == "working" {
                events.append(.finished(conversationId: conversation.id, title: title))
            }
        }
        return events
    }

    // MARK: - Snapshot lifecycle

    /// Update the last-seen snapshot without notifying — for foreground
    /// refreshes, where the user is already looking at the list.
    func seed(with conversations: [Conversation]) {
        snapshot = Self.entries(from: conversations)
        DiskStore.saveVersioned(snapshot, name: Self.storeName, version: Self.schemaVersion)
    }

    func reset() {
        snapshot = [:]
        DiskStore.remove(name: Self.storeName)
    }

    /// Background path: diff, notify, then seed. One notification per
    /// conversation, keyed by conversation id so a newer nudge replaces a
    /// stale one instead of stacking.
    func checkAndNotify(_ conversations: [Conversation]) {
        let events = Self.diff(previous: snapshot, current: conversations)
        seed(with: conversations)
        guard !events.isEmpty else { return }

        let center = UNUserNotificationCenter.current()
        for event in events {
            let content = UNMutableNotificationContent()
            switch event {
            case .needsAction(_, let title):
                content.title = "Agent needs your attention"
                content.body = title
            case .errored(_, let title):
                content.title = "Agent hit an error"
                content.body = title
            case .finished(_, let title):
                content.title = "Agent finished"
                content.body = title
            }
            content.sound = .default
            content.userInfo = ["conversationId": event.conversationId]
            let request = UNNotificationRequest(
                identifier: "attention-\(event.conversationId)",
                content: content,
                trigger: nil)
            center.add(request) { _ in }
        }
    }

    static func requestAuthorization() async -> Bool {
        (try? await UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound, .badge])) ?? false
    }
}
