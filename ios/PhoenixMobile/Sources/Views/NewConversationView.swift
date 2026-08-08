import SwiftUI

/// Creates a conversation on the server (requires connectivity — the server
/// validates the working directory and mints the id/slug). The directory
/// field shows inline validity per the Phoenix feedback pattern.
struct NewConversationView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @AppStorage(AppModel.lastCwdKey) private var cwd = ""
    @State private var firstMessage = ""
    @State private var modelIDs: [String] = []
    @State private var selectedModel: String?
    @State private var serverDefaultModel: String?
    @State private var cwdStatus: CwdStatus = .unknown
    @State private var creating = false
    @State private var errorText: String?
    @State private var validationTask: Task<Void, Never>?
    /// One id per draft, minted at sheet presentation and reused across
    /// retries: if the create POST reaches the server but the response is
    /// lost, retrying with the same message_id lets the server's duplicate
    /// guard converge instead of creating a second conversation.
    @State private var createMessageId = UUID().uuidString.lowercased()

    enum CwdStatus: Equatable {
        case unknown
        case checking
        case valid(isGit: Bool)
        case invalid(String)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Working directory") {
                    HStack(spacing: 6) {
                        cwdIndicator
                        TextField("/path/on/server", text: $cwd)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .font(.body.monospaced())
                    }
                    if case .invalid(let reason) = cwdStatus {
                        Text(reason)
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }

                if !modelIDs.isEmpty {
                    Section("Model") {
                        Picker("Model", selection: $selectedModel) {
                            if let serverDefaultModel {
                                Text("Server default (\(serverDefaultModel))")
                                    .tag(String?.some(serverDefaultModel))
                            }
                            ForEach(modelIDs, id: \.self) { id in
                                if id != serverDefaultModel {
                                    Text(id).tag(String?.some(id))
                                }
                            }
                        }
                    }
                }

                Section("First message") {
                    TextField("What should the agent do?", text: $firstMessage, axis: .vertical)
                        .lineLimit(3...10)
                }

                if let errorText {
                    Section {
                        Label(errorText, systemImage: "xmark.circle.fill")
                            .foregroundStyle(.red)
                            .font(.callout)
                    }
                }
            }
            .navigationTitle("New Conversation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(creating)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(creating ? "Creating…" : "Create") {
                        Task { await create() }
                    }
                    .disabled(!canCreate)
                }
            }
            .onChange(of: cwd) { _, newValue in
                scheduleValidation(newValue)
            }
            .task {
                if !cwd.isEmpty { scheduleValidation(cwd) }
                if let api = model.api,
                   let models = try? await api.models() {
                    modelIDs = models.modelIDs
                    serverDefaultModel = models.default
                    selectedModel = models.default ?? models.modelIDs.first
                }
            }
        }
    }

    private var canCreate: Bool {
        if creating { return false }
        if firstMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return false }
        guard selectedModel != nil else { return false }
        if case .valid = cwdStatus { return true }
        return false
    }

    @ViewBuilder
    private var cwdIndicator: some View {
        switch cwdStatus {
        case .unknown:
            Image(systemName: "questionmark.circle").foregroundStyle(.secondary)
        case .checking:
            ProgressView().controlSize(.small)
        case .valid(let isGit):
            Image(systemName: isGit ? "checkmark.seal.fill" : "checkmark.circle.fill")
                .foregroundStyle(.green)
        case .invalid:
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        }
    }

    private func scheduleValidation(_ path: String) {
        validationTask?.cancel()
        let trimmed = path.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            cwdStatus = .unknown
            return
        }
        cwdStatus = .checking
        validationTask = Task {
            // Debounce keystrokes.
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled, let api = model.api else { return }
            do {
                let result = try await api.validateCwd(path: trimmed)
                guard !Task.isCancelled else { return }
                cwdStatus = result.valid
                    ? .valid(isGit: result.is_git ?? false)
                    : .invalid(result.error ?? "Not a usable directory")
            } catch {
                guard !Task.isCancelled else { return }
                cwdStatus = .invalid(
                    (error as? APIError)?.errorDescription ?? error.localizedDescription)
            }
        }
    }

    private func create() async {
        guard let api = model.api, let selectedModel else { return }
        creating = true
        defer { creating = false }
        errorText = nil
        do {
            let conversation = try await api.createConversation(
                cwd: cwd.trimmingCharacters(in: .whitespaces),
                text: firstMessage.trimmingCharacters(in: .whitespacesAndNewlines),
                model: selectedModel,
                messageId: createMessageId)
            model.listStore.upsert(conversation)
            dismiss()
        } catch {
            errorText = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}
