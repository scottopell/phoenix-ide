import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var confirmClearCache = false
    @State private var confirmSignOut = false
    @State private var confirmForgetPin = false
    /// Bumped after forgetting the pin so the static CertPinStore values
    /// re-render.
    @State private var pinRefresh = 0

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section("Server") {
                    LabeledContent("URL", value: model.serverURLString)
                    Toggle("Trust self-signed certificate", isOn: $model.trustSelfSigned)
                }

                Section {
                    if let pinned = CertPinStore.pinnedDescription {
                        LabeledContent("Pinned certificate") {
                            Text(pinned)
                                .font(.caption.monospaced())
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                        if CertPinStore.lastMismatchAt != nil {
                            Label(
                                "The server's certificate changed — connections are "
                                + "being rejected. If you expected this (e.g. the server "
                                + "was reinstalled), trust the new certificate below.",
                                systemImage: "exclamationmark.shield.fill")
                                .font(.caption)
                                .foregroundStyle(.red)
                        }
                        Button("Forget pinned certificate", role: .destructive) {
                            confirmForgetPin = true
                        }
                    } else {
                        Text("No certificate pinned yet — the first successful "
                            + "connection pins the server's certificate.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                } header: {
                    Text("Security")
                } footer: {
                    Text(
                        "Self-signed certificates are trusted on first use and pinned; "
                        + "a changed certificate is rejected until you explicitly re-trust it.")
                }
                .id(pinRefresh)

                Section("Connection") {
                    LabeledContent("Network") {
                        Label(
                            model.connectivity.isOnline ? "Online" : "Offline",
                            systemImage: model.connectivity.isOnline ? "wifi" : "wifi.slash")
                            .foregroundStyle(model.connectivity.isOnline ? .green : .orange)
                    }
                    if let refreshed = model.listStore.lastRefreshed {
                        LabeledContent("List refreshed") {
                            Text(refreshed, format: .relative(presentation: .named))
                        }
                    }
                }

                Section {
                    Button("Clear offline cache", role: .destructive) {
                        confirmClearCache = true
                    }
                } header: {
                    Text("Storage")
                } footer: {
                    Text(
                        "Removes cached conversations and message history from this "
                        + "device. Queued unsent messages are also removed.")
                }

                Section {
                    Button("Sign out", role: .destructive) {
                        confirmSignOut = true
                    }
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .confirmationDialog(
                "Clear cached data? Unsent queued messages will be lost.",
                isPresented: $confirmClearCache, titleVisibility: .visible
            ) {
                Button("Clear cache", role: .destructive) { model.clearCache() }
            }
            .confirmationDialog(
                "Sign out and forget this server?",
                isPresented: $confirmSignOut, titleVisibility: .visible
            ) {
                Button("Sign out", role: .destructive) {
                    model.signOut()
                    dismiss()
                }
            }
            .confirmationDialog(
                "Forget the pinned certificate? The next connection will pin "
                    + "whatever certificate the server presents.",
                isPresented: $confirmForgetPin, titleVisibility: .visible
            ) {
                Button("Forget pin", role: .destructive) {
                    CertPinStore.forget()
                    pinRefresh += 1
                }
            }
        }
    }
}
