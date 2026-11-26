import SwiftUI

@main
struct PhoenixApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        Settings {
            SettingsView(
                serverManager: appDelegate.serverManager,
                onApplied: { appDelegate.settingsDidApply() }
            )
        }
        .commands {
            CommandGroup(replacing: .newItem) { }
            CommandGroup(after: .toolbar) {
                Button("Reload Page") { appDelegate.reloadWebView() }
                    .keyboardShortcut("r", modifiers: .command)
            }
            CommandMenu("Debug") {
                Button("Connection Status") { appDelegate.showServerStatusWindow() }
                    .keyboardShortcut("i", modifiers: [.command, .option])
            }
        }
    }
}

struct SettingsView: View {
    @ObservedObject var serverManager: ServerManager
    var onApplied: (() -> Void)? = nil
    @State private var draft = SettingsDraft.defaults
    @State private var savedDraft = SettingsDraft.defaults
    @State private var savedAppliedSnapshot: PersistedSettingsSnapshot?
    @State private var hasSavedModeSelection = false
    @State private var feedback = SettingsFeedback.empty
    @State private var modeMessage: String?
    @State private var llmTomlText = ""
    @State private var savedLlmTomlText = ""
    @State private var loadedExistingToml = false
    @State private var userConfigRestartState = PendingUserConfigRestartState()
    @StateObject private var helperTestRunner = AuthHelperTestRunner()
    private let persistence = SettingsPersistence()

    private var configURL: URL { UserConfigStore.configURL() }

