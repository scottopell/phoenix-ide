import SwiftUI

/// Answer the agent's questions in-app (REQ-IOS-016) — the second
/// interactive needs-action resolution after TaskApprovalCard, following
/// the same rules: online-only actions, no optimistic state (the server's
/// state_change unmounts the card; a concurrent answer from another client
/// wins cleanly and this one surfaces the 409), drafts never cleared
/// before success, controls disabled while offline or in flight.
struct QuestionCard: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession
    let questions: [UserQuestion]

    var body: some View {
        QuestionCardBody(
            questions: questions,
            isOnline: model.connectivity.isOnline,
            acceptsActions: session.acceptsConversationActions,
            busy: session.actionInFlight != nil,
            onAnswer: { session.perform(.respondToQuestions(answers: $0)) },
            onDismiss: { session.perform(.dismissQuestion) })
    }
}

struct QuestionCardBody: View {
    let questions: [UserQuestion]
    let isOnline: Bool
    let acceptsActions: Bool
    let busy: Bool
    let onAnswer: ([String: String]) -> Void
    let onDismiss: () -> Void

    /// Selected option labels per question text. Single-select questions
    /// keep at most one entry; the encoder treats them uniformly.
    @State private var selections: [String: Set<String>] = [:]
    /// Free "Other" text per question text.
    @State private var otherTexts: [String: String] = [:]
    /// Question texts whose "Other" field is revealed.
    @State private var otherRevealed: Set<String> = []
    @State private var confirmDismiss = false

    private var actionable: Bool { isOnline && acceptsActions && !busy }
    private var encodedAnswers: [String: String]? {
        QuestionAnswers.encode(
            questions: questions, selections: selections, otherTexts: otherTexts)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Label(
                    questions.count == 1
                        ? "The agent asked a question"
                        : "The agent asked \(questions.count) questions",
                    systemImage: "questionmark.bubble")
                    .font(.callout.bold())
                Spacer()
                if busy {
                    ProgressView().controlSize(.small)
                }
            }

            ForEach(questions, id: \.question) { question in
                questionSection(question)
            }

            HStack(spacing: 10) {
                Button("Dismiss") {
                    confirmDismiss = true
                }
                .font(.callout)
                .disabled(!actionable)

                Spacer()

                Button("Send answers") {
                    if let answers = encodedAnswers {
                        onAnswer(answers)
                        // Selections deliberately kept: on success the state
                        // change unmounts this card; on failure the user's
                        // choices must survive for retry.
                    }
                }
                .buttonStyle(.borderedProminent)
                .font(.callout)
                .disabled(!actionable || encodedAnswers == nil)
            }

            if !isOnline {
                Text("Offline — answering needs a connection and is never queued.")
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
            "Dismiss without answering? The conversation will return to idle.",
            isPresented: $confirmDismiss, titleVisibility: .visible
        ) {
            Button("Dismiss questions", role: .destructive) {
                onDismiss()
            }
        }
        .accessibilityIdentifier("state.questionCard")
    }

    @ViewBuilder
    private func questionSection(_ question: UserQuestion) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                if !question.header.isEmpty {
                    Text(question.header)
                        .font(.caption2.bold())
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Color.blue.opacity(0.15))
                        .clipShape(Capsule())
                }
                if question.multiSelect {
                    Text("select all that apply")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            Text(question.question)
                .font(.callout)

            ForEach(question.options, id: \.label) { option in
                optionRow(question: question, option: option)
            }

            // "Other" free-text escape hatch, matching the web panel.
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    if otherRevealed.contains(question.question) {
                        otherRevealed.remove(question.question)
                        otherTexts[question.question] = ""
                    } else {
                        otherRevealed.insert(question.question)
                        if !question.multiSelect {
                            selections[question.question] = []
                        }
                    }
                }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: otherRevealed.contains(question.question)
                        ? "checkmark.circle.fill" : "circle")
                        .font(.callout)
                        .foregroundStyle(otherRevealed.contains(question.question)
                            ? Color.accentColor : Color.secondary)
                    Text("Other…")
                        .font(.callout)
                        .foregroundStyle(.primary)
                }
            }
            .buttonStyle(.plain)
            if otherRevealed.contains(question.question) {
                TextField(
                    "Your answer",
                    text: Binding(
                        get: { otherTexts[question.question] ?? "" },
                        set: { otherTexts[question.question] = $0 }),
                    axis: .vertical)
                    .lineLimit(1...4)
                    .textFieldStyle(.roundedBorder)
                    .font(.callout)
            }
        }
        .padding(.vertical, 2)
    }

    private func optionRow(question: UserQuestion, option: UserQuestion.Option) -> some View {
        let selected = selections[question.question]?.contains(option.label) ?? false
        return Button {
            var current = selections[question.question] ?? []
            if question.multiSelect {
                if selected { current.remove(option.label) } else { current.insert(option.label) }
            } else {
                current = selected ? [] : [option.label]
                // Choosing an option clears a revealed Other for single-select.
                otherRevealed.remove(question.question)
                otherTexts[question.question] = ""
            }
            selections[question.question] = current
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .font(.callout)
                    .foregroundStyle(selected ? Color.accentColor : Color.secondary)
                VStack(alignment: .leading, spacing: 1) {
                    Text(option.label)
                        .font(.callout)
                        .foregroundStyle(.primary)
                    if !option.description.isEmpty {
                        Text(option.description)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(3)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
    }
}
