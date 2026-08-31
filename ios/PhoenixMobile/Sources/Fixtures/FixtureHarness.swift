#if DEBUG
import SwiftUI

enum FixtureAppLaunch {
    static let flag = "-fixture"

    static func isRequested(in arguments: [String]) -> Bool {
        arguments.contains(flag)
    }

    static func selection(from arguments: [String]) -> FixtureScenario.ID? {
        guard let index = arguments.firstIndex(of: flag), arguments.indices.contains(index + 1) else {
            return nil
        }
        return FixtureScenario.ID(rawValue: arguments[index + 1])
    }
}

struct FixtureConversationScreenModel {
    var title: String
    var isOnline: Bool
    var cacheNote: String?
    var connectionNote: String?
    var storageWarning: String?
    var messages: [Message]
    var toolIndex: [String: ToolUseRef]
    var streamingText: String
    var statePayload: JSONValue
    var presentationMode: String
    var requiresAction: Bool
    var bannerStyle: FixtureBannerStyle
    var bannerTitle: String
    var bannerDetail: String
    var accessibilityID: String

    var showsOfflineBanner: Bool { !isOnline }
}

enum FixtureBannerStyle {
    case info
    case warning
    case error

    var tint: Color {
        switch self {
        case .info: return .blue
        case .warning: return .orange
        case .error: return .red
        }
    }

    var systemImage: String {
        switch self {
        case .info: return "info.circle"
        case .warning: return "exclamationmark.triangle"
        case .error: return "xmark.octagon"
        }
    }
}

struct FixtureScenario: Identifiable {
    enum ID: String, CaseIterable {
        case catalog
        case normal
        case loading
        case empty
        case malformed
        case error
        case offline
        case cached
        case readOnly = "read-only"
    }

    var id: ID
    var title: String
    var summary: String
    var screen: FixtureConversationScreenModel

    static let all: [FixtureScenario] = [
        .catalogScenario,
        .normalScenario,
        .loadingScenario,
        .emptyScenario,
        .malformedScenario,
        .errorScenario,
        .offlineScenario,
        .cachedScenario,
        .readOnlyScenario,
    ]

    static func scenario(for id: ID) -> FixtureScenario {
        all.first(where: { $0.id == id }) ?? .normalScenario
    }
}

struct InvalidFixtureView: View {
    var body: some View {
        ContentUnavailableView(
            "Unknown fixture",
            systemImage: "exclamationmark.triangle",
            description: Text("Pass one of the typed fixture scenario identifiers."))
            .accessibilityIdentifier("fixture.invalid")
    }
}

struct FixtureRootView: View {
    let selection: FixtureScenario.ID

    var body: some View {
        NavigationStack {
            FixtureConversationScreen(scenario: FixtureScenario.scenario(for: selection))
        }
        .accessibilityIdentifier("fixture.root.\(selection.rawValue)")
    }
}

struct FixtureCatalogView: View {
    var body: some View {
        NavigationStack {
            List(FixtureScenario.all) { scenario in
                NavigationLink(value: scenario.id) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(scenario.title)
                            .font(.headline)
                        Text(scenario.summary)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .accessibilityIdentifier("fixture.catalog.\(scenario.id.rawValue)")
            }
            .navigationTitle("Fixtures")
            .navigationDestination(for: FixtureScenario.ID.self) { id in
                FixtureConversationScreen(scenario: FixtureScenario.scenario(for: id))
            }
        }
        .accessibilityIdentifier("fixture.catalog")
    }
}

struct FixtureStateInspection: View {
    let state: ConversationState
    let presentationMode: String
    let requiresAction: Bool

    private var stateType: String {
        switch state {
        case .idle: return "idle"
        case .awaitingLlm: return "awaiting_llm"
        case .llmRequesting: return "llm_requesting"
        case .toolExecuting: return "tool_executing"
        case .awaitingSubAgents: return "awaiting_sub_agents"
        case .awaitingContinuation: return "awaiting_continuation"
        case .awaitingUserResponse: return "awaiting_user_response"
        case .awaitingTaskApproval: return "awaiting_task_approval"
        case .awaitingRecovery: return "awaiting_recovery"
        case .provisioning: return "provisioning"
        case .error: return "error"
        case .creationFailed: return "creation_failed"
        case .contextExhausted: return "context_exhausted"
        case .cancelling: return "cancelling"
        case .cancellingTool: return "cancelling_tool"
        case .cancellingSubAgents: return "cancelling_sub_agents"
        case .terminal: return "terminal"
        case .handedOff: return "handed_off"
        case .other(let type): return type
        case .unknown: return "unknown"
        }
    }

