import SwiftUI

@main
struct PhoenixApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        Settings {
            SettingsView(serverManager: appDelegate.serverManager)
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
    @State private var draft = SettingsDraft.defaults
    @State private var savedDraft = SettingsDraft.defaults
    @State private var savedAppliedSnapshot: PersistedSettingsSnapshot?
    @State private var hasSavedModeSelection = false
    @State private var statusMessage: String?
    @State private var errorMessage: String?
    @State private var modeMessage: String?
    private let persistence = SettingsPersistence()

    private var selectedKind: PendingServerModeKind? {
        draft.mode
    }

    private var hasUnappliedChanges: Bool {
        draft != savedDraft
    }

    private var canApply: Bool {
        draft.mode != nil
    }

    var body: some View {
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
                Text("Pick how Phoenix.app should connect before the first run. The app does not auto-connect until you choose a mode and apply it.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
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
                    do {
                        let persisted = try persistence.persist(draft: draft, appliedSnapshot: savedAppliedSnapshot)
                        let reconnectDecision = ServerReconnectRequest.evaluate(
                            currentMode: serverManager.mode,
                            currentState: serverManager.state,
                            candidate: persisted.candidate,
                            changedBundledSecrets: persisted.summary.changedBundledSecrets
                        )
                        savedDraft = persisted.persistedSnapshot.draft()
                        savedAppliedSnapshot = persisted.persistedSnapshot
                        hasSavedModeSelection = true
                        if reconnectDecision.requiresReconnect {
                            try serverManager.reconnect(reconnectDecision)
                        }
                        statusMessage = SettingsFeedback.statusMessage(summary: persisted.summary)
                        modeMessage = nil
                    } catch {
                        if case ConfigurationError.missingModeSelection = error {
                            modeMessage = error.localizedDescription
                        } else {
                            errorMessage = error.localizedDescription
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canApply)
            }

            if hasUnappliedChanges {
                Text("Settings changed locally. Phoenix.app keeps using the saved configuration until you click Apply and Connect.")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
            if let errorMessage {
                Text(errorMessage)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
        .padding(24)
        .frame(width: 540)
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
            } catch {
                errorMessage = error.localizedDescription
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
            errorMessage = error.localizedDescription
        }
    }

    private var attachedSettings: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Managed deployment origin").font(.headline)
            TextField("https://phoenix.example.test:8031", text: $draft.attachedOrigin)
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
            DisclosureGroup("Optional provider credentials") {
                VStack(alignment: .leading, spacing: 8) {
                    SecureField("Anthropic API key", text: $draft.anthropicKey)
                        .textFieldStyle(.roundedBorder)
                    SecureField("OpenAI API key", text: $draft.openAIKey)
                        .textFieldStyle(.roundedBorder)
                    Text("Stored in this Mac's Keychain only when you click Apply and Connect. Clearing a field deletes that saved secret from Keychain on the next apply.")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    if let statusMessage {
                        Text(statusMessage).font(.caption).foregroundStyle(.secondary)
                    }
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
        case .failed, .unavailable, .wrongService, .tlsFailure, .unsupportedOwnership: "xmark.circle.fill"
        default: "ellipsis.circle"
        }
    }

    private var stateColor: Color {
        switch serverManager.state {
        case .ready: .green
        case .failed, .unavailable, .wrongService, .tlsFailure, .unsupportedOwnership: .red
        default: .secondary
        }
    }
}
