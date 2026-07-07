import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var confirmClearCache = false
    @State private var confirmSignOut = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section("Server") {
                    LabeledContent("URL", value: model.serverURLString)
                    Toggle("Trust self-signed certificate", isOn: $model.trustSelfSigned)
                }

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

                Section("Storage") {
                    Button("Clear offline cache", role: .destructive) {
                        confirmClearCache = true
                    }
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
        }
    }
}