    var body: some View {
        HStack(spacing: 8) {
            StateDot(
                presentationMode: presentationMode,
                requiresAction: requiresAction,
                stateType: stateType)
            Text(stateType.replacingOccurrences(of: "_", with: " "))
                .font(.caption.monospaced())
            Spacer()
            Text(presentationMode.replacingOccurrences(of: "_", with: " "))
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("fixture.stateDetail")
    }
}

struct FixtureConversationScreen: View {
    let scenario: FixtureScenario

    var body: some View {
        let screen = scenario.screen
        VStack(alignment: .leading, spacing: 0) {
            Text("Fixture ready: \(scenario.id.rawValue)")
                .font(.caption2)
                .foregroundStyle(.clear)
                .frame(height: 1)
                .accessibilityIdentifier("fixture.ready.\(scenario.id.rawValue)")
            if screen.showsOfflineBanner {
                    fixtureBanner(
                        title: "Offline — showing cached data, messages will queue",
                        detail: nil,
                        style: .warning)
                        .accessibilityIdentifier("fixture.offlineBanner")
                }
                if let connectionNote = screen.connectionNote {
                    Text(connectionNote)
                        .font(.caption2)
                        .foregroundStyle(.orange)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 3)
                        .background(.thinMaterial)
                        .accessibilityIdentifier("fixture.connectionNote")
                }
                if let cacheNote = screen.cacheNote {
                    Text(cacheNote)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 2)
                        .background(.thinMaterial)
                        .accessibilityIdentifier("fixture.cacheNote")
                }
                if let storageWarning = screen.storageWarning {
                    Label(storageWarning, systemImage: "externaldrive.badge.exclamationmark")
                        .font(.caption2)
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                        .background(.red.gradient)
                        .accessibilityIdentifier("fixture.storageWarning")
                }
                transcript(screen: screen)
                FixtureStateInspection(
                    state: ConversationState.parse(screen.statePayload),
                    presentationMode: screen.presentationMode,
                    requiresAction: screen.requiresAction)
                fixtureStateFamily(screen: screen)
                if case .awaitingTaskApproval(let title, let priority, let plan) =
                    ConversationState.parse(screen.statePayload)
                {
                    TaskApprovalCardBody(
                        title: title,
                        priority: priority,
                        plan: plan,
                        isOnline: screen.isOnline,
                        acceptsActions: false,
                        busy: false,
                        onReject: {},
                        onFeedback: { _ in },
                        onApproveHere: {},
                        onApproveFresh: {})
                }
                if case .awaitingUserResponse(let questions) = ConversationState.parse(screen.statePayload),
                   !questions.isEmpty
                {
                    QuestionCardBody(
                        questions: questions,
                        isOnline: screen.isOnline,
                        acceptsActions: false,
                        busy: false,
                        onAnswer: { _ in },
                        onDismiss: {})
                }
                fixtureBanner(
                    title: screen.bannerTitle,
                    detail: screen.bannerDetail,
                    style: screen.bannerStyle)
                    .accessibilityIdentifier(screen.accessibilityID)
        }
        .navigationTitle(screen.title)
        .navigationBarTitleDisplayMode(.inline)
    }

    @ViewBuilder
    private func fixtureStateFamily(screen: FixtureConversationScreenModel) -> some View {
        switch screen.presentationMode {
        case "working":
            WorkingStateRow {
                Text("Deterministic work in progress")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        case "needs_action":
            NeedsActionStateCard(
                icon: "person.crop.circle.badge.exclamationmark",
                title: "Action needed",
                detail: "Deterministic fixture state",
                footnote: screen.isOnline
                    ? "Actions are disabled in fixture mode."
                    : "Offline — actions need a connection and are never queued.")
        case "error":
            ErrorStateCard(
                message: "Server returned malformed tool output.",
                dismissible: true,
                dismissDisabled: true,
                onDismiss: {})
        default:
            EmptyView()
        }
    }

    @ViewBuilder
    private func transcript(screen: FixtureConversationScreenModel) -> some View {
        if screen.messages.isEmpty && screen.streamingText.isEmpty {
            ContentUnavailableView {
                Label("No messages yet", systemImage: "text.bubble")
            } description: {
                Text("This fixture isolates the empty conversation shell.")
            }
            .frame(maxWidth: .infinity)
            .frame(minHeight: 320)
            .accessibilityIdentifier("fixture.emptyTranscript")
        } else {
            ConversationTranscriptView(
                messages: screen.messages,
                toolIndex: screen.toolIndex,
                streamingText: screen.streamingText,
                outboxSession: nil)
                .frame(minHeight: 320)
        }
    }

    private func fixtureBanner(title: String, detail: String?, style: FixtureBannerStyle) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: style.systemImage)
                .font(.callout.bold())
                .foregroundStyle(style.tint)
            if let detail {
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(style.tint.opacity(0.08))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(style.tint.opacity(0.35), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }
}

