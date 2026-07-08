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

        case .awaitingTaskApproval(let title):
            needsActionCard(
                icon: "checklist",
                title: "Task plan awaiting approval",
                detail: title.isEmpty ? nil : title,
                footnote: "Review and approve from the web UI.")

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

        case .cancelling:
            workingRow {
                Text("Cancelling…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        case .other(let type):
            // Unhandled variant: label it rather than guessing. The busy
            // spinner still comes from agentWorking (presentation_mode).
            if session.agentWorking {
                workingRow {
                    Text(type.replacingOccurrences(of: "_", with: " "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

        case .idle, .awaitingLlm, .terminal, .handedOff, .unknown:
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
