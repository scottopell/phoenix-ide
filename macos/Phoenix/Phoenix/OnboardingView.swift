import SwiftUI
import AppKit

enum OnboardingProviderChoice: String, CaseIterable, Identifiable {
    case authHelper
    case codex
    case apiKeys

    var id: String { rawValue }

    var label: String {
        switch self {
        case .authHelper: "Auth Helper"
        case .codex: "Codex / ChatGPT"
        case .apiKeys: "API Keys"
        }
    }
}

struct AuthHelperConfigEditor: View {
    @Binding var tomlText: String
    @ObservedObject var testRunner: AuthHelperTestRunner
    let loadedFromExistingFile: Bool
    let configPathDisplay: String

    private var parseResult: Result<ParsedPastedConfig, UserConfigTomlError>? {
        let trimmed = tomlText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        do {
            return .success(try UserConfigTomlParser.parseSnippet(tomlText))
        } catch let error as UserConfigTomlError {
            return .failure(error)
        } catch {
            return .failure(.invalidSyntax(line: 1, detail: error.localizedDescription))
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text("Provider config (TOML)")
                    .font(.subheadline)
                    .fontWeight(.medium)
                Spacer()
                Button {
                    if let pasted = NSPasteboard.general.string(forType: .string) {
                        tomlText = pasted
                        testRunner.reset()
                    }
                } label: {
                    Label("Paste from Clipboard", systemImage: "doc.on.clipboard")
                }
                .controlSize(.small)

                if !tomlText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Button("Clear") {
                        tomlText = ""
                        testRunner.reset()
                    }
                    .controlSize(.small)
                }
            }

