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
    @State private var modelsAvailable = true
    @State private var loadingModels = false
    @State private var modelsError: String?
    @State private var cwdStatus: CwdStatus = .unknown
    @State private var creating = false
    @State private var errorText: String?
    @State private var validationTask: Task<Void, Never>?
    @State private var pendingAttempt: CreateAttempt?

    private static let pendingAttemptStoreName = "pending-conversation-creation"

    private struct CreateAttempt: Codable, Equatable {
        let cwd: String
        let text: String
        let model: String
        let messageId: String
    }

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
                            .disabled(pendingAttempt != nil)
                            .accessibilityIdentifier("newConversation.cwd")
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
                        .disabled(pendingAttempt != nil)
                        .accessibilityIdentifier("newConversation.model")
                    }
                }
                if !modelsAvailable {
                    Section {
                        Label("No model is configured on this server.", systemImage: "xmark.circle.fill")
                            .foregroundStyle(.red)
                            .font(.callout)
                    }
                }
                if let modelsError {
                    Section {
                        Label(modelsError, systemImage: "xmark.circle.fill")
                            .foregroundStyle(.red)
                            .font(.callout)
                        Button("Retry loading models") {
                            Task { await loadModels() }
                        }
                        .disabled(loadingModels)
                    }
                }

                Section("First message") {
                    TextField("What should the agent do?", text: $firstMessage, axis: .vertical)
                        .lineLimit(3...10)
                        .disabled(pendingAttempt != nil)
                        .accessibilityIdentifier("newConversation.message")
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
                        .disabled(creating || pendingAttempt != nil)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(creating ? "Creating…" : "Create") {
                        Task { await create() }
                    }
                    .disabled(!canCreate)
                    .accessibilityIdentifier("newConversation.create")
                }
            }
            .onChange(of: cwd) { _, newValue in
                scheduleValidation(newValue)
            }
            .task {
                restorePendingAttempt()
                if !cwd.isEmpty { scheduleValidation(cwd) }
                await loadModels()
                if let pendingAttempt {
                    selectedModel = pendingAttempt.model
                }
            }
            .interactiveDismissDisabled(creating || pendingAttempt != nil)
        }
    }

    private var canCreate: Bool {
        if creating { return false }
        if !model.connectivity.isOnline { return false }
        if pendingAttempt != nil { return true }
        if firstMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return false }
        guard !loadingModels, modelsError == nil,
              modelsAvailable, let selectedModel, modelIDs.contains(selectedModel) else {
            return false
        }
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
        guard let api = model.api else { return }
        let attempt: CreateAttempt
        if let pendingAttempt {
            attempt = pendingAttempt
        } else {
            guard let selectedModel else { return }
            attempt = CreateAttempt(
                cwd: cwd.trimmingCharacters(in: .whitespaces),
                text: firstMessage.trimmingCharacters(in: .whitespacesAndNewlines),
                model: selectedModel,
                messageId: UUID().uuidString.lowercased())
            guard DiskStore.save(attempt, name: Self.pendingAttemptStoreName) else {
                errorText = "Creation attempt could not be saved on this device. Free storage and try again."
                return
            }
            pendingAttempt = attempt
        }
        creating = true
        defer { creating = false }
        errorText = nil
        do {
            let conversation = try await api.createConversation(
                cwd: attempt.cwd,
                text: attempt.text,
                model: attempt.model,
                messageId: attempt.messageId)
            model.listStore.upsert(conversation)
            clearPendingAttempt()
            dismiss()
        } catch {
            errorText = (error as? APIError)?.errorDescription ?? error.localizedDescription
            if let apiError = error as? APIError {
                switch apiError {
                case .http, .certificatePinMismatch, .invalidURL:
                    clearPendingAttempt()
                case .transport, .decoding:
                    break
                }
            }
        }
    }

    private func restorePendingAttempt() {
        guard pendingAttempt == nil,
              let attempt = DiskStore.load(
                CreateAttempt.self, name: Self.pendingAttemptStoreName)
        else { return }
        pendingAttempt = attempt
        cwd = attempt.cwd
        firstMessage = attempt.text
        selectedModel = attempt.model
    }

    private func clearPendingAttempt() {
        pendingAttempt = nil
        DiskStore.remove(name: Self.pendingAttemptStoreName)
    }

    private func loadModels() async {
        guard let api = model.api else { return }
        loadingModels = true
        modelsError = nil
        defer { loadingModels = false }
        do {
            let models = try await api.models()
            modelIDs = models.modelIDs
            modelsAvailable = models.llm_configured ?? !models.modelIDs.isEmpty
            serverDefaultModel = models.default.flatMap {
                models.modelIDs.contains($0) ? $0 : nil
            }
            selectedModel = serverDefaultModel ?? models.modelIDs.first
        } catch {
            modelIDs = []
            selectedModel = nil
            modelsError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}
