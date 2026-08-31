import SwiftUI

// State-detail rendering falls into three visual families: working detail,
// needs-action cards, and error cards.

struct WorkingStateRow<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            content()
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .accessibilityIdentifier("state.working")
    }
}

struct NeedsActionStateCard: View {
    let icon: String
    let title: String
    let detail: String?
    let footnote: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: icon)
                .font(.callout.bold())
            if let detail {
                Text(detail)
                    .font(.callout)
                    .lineLimit(3)
            }
            Text(footnote)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.blue.opacity(0.1))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(Color.blue.opacity(0.35), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .accessibilityIdentifier("state.needsAction")
    }
}

struct ErrorStateCard: View {
    let message: String
    let dismissible: Bool
    let dismissDisabled: Bool
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Agent error", systemImage: "exclamationmark.triangle.fill")
                .font(.callout.bold())
                .foregroundStyle(.red)
            Text(message)
                .font(.callout)
                .lineLimit(4)
            if dismissible {
                HStack {
                    Spacer()
                    Button("Dismiss error", action: onDismiss)
                        .font(.callout.bold())
                        .disabled(dismissDisabled)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.red.opacity(0.08))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(Color.red.opacity(0.35), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .accessibilityIdentifier("state.error")
    }
}

/// Rendered between the transcript and the composer. Quiet when idle.
struct StateDetailView: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession

    var body: some View {
        StateDetailBody(
            state: session.typedState,
            presentationMode: session.presentationMode ?? "idle",
            agentWorking: session.agentWorking,
            isOnline: model.connectivity.isOnline,
            acceptsActions: session.acceptsConversationActions,
            busy: session.actionInFlight != nil,
            convState: session.convState,
            onAction: { session.perform($0) })
    }
}

struct StateDetailBody: View {
    let state: ConversationState
    let presentationMode: String
    let agentWorking: Bool
    let isOnline: Bool
    let acceptsActions: Bool
    let busy: Bool
    let convState: JSONValue?
    let onAction: (ConversationAction) -> Void
    @State private var confirmEmptyQuestionDismissal = false