            if loadedFromExistingFile {
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                    Text("Loaded existing [llm] config from \(configPathDisplay)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }

            ZStack(alignment: .topLeading) {
                TextEditor(text: $tomlText)
                    .font(.system(.caption, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .frame(minHeight: 118, maxHeight: 165)
                    .background(Color(nsColor: .textBackgroundColor))
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .stroke(borderColor, lineWidth: 1)
                    )
                    .onChange(of: tomlText) { _, _ in
                        if case .succeeded = testRunner.status {
                            testRunner.reset()
                        } else if case .failed = testRunner.status {
                            testRunner.reset()
                        }
                    }

                if tomlText.isEmpty {
                    Text(Self.placeholderSnippet)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 11)
                        .padding(.vertical, 9)
                        .allowsHitTesting(false)
                }
            }

            Text("Saves to \(configPathDisplay) while preserving any existing agents, tiers, or custom models. Markdown code fences and curly quotes from chat messages are cleaned up automatically.")
                .font(.caption2)
                .foregroundStyle(.secondary)

            switch parseResult {
            case .none:
                EmptyView()
            case .failure(let error):
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.red)
                    Text(error.localizedDescription)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.red.opacity(0.08))
                .clipShape(RoundedRectangle(cornerRadius: 6))

            case .success(let parsed):
                parsedSummaryCard(parsed)
            }
        }
    }

    private var borderColor: Color {
        switch parseResult {
        case .success: Color.green.opacity(0.45)
        case .failure: Color.red.opacity(0.6)
        case .none: Color(nsColor: .separatorColor)
        }
    }

    @ViewBuilder
    private func parsedSummaryCard(_ parsed: ParsedPastedConfig) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .center, spacing: 8) {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                Text("Valid Phoenix config")
                    .font(.caption)
                    .fontWeight(.semibold)
                Spacer()
                if let helper = parsed.llm.apiKeyHelper {
                    if case .running = testRunner.status {
                        Button("Cancel Test") {
                            testRunner.cancel()
                        }
                        .controlSize(.small)
                    } else {
                        Button {
                            testRunner.run(command: helper)
                        } label: {
                            Label(
                                testButtonLabel,
                                systemImage: testButtonIcon
                            )
                        }
                        .controlSize(.small)
                        .buttonStyle(.bordered)
                    }
                }
            }

            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(parsed.llm.summaryItems.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("\(item.label):")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .frame(width: 96, alignment: .trailing)
                        Text(item.value)
                            .font(.system(.caption2, design: .monospaced))
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                    }
                }
                if !parsed.rawModelsBlocks.isEmpty {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("Extra models:")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .frame(width: 96, alignment: .trailing)
                        Text("\(parsed.rawModelsBlocks.count) [[models]] definition\(parsed.rawModelsBlocks.count == 1 ? "" : "s")")
                            .font(.system(.caption2, design: .monospaced))
                    }
                }
            }

            ForEach(parsed.warnings, id: \.self) { warning in
                HStack(alignment: .top, spacing: 6) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                        .font(.caption2)
                    Text(warning)
                        .font(.caption2)
                        .foregroundStyle(.orange)
                }
            }

            helperTestFeedbackView
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.green.opacity(0.07))
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(Color.green.opacity(0.25), lineWidth: 1)
        )
    }

    private var testButtonLabel: String {
        switch testRunner.status {
        case .succeeded: "Re-test Helper"
        case .failed: "Retry Helper"
        default: "Test Helper"
        }
    }

    private var testButtonIcon: String {
        switch testRunner.status {
        case .succeeded: "checkmark.seal.fill"
        case .failed: "arrow.clockwise"
        default: "play.fill"
        }
    }

    @ViewBuilder
    private var helperTestFeedbackView: some View {
        switch testRunner.status {
        case .idle:
            EmptyView()

        case .running(let lines, let detectedURLs):
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Testing auth helper…")
                        .font(.caption)
                        .fontWeight(.medium)
                }
                if !detectedURLs.isEmpty {
                    HStack(spacing: 8) {
                        ForEach(detectedURLs, id: \.self) { url in
                            Button {
                                NSWorkspace.shared.open(url)
                            } label: {
                                Label("Open Sign-In Link", systemImage: "safari")
                            }
                            .controlSize(.small)
                            .buttonStyle(.borderedProminent)
                        }
                    }
                }
                if !lines.isEmpty {
                    Text(lines.joined(separator: "\n"))
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(8)
            .background(Color(nsColor: .controlBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 6))

        case .succeeded(let credentialLength, let durationSeconds, let lines):
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.seal.fill")
                        .foregroundStyle(.green)
                    Text(String(
                        format: "Auth helper verified — received credential (%d chars, redacted) in %.1fs.",
                        credentialLength,
                        durationSeconds
                    ))
                    .font(.caption)
                    .fontWeight(.medium)
                    .foregroundStyle(.green)
                }
                if !lines.isEmpty {
                    Text(lines.joined(separator: "\n"))
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(4)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.green.opacity(0.12))
            .clipShape(RoundedRectangle(cornerRadius: 6))

        case .failed(let message, let lines):
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .top, spacing: 6) {
                    Image(systemName: "xmark.octagon.fill")
                        .foregroundStyle(.red)
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                }
                if !lines.isEmpty {
                    Text(lines.joined(separator: "\n"))
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.red.opacity(0.08))
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
    }

    private static let placeholderSnippet = """
    [llm]
    api_key_helper = "your-auth-helper token"
    auth_header = "bearer"
    anthropic_base_url = "https://gateway.example/v1/messages"
    openai_chat_completions_base_url = "https://gateway.example/v1/chat/completions"
    """
}

struct OnboardingView: View {
    let onComplete: (ServerMode) -> Void

    @State private var showExistingServer = false
    @State private var providerChoice: OnboardingProviderChoice = .authHelper
    @State private var tomlText = ""
    @State private var loadedExistingToml = false
    @State private var anthropicKey = ""
    @State private var openAIKey = ""
    @State private var attachedOrigin = ConfigurationStore.defaultAttachedOrigin
    @State private var userEditedAttachedOrigin = false
    @State private var detectedLocalServer: DetectedLocalServer?
    @State private var errorMessage: String?
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