private extension FixtureScenario {
    static var catalogScenario: FixtureScenario {
        FixtureScenario(
            id: .catalog,
            title: "Catalog",
            summary: "Fixture launcher index.",
            screen: .init(
                title: "Catalog",
                isOnline: true,
                cacheNote: nil,
                connectionNote: nil,
                storageWarning: nil,
                messages: [],
                toolIndex: [:],
                streamingText: "",
                statePayload: .string("idle"),
                presentationMode: "idle",
                requiresAction: false,
                bannerStyle: .info,
                bannerTitle: "Catalog",
                bannerDetail: "Launches one deterministic fixture at a time.",
                accessibilityID: "fixture.footer.catalog"))
    }

    static var normalScenario: FixtureScenario {
        FixtureScenario(
            id: .normal,
            title: "Normal",
            summary: "Live-looking transcript with markdown, think, bash, and a read-only footer.",
            screen: .init(
                title: "Normal",
                isOnline: true,
                cacheNote: nil,
                connectionNote: nil,
                storageWarning: nil,
                messages: baseMessages,
                toolIndex: baseToolIndex,
                streamingText: "Streaming tail with fixed timing…",
                statePayload: .object([
                    "type": .string("tool_executing"),
                    "tool_name": .string("bash"),
                    "remaining_count": .number(1),
                    "completed_count": .number(2),
                ]),
                presentationMode: "working",
                requiresAction: false,
                bannerStyle: .info,
                bannerTitle: "Fixture transcript is deterministic",
                bannerDetail: "No session, network, persistence, or timers start in fixture mode.",
                accessibilityID: "fixture.footer.normal"))
    }

    static var loadingScenario: FixtureScenario {
        FixtureScenario(
            id: .loading,
            title: "Loading",
            summary: "Conversation shell during connection and initial agent work.",
            screen: .init(
                title: "Loading",
                isOnline: true,
                cacheNote: nil,
                connectionNote: "Connecting…",
                storageWarning: nil,
                messages: [baseMessages[0]],
                toolIndex: [:],
                streamingText: "Synthesizing fixture response…",
                statePayload: .object([
                    "type": .string("llm_requesting"),
                    "attempt": .number(2),
                ]),
                presentationMode: "working",
                requiresAction: false,
                bannerStyle: .info,
                bannerTitle: "Loading fixture",
                bannerDetail: "Shows the progress-only shell before authoritative history arrives.",
                accessibilityID: "fixture.footer.loading"))
    }

    static var emptyScenario: FixtureScenario {
        FixtureScenario(
            id: .empty,
            title: "Empty",
            summary: "No transcript yet; shell and footer remain explicit.",
            screen: .init(
                title: "Empty",
                isOnline: true,
                cacheNote: nil,
                connectionNote: nil,
                storageWarning: nil,
                messages: [],
                toolIndex: [:],
                streamingText: "",
                statePayload: .string("idle"),
                presentationMode: "idle",
                requiresAction: false,
                bannerStyle: .info,
                bannerTitle: "Empty fixture",
                bannerDetail: "Use this to regress blank-state spacing and copy.",
                accessibilityID: "fixture.footer.empty"))
    }

    static var malformedScenario: FixtureScenario {
        FixtureScenario(
            id: .malformed,
            title: "Malformed",
            summary: "Unknown blocks and generic tool fallbacks degrade visibly.",
            screen: .init(
                title: "Malformed",
                isOnline: true,
                cacheNote: nil,
                connectionNote: nil,
                storageWarning: nil,
                messages: malformedMessages,
                toolIndex: malformedToolIndex,
                streamingText: "",
                statePayload: .object(["type": .string("brand_new_state")]),
                presentationMode: "needs_action",
                requiresAction: true,
                bannerStyle: .warning,
                bannerTitle: "Malformed fixture",
                bannerDetail: "Exercises compact JSON fallbacks instead of silent omission.",
                accessibilityID: "fixture.footer.malformed"))
    }

