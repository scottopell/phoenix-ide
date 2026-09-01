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

    /// v1 stores the latest mode and title for each known conversation.
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
        for conversation in conversations where conversation.archived != true {
            result[conversation.aggregateIdentity] = Entry(
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
    func checkAndNotify(
        _ conversations: [Conversation], isCurrent: () -> Bool = { true }
    ) async -> Bool {
        let events = Self.diff(previous: snapshot, current: conversations)
        guard !events.isEmpty else {
            guard !Task.isCancelled, isCurrent() else { return false }
            seed(with: conversations)
            return true
        }

        let center = UNUserNotificationCenter.current()
        var submittedRequestIds: [String] = []
        func removeSubmittedRequests() {
            center.removeDeliveredNotifications(withIdentifiers: submittedRequestIds)
            center.removePendingNotificationRequests(withIdentifiers: submittedRequestIds)
        }
        for event in events {
            guard !Task.isCancelled, isCurrent() else {
                removeSubmittedRequests()
                return false
            }
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
            do {
                try await center.add(request)
                submittedRequestIds.append(request.identifier)
            } catch is CancellationError {
                removeSubmittedRequests()
                return false
            } catch {
                continue
            }
            guard !Task.isCancelled, isCurrent() else {
                removeSubmittedRequests()
                return false
            }
        }
        guard !Task.isCancelled, isCurrent() else {
            removeSubmittedRequests()
            return false
        }
        seed(with: conversations)
        return true
    }

    static func requestAuthorization() async -> Bool {
        (try? await UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound, .badge])) ?? false
    }
}