    private var parsedAuthHelperConfig: ParsedPastedConfig? {
        let trimmed = tomlText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return try? UserConfigTomlParser.parseSnippet(tomlText)
    }

    private var canStartLocal: Bool {
        switch providerChoice {
        case .authHelper:
            return parsedAuthHelperConfig != nil
        case .codex:
            return true
        case .apiKeys:
            return !anthropicKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                || !openAIKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            Spacer(minLength: 16)
            VStack(alignment: .leading, spacing: 20) {
                header

                if let detected = detectedLocalServer, !showExistingServer {
                    detectedLocalServerBanner(detected)
                }

                if showExistingServer {
                    existingServerPanel
                } else {
                    localSetupPanel
                }

                if let errorMessage {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                        Text(errorMessage)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.red.opacity(0.08))
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                }

                Divider()

                footerBar
            }
            .padding(28)
            .frame(maxWidth: 620)
            .background(Color(nsColor: .windowBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(Color(nsColor: .separatorColor).opacity(0.6), lineWidth: 1)
            )
            Spacer(minLength: 16)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            if tomlText.isEmpty, let existing = try? UserConfigStore.loadExistingLLMConfig(at: configURL) {
                tomlText = UserConfigTomlParser.serializeLLMBlock(existing)
                loadedExistingToml = true
            }
        }
        .task {
            let detected = await LocalDeploymentDetector.detect()
            guard let detected else { return }
            detectedLocalServer = detected
            if !userEditedAttachedOrigin {
                attachedOrigin = detected.origin.description
            }
        }
        .onDisappear {
            helperTestRunner.cancel()
        }
    }

    @ViewBuilder
    private func detectedLocalServerBanner(_ detected: DetectedLocalServer) -> some View {
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: detected.tlsTrustRequired ? "lock.trianglebadge.exclamationmark.fill" : "bolt.horizontal.circle.fill")
                .font(.title3)
                .foregroundStyle(detected.tlsTrustRequired ? .orange : .green)