    static var errorScenario: FixtureScenario {
        FixtureScenario(
            id: .error,
            title: "Error",
            summary: "Conversation error surface with a failed bash result and dismiss affordance.",
            screen: .init(
                title: "Error",
                isOnline: true,
                cacheNote: nil,
                connectionNote: nil,
                storageWarning: "Storage write failed — queued messages may not survive a restart",
                messages: errorMessages,
                toolIndex: errorToolIndex,
                streamingText: "",
                statePayload: .object([
                    "type": .string("error"),
                    "message": .string("Server returned malformed tool output."),
                    "error_kind": .string("invalid_response"),
                ]),
                presentationMode: "error",
                requiresAction: false,
                bannerStyle: .error,
                bannerTitle: "Error fixture",
                bannerDetail: "Uses the real error card copy and failed tool result styling.",
                accessibilityID: "fixture.footer.error"))
    }

    static var offlineScenario: FixtureScenario {
        FixtureScenario(
            id: .offline,
            title: "Offline",
            summary: "Offline banner, cached transcript, and disabled question/resolution surface.",
            screen: .init(
                title: "Offline",
                isOnline: false,
                cacheNote: "Cached 8m ago",
                connectionNote: nil,
                storageWarning: nil,
                messages: baseMessages,
                toolIndex: baseToolIndex,
                streamingText: "",
                statePayload: .object([
                    "type": .string("awaiting_user_response"),
                    "questions": .array([
                        .object([
                            "question": .string("Which fixture state should ship first?"),
                            "header": .string("Prioritization"),
                            "options": .array([
                                .object([
                                    "label": .string("Normal"),
                                    "description": .string("Happy-path deterministic transcript"),
                                ]),
                                .object([
                                    "label": .string("Offline"),
                                    "description": .string("Cached transcript with disabled actions"),
                                ]),
                            ]),
                            "multiSelect": .bool(false),
                        ]),
                    ]),
                ]),
                presentationMode: "needs_action",
                requiresAction: true,
                bannerStyle: .warning,
                bannerTitle: "Offline fixture",
                bannerDetail: "Shows read-only needs-action copy when the device has no network path.",
                accessibilityID: "fixture.footer.offline"))
    }

    static var cachedScenario: FixtureScenario {
        FixtureScenario(
            id: .cached,
            title: "Cached",
            summary: "Warm cached transcript with an explicit staleness note.",
            screen: .init(
                title: "Cached",
                isOnline: true,
                cacheNote: "Cached 2h ago",
                connectionNote: "Connection lost — reconnecting…",
                storageWarning: nil,
                messages: baseMessages,
                toolIndex: baseToolIndex,
                streamingText: "",
                statePayload: .object([
                    "type": .string("awaiting_recovery"),
                    "message": .string("Replaying deterministic event log…"),
                ]),
                presentationMode: "working",
                requiresAction: false,
                bannerStyle: .info,
                bannerTitle: "Cached fixture",
                bannerDetail: "Use this to validate stale-data affordances without a live SSE stream.",
                accessibilityID: "fixture.footer.cached"))
    }

    static var readOnlyScenario: FixtureScenario {
        FixtureScenario(
            id: .readOnly,
            title: "Read-only",
            summary: "Archived/done visual stance without editable controls.",
            screen: .init(
                title: "Read-only",
                isOnline: true,
                cacheNote: nil,
                connectionNote: nil,
                storageWarning: nil,
                messages: baseMessages,
                toolIndex: baseToolIndex,
                streamingText: "",
                statePayload: .object([
                    "type": .string("awaiting_task_approval"),
                    "title": .string("Inspect deterministic fixture harness"),
                    "priority": .string("p2"),
                    "plan": .string("Exercise the real native task approval presentation without a live session."),
                ]),
                presentationMode: "needs_action",
                requiresAction: true,
                bannerStyle: .info,
                bannerTitle: "Read-only fixture",
                bannerDetail: "Composer is intentionally absent; transcript and state visuals stay readable.",
                accessibilityID: "fixture.footer.readOnly"))
    }

