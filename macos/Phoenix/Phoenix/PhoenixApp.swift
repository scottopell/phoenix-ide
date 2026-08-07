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
    @AppStorage(PreferenceKey.serverMode) private var mode = ServerModeKind.attached.rawValue
    @AppStorage(PreferenceKey.attachedOrigin) private var attachedOrigin = ConfigurationStore.defaultAttachedOrigin
    @AppStorage(PreferenceKey.bundledPort) private var bundledPort = ConfigurationStore.defaultBundledPort
    @AppStorage(PreferenceKey.bundledDevelopmentBinary) private var developmentBinary = ""
    @AppStorage(PreferenceKey.rustLogLevel) private var rustLogLevel = "phoenix_ide=info"
    @State private var savedSignature = ""
    @State private var anthropicKey = ""
    @State private var openAIKey = ""
    @State private var keychainMessage: String?
    private let keychain = KeychainStore()

    private var selectedKind: ServerModeKind {
        ServerModeKind(rawValue: mode) ?? .attached
    }

    private var signature: String {
        [mode, attachedOrigin, String(bundledPort), developmentBinary, rustLogLevel].joined(separator: "\u{0}")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Picker("Phoenix server", selection: $mode) {
                ForEach(ServerModeKind.allCases) { value in
                    Text(value.label).tag(value.rawValue)
                }
            }
            .pickerStyle(.segmented)

            switch selectedKind {
            case .attached: attachedSettings
            case .bundled: bundledSettings
            }

            Divider()
            HStack {
                Label(serverManager.state.displayName, systemImage: stateSymbol)
                    .foregroundStyle(stateColor)
                Spacer()
                Button("Apply and Connect") {
                    savedSignature = signature
                    serverManager.reconnect()
                }
                .buttonStyle(.borderedProminent)
            }

            if savedSignature != signature {
                Text("Settings changed; apply them to reconnect.")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        }
        .padding(24)
        .frame(width: 540)
        .onAppear {
            savedSignature = signature
            anthropicKey = (try? keychain.read(.anthropicAPIKey)) ?? ""
            openAIKey = (try? keychain.read(.openAIAPIKey)) ?? ""
        }
    }

    private func saveSecrets() {
        do {
            try keychain.write(anthropicKey, for: .anthropicAPIKey)
            try keychain.write(openAIKey, for: .openAIAPIKey)
            keychainMessage = "Saved. Reconnect bundled Phoenix to apply."
        } catch {
            keychainMessage = error.localizedDescription
        }
    }

    private var attachedSettings: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Managed deployment origin").font(.headline)
            TextField("https://phoenix.example.test:8031", text: $attachedOrigin)
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
                TextField("Port", value: $bundledPort, format: .number)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 100)
            }
            TextField("Rust log filter", text: $rustLogLevel)
                .textFieldStyle(.roundedBorder)
            #if DEBUG
            VStack(alignment: .leading, spacing: 4) {
                Text("Development binary override").font(.subheadline)
                TextField("Leave empty to use the bundled sidecar", text: $developmentBinary)
                    .textFieldStyle(.roundedBorder)
                Text("Debug builds only. Release builds always use Contents/Helpers/phoenix_ide.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            #endif
            DisclosureGroup("Optional provider credentials") {
                VStack(alignment: .leading, spacing: 8) {
                    SecureField("Anthropic API key", text: $anthropicKey)
                        .textFieldStyle(.roundedBorder)
                    SecureField("OpenAI API key", text: $openAIKey)
                        .textFieldStyle(.roundedBorder)
                    HStack {
                        Text("Stored in this Mac's Keychain; never shown in diagnostics or preferences.")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Save to Keychain") { saveSecrets() }
                    }
                    if let keychainMessage {
                        Text(keychainMessage).font(.caption).foregroundStyle(.secondary)
                    }
                }
                .padding(.top, 6)
            }
        }
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