    private var configPathDisplay: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let path = configURL.path
        if path.hasPrefix(home) {
            return "~" + path.dropFirst(home.count)
        }
        return path
    }

    private var selectedKind: PendingServerModeKind? {
        draft.mode
    }

    private var llmTomlChanged: Bool {
        userConfigRestartState.shouldForceRestart(for: selectedKind, tomlEdited: llmTomlText != savedLlmTomlText)
    }

    private var hasUnappliedChanges: Bool {
        draft != savedDraft || llmTomlChanged
    }

    private var canApply: Bool {
        guard draft.mode != nil else { return false }
        if selectedKind == .bundled {
            let trimmed = llmTomlText.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty, (try? UserConfigTomlParser.parseSnippet(llmTomlText)) == nil {
                return false
            }
        }
        return true
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Phoenix server").font(.headline)
                    Picker(
                        "Phoenix server",
                        selection: Binding(
                            get: { selectedKind },
                            set: { selectMode($0) }
                        )
                    ) {
                        Text("Choose one").tag(Optional<PendingServerModeKind>.none)
                        ForEach(PendingServerModeKind.allCases) { value in
                            Text(value.label).tag(Optional(value))
                        }
                    }
                    .pickerStyle(.radioGroup)
                    if let modeMessage {
                        Text(modeMessage).font(.caption).foregroundStyle(.orange)
                    }
                }

                switch selectedKind {
                case .attached?: attachedSettings
                case .bundled?: bundledSettings
                case nil:
                    onboardingState
                }

                Divider()
                HStack {
                    Label(serverManager.state.displayName, systemImage: stateSymbol)
                        .foregroundStyle(stateColor)
                    Spacer()
                    Button("Apply and Connect") {
                        applySettings()
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!canApply)
                }

                if hasUnappliedChanges {
                    Text("Settings changed locally. Phoenix.app keeps using the saved configuration until you click Apply and Connect.")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
                if let statusMessage = feedback.statusMessage {
                    Text(statusMessage)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let errorMessage = feedback.errorMessage {
                    Text(errorMessage)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }
            .padding(24)
        }
        .frame(width: 580)
        .frame(minHeight: 420, maxHeight: 720)
        .onAppear {
            do {
                let loaded = try persistence.loadDraft()
                draft = loaded.draft
                savedDraft = loaded.draft
                savedAppliedSnapshot = try persistence.persistedSnapshot()
                hasSavedModeSelection = loaded.hasSavedModeSelection
                if !hasSavedModeSelection {
                    modeMessage = "Choose a mode before Phoenix.app can connect."
                }
                if let existing = try? UserConfigStore.loadExistingLLMConfig(at: configURL) {
                    let serialized = UserConfigTomlParser.serializeLLMBlock(existing)
                    llmTomlText = serialized
                    savedLlmTomlText = serialized
                    loadedExistingToml = true
                } else {
                    llmTomlText = ""
                    savedLlmTomlText = ""
                    loadedExistingToml = false
                }
            } catch {
                feedback = .failure(error)
            }
        }
        .onDisappear {
            helperTestRunner.cancel()
        }
    }

    private func applySettings() {
        helperTestRunner.cancel()
        do {
            guard let mode = draft.mode else { throw ConfigurationError.missingModeSelection }

            // Validate both the TOML snippet and the target server candidate before
            // mutating config.toml, preferences, or Keychain.
            let pendingTomlEdit: (parsed: ParsedPastedConfig?, canonical: String, hasConfig: Bool)?
            if mode == .bundled && llmTomlText != savedLlmTomlText {
                let trimmed = llmTomlText.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmed.isEmpty {
                    pendingTomlEdit = (nil, "", false)
                } else {
                    let parsed = try UserConfigTomlParser.parseSnippet(llmTomlText)
                    pendingTomlEdit = (parsed, UserConfigTomlParser.serializeLLMBlock(parsed.llm), true)
                }
            } else {
                pendingTomlEdit = nil
            }

            _ = try ConfigurationStore.loadCandidate(
                kind: mode.persistedKind,
                attachedOrigin: draft.attachedOrigin,
                bundledPort: draft.bundledPort,
                developmentBinaryOverride: draft.developmentBinaryOverride,
                rustLogLevel: draft.rustLogLevel
            )

            if let pendingTomlEdit {
                if let parsed = pendingTomlEdit.parsed {
                    try UserConfigStore.mergeAndWrite(
                        llm: parsed.llm,
                        rawModelsBlocks: parsed.rawModelsBlocks.isEmpty ? nil : parsed.rawModelsBlocks,
                        to: configURL
                    )
                } else {
                    try UserConfigStore.mergeAndWrite(llm: nil, to: configURL)
                }
                llmTomlText = pendingTomlEdit.canonical
                savedLlmTomlText = pendingTomlEdit.canonical
                loadedExistingToml = pendingTomlEdit.hasConfig
                userConfigRestartState.markConfigWritten()
            }

            let changedUserConfig = userConfigRestartState.shouldForceRestart(
                for: draft.mode,
                tomlEdited: pendingTomlEdit != nil
            )
            let persisted = try persistence.persist(draft: draft, appliedSnapshot: savedAppliedSnapshot)
            let reconnectDecision = ServerReconnectRequest.evaluate(
                currentMode: serverManager.mode,
                currentState: serverManager.state,
                candidate: persisted.candidate,
                changedBundledSecrets: persisted.summary.changedBundledSecrets,
                changedUserConfig: changedUserConfig
            )
            savedDraft = persisted.persistedSnapshot.draft()
            draft = savedDraft
            savedAppliedSnapshot = persisted.persistedSnapshot
            hasSavedModeSelection = true
            if reconnectDecision.requiresReconnect {
                try serverManager.reconnect(reconnectDecision)
            }
            userConfigRestartState.markApplyCompleted(for: draft.mode)
            feedback = .success(summary: persisted.summary)
            modeMessage = nil
            onApplied?()
        } catch {
            feedback = .failure(error)
            if case ConfigurationError.missingModeSelection = error {
                modeMessage = error.localizedDescription
            } else {
                modeMessage = nil
            }
        }
    }



    private func selectMode(_ mode: PendingServerModeKind?) {
        draft.mode = mode
        modeMessage = nil
        guard mode == .bundled,
              let savedAppliedSnapshot,
              savedAppliedSnapshot.secrets.values.contains(.unloaded) || savedAppliedSnapshot.secrets.values.contains(.preserveUnloaded) else { return }
        do {
            self.savedAppliedSnapshot = try persistence.loadBundledSecrets(
                into: &draft,
                appliedSnapshot: savedAppliedSnapshot
            )
        } catch {
            feedback = .failure(error)
        }
    }

    private var attachedSettings: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Managed deployment origin").font(.headline)
            TextField("http://localhost:8031", text: $draft.attachedOrigin)
                .textFieldStyle(.roundedBorder)
            Text("Phoenix.app connects through the API and normal Phoenix login. It never starts, stops, configures, or updates this deployment.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var bundledSettings: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Bundled sidecar").font(.headline)
            Text("The sidecar binds to 127.0.0.1 with TLS off and uses private Phoenix.app data. Quitting the app stops this process.")
                .font(.caption)
                .foregroundStyle(.secondary)

            AuthHelperConfigEditor(
                tomlText: $llmTomlText,
                testRunner: helperTestRunner,
                loadedFromExistingFile: loadedExistingToml,
                configPathDisplay: configPathDisplay
            )

            DisclosureGroup("Optional API keys (Keychain)") {
                VStack(alignment: .leading, spacing: 8) {
                    SecureField("Anthropic API key", text: $draft.anthropicKey)
                        .textFieldStyle(.roundedBorder)
                    SecureField("OpenAI API key", text: $draft.openAIKey)
                        .textFieldStyle(.roundedBorder)
                    Text("Stored in this Mac's Keychain only when you click Apply and Connect. Clearing a field deletes that saved secret from Keychain on the next apply. For Codex / ChatGPT sign-in, use the in-app login flow.")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                .padding(.top, 6)
            }

            DisclosureGroup("Advanced sidecar options") {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("Port")
                        TextField("Port", value: $draft.bundledPort, format: .number)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 100)
                    }
                    TextField("Rust log filter", text: $draft.rustLogLevel)
                        .textFieldStyle(.roundedBorder)
                    #if DEBUG
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Development binary override").font(.subheadline)
                        TextField("Leave empty to use the bundled sidecar", text: $draft.developmentBinaryOverride)
                            .textFieldStyle(.roundedBorder)
                        Text("Debug builds only. Release builds always use Contents/Helpers/phoenix_ide.")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    #endif
                }
                .padding(.top, 6)
            }
        }
    }

    private var onboardingState: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Choose a first-run connection mode").font(.headline)
            Text("Managed deployment connects to an existing Phoenix server. Bundled Phoenix starts the app-owned sidecar on loopback with private data. Phoenix.app will not connect until you make a choice and apply it.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 8)
    }

    private var stateSymbol: String {
        switch serverManager.state {
        case .ready: "checkmark.circle.fill"
        case .failed, .unavailable, .wrongService, .tlsFailure: "xmark.circle.fill"
        default: "ellipsis.circle"
        }
    }

    private var stateColor: Color {
        switch serverManager.state {
        case .ready: .green
        case .failed, .unavailable, .wrongService, .tlsFailure: .red
        default: .secondary
        }
    }
}