    var body: some View {
        switch state {
        case .toolExecuting(let toolName, let remaining, let completed):
            workingRow {
                HStack(spacing: 4) {
                    Text(toolName)
                        .font(.caption.monospaced().bold())
                    // Queue depth per REQ-API-011: evidence of progress.
                    Text(progressSuffix(remaining: remaining, completed: completed))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

        case .llmRequesting(let attempt):
            workingRow {
                Text(attempt > 1 ? "Thinking… (attempt \(attempt))" : "Thinking…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .awaitingSubAgents(let pending, let completed):
            workingRow {
                Text("Sub-agents: \(completed) done, \(pending) running")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .awaitingUserResponse(let questions):
            if questions.isEmpty {
                emptyQuestionCard
            } else {
                QuestionCardBody(
                    questions: questions,
                    isOnline: isOnline,
                    acceptsActions: acceptsActions,
                    busy: busy,
                    onAnswer: { onAction(.respondToQuestions(answers: $0)) },
                    onDismiss: { onAction(.dismissQuestion) })
                    .id(questions)
            }

        case .awaitingTaskApproval(let title, let priority, let plan):
            TaskApprovalCardBody(
                title: title,
                priority: priority,
                plan: plan,
                isOnline: isOnline,
                acceptsActions: acceptsActions,
                busy: busy,
                onReject: { onAction(.rejectTask) },
                onFeedback: { onAction(.provideTaskFeedback($0)) },
                onApproveHere: {
                    onAction(.approveTask(handoff: .continueInCurrentConversation))
                },
                onApproveFresh: {
                    onAction(.approveTask(handoff: .startFreshWorkConversation))
                })

        case .awaitingRecovery(let message):
            workingRow {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .provisioning:
            workingRow {
                Text("Preparing conversation…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .error(let message, _):
            errorCard(message: message)

        case .creationFailed(let message):
            errorCard(message: message, dismissible: false)

        case .contextExhausted(let summary):
            if presentationMode != "done" {
                needsActionCard(
                    icon: "arrow.triangle.2.circlepath",
                    title: "Context exhausted",
                    detail: summary,
                    footnote: "Continue this work from the web UI.")
            }

        case .cancelling, .cancellingTool, .cancellingSubAgents:
            workingRow {
                Text("Cancelling…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .other(let type):
            fallbackState(type: type)

        case .unknown:
            fallbackState(type: "Unknown state")

        case .handedOff(let successorConversationId):
            if let successorConversationId {
                NavigationLink(value: successorConversationId) {
                    Label("Open new conversation", systemImage: "arrow.right.circle")
                }
                .font(.callout.bold())
            }

        case .idle, .awaitingLlm, .awaitingContinuation, .terminal:
            if agentWorking {
                workingRow {
                    Text("Working…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    nonisolated static func fallbackErrorMessage(type: String, state: JSONValue?) -> String {
        state?["message"]?.stringValue
            ?? state?["error"]?.stringValue
            ?? state?["failure"]?["message"]?.stringValue
            ?? type.replacingOccurrences(of: "_", with: " ")
    }

    @ViewBuilder
    private func fallbackState(type: String) -> some View {
        if presentationMode == "error" {
            errorCard(
                message: Self.fallbackErrorMessage(type: type, state: convState),
                dismissible: false)
        } else if presentationMode == "needs_action" {
            needsActionCard(
                icon: "person.crop.circle.badge.exclamationmark",
                title: "Action needed",
                detail: type.replacingOccurrences(of: "_", with: " "),
                footnote: "Handle this from the web UI.")
        } else if agentWorking {
            workingRow {
                Text(type.replacingOccurrences(of: "_", with: " "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func progressSuffix(remaining: Int, completed: Int) -> String {
        var parts: [String] = []
        if completed > 0 { parts.append("\(completed) done") }
        if remaining > 0 { parts.append("\(remaining) queued") }
        return parts.isEmpty ? "running" : parts.joined(separator: " · ")
    }

    private func workingRow(@ViewBuilder _ content: @escaping () -> some View) -> some View {
        WorkingStateRow(content: content)
    }

    private func needsActionCard(
        icon: String, title: String, detail: String?, footnote: String
    ) -> some View {
        NeedsActionStateCard(
            icon: icon, title: title, detail: detail, footnote: footnote)
    }

    private func errorCard(message: String, dismissible: Bool = true) -> some View {
        ErrorStateCard(
            message: message,
            dismissible: dismissible,
            dismissDisabled: !isOnline
                || !acceptsActions
                || busy,
            onDismiss: { onAction(.dismissError) })
    }

    private var emptyQuestionCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("The agent is waiting for a response", systemImage: "questionmark.bubble")
                .font(.callout.bold())
            Text("The question payload is empty. Dismiss it to unblock the conversation.")
                .font(.caption2)
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Dismiss question") {
                    confirmEmptyQuestionDismissal = true
                }
                .font(.callout.bold())
                .disabled(!isOnline || busy)
            }
            if !isOnline {
                Text("Offline — dismissal needs a connection and is never queued.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.blue.opacity(0.1))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(Color.blue.opacity(0.35), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .confirmationDialog(
            "Dismiss this unanswered prompt?",
            isPresented: $confirmEmptyQuestionDismissal,
            titleVisibility: .visible
        ) {
            Button("Dismiss question", role: .destructive) {
                onAction(.dismissQuestion)
            }
        }
    }
}

/// Reviews and resolves a proposed task plan.
struct TaskApprovalCard: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession
    let title: String
    let priority: String
    let plan: String

    var body: some View {
        TaskApprovalCardBody(
            title: title,
            priority: priority,
            plan: plan,
            isOnline: model.connectivity.isOnline,
            acceptsActions: session.acceptsConversationActions,
            busy: session.actionInFlight != nil,
            onReject: { session.perform(.rejectTask) },
            onFeedback: { session.perform(.provideTaskFeedback($0)) },
            onApproveHere: {
                session.perform(.approveTask(handoff: .continueInCurrentConversation))
            },
            onApproveFresh: {
                session.perform(.approveTask(handoff: .startFreshWorkConversation))
            })
    }
}

struct TaskApprovalCardBody: View {
    let title: String
    let priority: String
    let plan: String
    let isOnline: Bool
    let acceptsActions: Bool
    let busy: Bool
    let onReject: () -> Void
    let onFeedback: (TaskFeedback) -> Void
    let onApproveHere: () -> Void
    let onApproveFresh: () -> Void

    @State private var planExpanded = false
    @State private var showFeedbackField = false
    @State private var feedbackText = ""
    @State private var confirmReject = false

    private var actionable: Bool { isOnline && acceptsActions && !busy }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Label("Task plan awaiting approval", systemImage: "checklist")
                    .font(.callout.bold())
                Spacer()
                if busy {
                    ProgressView().controlSize(.small)
                }
            }

            HStack(spacing: 6) {
                Text(title.isEmpty ? "(untitled task)" : title)
                    .font(.callout)
                    .lineLimit(2)
                if !priority.isEmpty {
                    Text(priority)
                        .font(.caption2.monospaced().bold())
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Color.blue.opacity(0.15))
                        .clipShape(Capsule())
                }
            }

            if !plan.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text(plan)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(planExpanded ? nil : 4)
                        .textSelection(.enabled)
                    Button(planExpanded ? "Show less" : "Show full plan") {
                        withAnimation(.easeInOut(duration: 0.15)) {
                            planExpanded.toggle()
                        }
                    }
                    .font(.caption.bold())
                }
            }

            if showFeedbackField {
                TextField("What should change?", text: $feedbackText, axis: .vertical)
                    .lineLimit(2...5)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout)
            }

            HStack(spacing: 10) {
                Button("Reject", role: .destructive) {
                    confirmReject = true
                }
                .disabled(!actionable)

                Button(showFeedbackField ? "Send changes" : "Request changes") {
                    if showFeedbackField {
                        guard let feedback = TaskFeedback(feedbackText) else { return }
                        onFeedback(feedback)
                    } else {
                        withAnimation(.easeInOut(duration: 0.15)) {
                            showFeedbackField = true
                        }
                    }
                }
                .disabled(!actionable
                    || (showFeedbackField
                        && feedbackText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty))

                Spacer()

                Menu {
                    Button("Start here", action: onApproveHere)
                    Button("New chat", action: onApproveFresh)
                } label: {
                    Label("Approve…", systemImage: "checkmark")
                }
                .buttonStyle(.borderedProminent)
                .disabled(!actionable)
            }
            .font(.callout)

            if !isOnline {
                Text("Offline — approval needs a connection and is never queued.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.blue.opacity(0.1))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(Color.blue.opacity(0.35), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .accessibilityIdentifier("state.taskApprovalCard")
        .confirmationDialog(
            "Reject this task plan?",
            isPresented: $confirmReject, titleVisibility: .visible
        ) {
            Button("Reject plan", role: .destructive, action: onReject)
        }
    }
}
