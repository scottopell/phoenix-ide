import SwiftUI

/// First-run server configuration. Validates by hitting /api/auth/status
/// with the entered credentials before committing them.
struct SetupView: View {
    @Environment(AppModel.self) private var model

    @State private var urlText = "https://"
    @State private var passwordText = ""
    @State private var trustSelfSigned = true
    @State private var checking = false
    @State private var errorText: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("Server") {
                    TextField("https://phoenix.local:8031", text: $urlText)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    SecureField("Password (blank if auth disabled)", text: $passwordText)
                    Toggle("Trust self-signed certificate", isOn: $trustSelfSigned)
                }

                if let errorText {
                    Section {
                        Label(errorText, systemImage: "xmark.circle.fill")
                            .foregroundStyle(.red)
                            .font(.callout)
                    }
                }

                Section {
                    Button {
                        Task { await connect() }
                    } label: {
                        if checking {
                            HStack {
                                ProgressView()
                                Text("Checking…")
                            }
                        } else {
                            Text("Connect")
                        }
                    }
                    .disabled(checking || URL(string: urlText)?.host == nil)
                } footer: {
                    Text(
                        "Phoenix dev and prod servers usually serve TLS with a "
                        + "self-signed certificate; leave the trust toggle on for those. "
                        + "The certificate is pinned on first use — if the server later "
                        + "presents a different one, connections fail until you re-trust "
                        + "it in Settings.")
                }
            }
            .navigationTitle("Phoenix")
        }
    }

    private func connect() async {
        checking = true
        defer { checking = false }
        errorText = nil

        guard let url = URL(string: urlText), url.host != nil else {
            errorText = "Enter a valid server URL."
            return
        }
        guard let probe = PhoenixAPI(
            baseURL: url,
            password: passwordText.isEmpty ? nil : passwordText,
            allowSelfSigned: trustSelfSigned)
        else {
            errorText = "Use HTTPS when a server password is configured."
            return
        }
        do {
            let status = try await probe.authStatus()
            if status.auth_required && !status.authenticated {
                errorText = passwordText.isEmpty
                    ? "This server requires a password."
                    : "Incorrect password."
                return
            }
            model.trustSelfSigned = trustSelfSigned
            model.password = passwordText
            model.serverURLString = urlText
            await model.refreshList()
        } catch {
            errorText = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}