            VStack(alignment: .leading, spacing: 2) {
                if let version = detected.version {
                    Text("Running Phoenix detected (\(version.version))")
                        .font(.subheadline)
                        .fontWeight(.medium)
                    Text("\(detected.origin.description) — connect directly without starting a second copy.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    Text("HTTPS Phoenix detected at \(detected.origin.description)")
                        .font(.subheadline)
                        .fontWeight(.medium)
                    Text("Trust ~/.phoenix-ide/tls/phoenix-local-ca.pem in Keychain, then connect.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            Spacer()

            Button("Connect") {
                attachedOrigin = detected.origin.description
                connectExistingServer()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background((detected.tlsTrustRequired ? Color.orange : Color.green).opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke((detected.tlsTrustRequired ? Color.orange : Color.green).opacity(0.3), lineWidth: 1)
        )
    }

    private var header: some View {
        HStack(spacing: 14) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 48, height: 48)
            VStack(alignment: .leading, spacing: 3) {
                Text("Welcome to Phoenix")
                    .font(.title2)
                    .fontWeight(.semibold)
                Text(showExistingServer
                    ? "Connect to a running Phoenix server."
                    : "Choose how Phoenix should authenticate with your LLM provider.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var localSetupPanel: some View {
        VStack(alignment: .leading, spacing: 16) {
            Picker("Authentication method", selection: $providerChoice) {
                ForEach(OnboardingProviderChoice.allCases) { choice in
                    Text(choice.label).tag(choice)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .onChange(of: providerChoice) { _, _ in
                errorMessage = nil
                helperTestRunner.cancel()
            }

            switch providerChoice {
            case .authHelper:
                AuthHelperConfigEditor(
                    tomlText: $tomlText,
                    testRunner: helperTestRunner,
                    loadedFromExistingFile: loadedExistingToml,
                    configPathDisplay: configPathDisplay
                )

            case .codex:
                VStack(alignment: .leading, spacing: 8) {
                    Label("Sign in with your ChatGPT / Codex account", systemImage: "person.badge.key.fill")
                        .font(.subheadline)
                        .fontWeight(.medium)
                    Text("No upfront configuration needed. Phoenix will start locally and open the built-in Codex sign-in flow in your browser.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(nsColor: .controlBackgroundColor))
                .clipShape(RoundedRectangle(cornerRadius: 8))

            case .apiKeys:
                VStack(alignment: .leading, spacing: 10) {
                    Text("Enter one or both provider API keys. Keys are stored in your Mac's Keychain, never in plain text.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    SecureField("Anthropic API key (sk-ant-…)", text: $anthropicKey)
                        .textFieldStyle(.roundedBorder)
                    SecureField("OpenAI API key (sk-…)", text: $openAIKey)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    private var existingServerPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Server URL")
                .font(.subheadline)
                .fontWeight(.medium)
            TextField("http://localhost:8031", text: Binding(
                get: { attachedOrigin },
                set: {
                    attachedOrigin = $0
                    userEditedAttachedOrigin = true
                }
            ))
            .textFieldStyle(.roundedBorder)
            if let detected = detectedLocalServer {
                HStack(spacing: 6) {
                    Image(systemName: detected.tlsTrustRequired ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                        .foregroundStyle(detected.tlsTrustRequired ? .orange : .green)
                    if let version = detected.version {
                        Text("Detected local Phoenix v\(version.version) at \(detected.origin.description)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        Text("Detected HTTPS Phoenix at \(detected.origin.description) (CA trust required)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            Text("Connects to an existing local or remote Phoenix server (HTTP allowed on localhost; HTTPS required for remote hosts). Quitting Phoenix.app leaves that server running.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var footerBar: some View {
        HStack {
            if showExistingServer {
                Button("Back to local setup") {
                    errorMessage = nil
                    showExistingServer = false
                }
                .buttonStyle(.link)
            } else {
                Button("Connect to an existing server…") {
                    errorMessage = nil
                    helperTestRunner.cancel()
                    showExistingServer = true
                }
                .buttonStyle(.link)
            }

            Spacer()

            if showExistingServer {
                Button("Connect") {
                    connectExistingServer()
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
            } else {
                Button("Start Phoenix") {
                    startLocalPhoenix()
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canStartLocal)
                .keyboardShortcut(.defaultAction)
            }
        }
    }

    private func startLocalPhoenix() {
        errorMessage = nil
        helperTestRunner.cancel()
        do {
            var draft = persistence.loadConnectionDraft().draft
            draft.mode = .bundled

            let parsedAuthHelper: ParsedPastedConfig?
            switch providerChoice {
            case .authHelper:
                parsedAuthHelper = try UserConfigTomlParser.parseSnippet(tomlText)
            case .codex:
                parsedAuthHelper = nil
            case .apiKeys:
                parsedAuthHelper = nil
                draft.anthropicKey = anthropicKey
                draft.openAIKey = openAIKey
            }

            // Validate bundled candidate (e.g. sidecar executable present) before writing config.toml.
            _ = try ConfigurationStore.loadCandidate(
                kind: .bundled,
                attachedOrigin: draft.attachedOrigin,
                bundledPort: draft.bundledPort,
                developmentBinaryOverride: draft.developmentBinaryOverride,
                rustLogLevel: draft.rustLogLevel
            )

            if let parsed = parsedAuthHelper {
                try UserConfigStore.mergeAndWrite(
                    llm: parsed.llm,
                    rawModelsBlocks: parsed.rawModelsBlocks.isEmpty ? nil : parsed.rawModelsBlocks,
                    to: configURL
                )
            }

            let persisted = try persistence.persist(draft: draft)
            onComplete(persisted.candidate)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func connectExistingServer() {
        errorMessage = nil
        do {
            var draft = persistence.loadConnectionDraft().draft
            draft.mode = .attached
            draft.attachedOrigin = attachedOrigin
            let persisted = try persistence.persist(draft: draft)
            onComplete(persisted.candidate)
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