    static var baseMessages: [Message] {
        [
            .init(
                message_id: "m-user-1",
                conversation_id: "fixture-conv",
                sequence_id: 1,
                message_type: "user",
                content: .object([
                    "text": .string("Please review this **deterministic** fixture run."),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:04:05Z"),
            .init(
                message_id: "m-agent-1",
                conversation_id: "fixture-conv",
                sequence_id: 2,
                message_type: "agent",
                content: .array([
                    .object([
                        "type": .string("text"),
                        "text": .string("Here is real Markdown:\n\n- item one\n- item two\n- `code`"),
                    ]),
                    .object([
                        "type": .string("tool_use"),
                        "id": .string("tool-think-1"),
                        "name": .string("think"),
                        "input": .object([
                            "thoughts": .string("Reason through the review before changing code."),
                        ]),
                    ]),
                    .object([
                        "type": .string("tool_use"),
                        "id": .string("tool-bash-1"),
                        "name": .string("bash"),
                        "display": .string("swift test --filter FixtureHarness"),
                        "input": .object([
                            "op": .string("run"),
                            "cmd": .string("swift test --filter FixtureHarness"),
                            "label": .string("fixture-validation"),
                        ]),
                    ]),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:04:15Z"),
            .init(
                message_id: "m-tool-1",
                conversation_id: "fixture-conv",
                sequence_id: 3,
                message_type: "tool",
                content: .object([
                    "tool_use_id": .string("tool-bash-1"),
                    "content": .string(#"{"status":"exited","exit_code":0,"duration_ms":420,"lines":[{"bytes":"Fixture harness validated"},{"bytes":"8 scenarios loaded"}]}"#),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:04:16Z"),
        ]
    }

    static var baseToolIndex: [String: ToolUseRef] {
        [
            "tool-think-1": .init(name: "think", input: .object([
                "thoughts": .string("Reason through the review before changing code."),
            ])),
            "tool-bash-1": .init(name: "bash", input: .object([
                "op": .string("run"),
                "cmd": .string("swift test --filter FixtureHarness"),
                "label": .string("fixture-validation"),
            ])),
        ]
    }

    static var malformedMessages: [Message] {
        [
            baseMessages[0],
            .init(
                message_id: "m-agent-malformed",
                conversation_id: "fixture-conv",
                sequence_id: 2,
                message_type: "agent",
                content: .array([
                    .object([
                        "type": .string("future_block"),
                        "payload": .object(["alpha": .number(1)]),
                    ]),
                    .object([
                        "type": .string("tool_use"),
                        "id": .string("tool-unknown-1"),
                        "name": .string("future_tool"),
                        "input": .object(["alpha": .string("beta")]),
                    ]),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:05:00Z"),
            .init(
                message_id: "m-tool-malformed",
                conversation_id: "fixture-conv",
                sequence_id: 3,
                message_type: "tool",
                content: .object([
                    "tool_use_id": .string("tool-unknown-1"),
                    "content": .string("raw fallback output\nsecond line"),
                    "is_error": .bool(true),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:05:01Z"),
        ]
    }

    static var malformedToolIndex: [String: ToolUseRef] {
        [
            "tool-unknown-1": .init(name: "future_tool", input: .object(["alpha": .string("beta")]))
        ]
    }

    static var errorMessages: [Message] {
        [
            baseMessages[0],
            .init(
                message_id: "m-agent-error",
                conversation_id: "fixture-conv",
                sequence_id: 2,
                message_type: "agent",
                content: .array([
                    .object([
                        "type": .string("text"),
                        "text": .string("The tool failed; inspect the real error surface below."),
                    ]),
                    .object([
                        "type": .string("tool_use"),
                        "id": .string("tool-bash-error"),
                        "name": .string("bash"),
                        "display": .string("cargo test fixture_harness"),
                        "input": .object([
                            "op": .string("run"),
                            "cmd": .string("cargo test fixture_harness"),
                        ]),
                    ]),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:06:00Z"),
            .init(
                message_id: "m-tool-error",
                conversation_id: "fixture-conv",
                sequence_id: 3,
                message_type: "tool",
                content: .object([
                    "tool_use_id": .string("tool-bash-error"),
                    "content": .string(#"{"status":"tombstoned","final_cause":"killed","signal_number":15,"lines":[{"bytes":"Compiling fixture harness…"}],"error_message":"Timed out waiting for fixture output"}"#),
                    "is_error": .bool(true),
                ]),
                display_data: nil,
                created_at: "2025-01-02T03:06:01Z"),
        ]
    }

    static var errorToolIndex: [String: ToolUseRef] {
        [
            "tool-bash-error": .init(name: "bash", input: .object([
                "op": .string("run"),
                "cmd": .string("cargo test fixture_harness"),
            ])),
        ]
    }

}
#endif
