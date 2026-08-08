import SwiftUI

// State-detail rendering: the dispatch view for the typed ConversationState.
// Same extension recipe as the tool renderers — add a case here when a
// variant graduates from the generic fallbacks (see ConversationState's
// doc comment for the full checklist). Cases fall into three visual
// families: working detail (inline, quiet), needs-action cards (blue,
// prominent), and the error card (red, with its dismiss action).

/// Rendered between the transcript and the composer. Quiet when idle.
struct StateDetailView: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession

    var body: some View {
        switch session.typedState {
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

        case .awaitingUserResponse(let count, let firstQuestion):
            needsActionCard(
                icon: "questionmark.bubble",
                title: count == 1
                    ? "The agent asked a question"
                    : "The agent asked \(count) questions",
                detail: firstQuestion,
                footnote: "Answer from the web UI — responding here isn't supported yet.")

        case .awaitingTaskApproval(let title, let priority, let plan):
            TaskApprovalCard(
                session: session, title: title, priority: priority, plan: plan)

        case .awaitingCommissionReviewApproval:
            needsActionCard(
                icon: "checkmark.seal",
                title: "Review approval needed",
                detail: nil,
                footnote: "Handle this review from the web UI.")

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

        case .error(let message):
            errorCard(message: message)

        case .contextExhausted:
            // Gate on the server's mode, per this type's own rule: an
            // already-continued conversation is presented as done and must
            // not look blocked; only an uncontinued one needs action.
            if session.presentationMode == "needs_action" {
                needsActionCard(
                    icon: "arrow.triangle.2.circlepath",
                    title: "Context exhausted",
                    detail: nil,
                    footnote: "Continue this work from the web UI.")
            }

        case .cancelling, .cancellingTool, .cancellingSubAgents:
            workingRow {
                Text("Cancelling…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .other(let type):
            // Unhandled variant: label it rather than guessing. The server's
            // presentation mode decides the family — an untyped variant that
            // needs the user still gets a card, not silence.
            if session.presentationMode == "needs_action" {
                needsActionCard(
                    icon: "person.crop.circle.badge.exclamationmark",
                    title: "Action needed",
                    detail: type.replacingOccurrences(of: "_", with: " "),
                    footnote: "Handle this from the web UI.")
            } else if session.agentWorking {
                workingRow {
                    Text(type.replacingOccurrences(of: "_", with: " "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

        case .idle, .awaitingLlm, .awaitingContinuation, .terminal, .handedOff, .unknown:
            if session.agentWorking {
                workingRow {
                    Text("Working…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func progressSuffix(remaining: Int, completed: Int) -> String {
        var parts: [String] = []
        if completed > 0 { parts.append("\(completed) done") }
        if remaining > 0 { parts.append("\(remaining) queued") }
        return parts.isEmpty ? "running" : parts.joined(separator: " · ")
    }

    private func workingRow(@ViewBuilder _ content: () -> some View) -> some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            content()
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    private func needsActionCard(
        icon: String, title: String, detail: String?, footnote: String
    ) -> some View {
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
    }

    private func errorCard(message: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Agent error", systemImage: "exclamationmark.triangle.fill")
                .font(.callout.bold())
                .foregroundStyle(.red)
            Text(message)
                .font(.callout)
                .lineLimit(4)
            HStack {
                Spacer()
                // Exemplar online-only action (see ConversationAction):
                // resumable errors clear server-side; non-resumable ones
                // come back as a conflict toast explaining why.
                Button("Dismiss error") {
                    session.perform(.dismissError)
                }
                .font(.callout.bold())
                .disabled(!model.connectivity.isOnline)
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
    }
}

/// Reviews and resolves a proposed task plan.
struct TaskApprovalCard: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession
    let title: String
    let priority: String
    let plan: String

    @State private var planExpanded = false
    @State private var showFeedbackField = false
    @State private var feedbackText = ""
    @State private var confirmReject = false

    private var busy: Bool { session.actionInFlight != nil }
    private var actionable: Bool { model.connectivity.isOnline && !busy }

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
                        let text = feedbackText.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !text.isEmpty else { return }
                        session.perform(.provideTaskFeedback(annotations: text))
                        // Deliberately NOT cleared here: on success the
                        // state change unmounts this card (draft discarded
                        // with it); on failure the user's typed annotations
                        // must survive for retry — these actions don't
                        // queue, so the draft is the only copy.
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
                    Button("Start here") {
                        session.perform(.approveTask(
                            handoff: .continueInCurrentConversation))
                    }
                    Button("New chat") {
                        session.perform(.approveTask(
                            handoff: .startFreshWorkConversation))
                    }
                } label: {
                    Label("Approve…", systemImage: "checkmark")
                }
                .buttonStyle(.borderedProminent)
                .disabled(!actionable)
            }
            .font(.callout)

            if !model.connectivity.isOnline {
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
        .confirmationDialog(
            "Reject this task plan?",
            isPresented: $confirmReject, titleVisibility: .visible
        ) {
            Button("Reject plan", role: .destructive) {
                session.perform(.rejectTask)
            }
        }
    }
}
