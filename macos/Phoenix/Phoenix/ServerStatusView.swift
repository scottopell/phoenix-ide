import SwiftUI
import AppKit

struct ServerStatusView: View {
    @ObservedObject private var serverManager: ServerManager

    init(serverManager: ServerManager) {
        self.serverManager = serverManager
    }

    private var snapshot: ServerStatusSnapshot { serverManager.statusSnapshot() }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Phoenix Connection").font(.title2).fontWeight(.semibold)
                    Text(snapshot.origin ?? "No origin selected")
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Reconnect") { serverManager.reconnect() }
                    .buttonStyle(.borderedProminent)
            }
            .padding(16)
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    StatusSection("Connection") {
                        StatusRow("State", snapshot.state.displayName)
                        StatusRow("Mode", snapshot.mode?.label ?? "Unknown")
                        StatusRow("Version", snapshot.version ?? "Not verified")
                        StatusRow("Git SHA", snapshot.gitSHA ?? "Not verified")
                        StatusRow("Ownership", snapshot.ownership ?? "Authentication or verification required")
                    }
                    StatusSection("Network") {
                        StatusRow("Bind address", snapshot.bindAddress ?? "Not verified")
                        StatusRow("TLS", yesNo(snapshot.tlsEnabled))
                        StatusRow("Socket activated", yesNo(snapshot.socketActivated))
                        StatusRow("Same-host access", yesNo(snapshot.localAccess))
                    }

                    if snapshot.mode == .bundled {
                        StatusSection("App-owned sidecar") {
                            StatusRow("PID", snapshot.processID.map(String.init) ?? "None")
                            StatusRow("Executable", snapshot.executablePath ?? "Unavailable")
                            StatusRow("Private database", snapshot.databasePath ?? "Unavailable")
                            StatusRow("Log", snapshot.logPath ?? "Unavailable")
                        }
                        StatusSection("Recent sidecar log") {
                            if snapshot.recentLogLines.isEmpty {
                                Text("No log lines captured.").foregroundStyle(.secondary)
                            } else {
                                Text(snapshot.recentLogLines.joined(separator: "\n"))
                                    .font(.system(.caption, design: .monospaced))
                                    .textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(10)
                                    .background(.quaternary)
                                    .clipShape(RoundedRectangle(cornerRadius: 6))
                            }
                        }
                    } else {
                        Text("Attached mode has no process, database, credential, restart, or backend-update controls. Those remain owned by the Phoenix deployment.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(16)
            }
        }
        .frame(width: 680, height: 620)
    }

    private func yesNo(_ value: Bool?) -> String {
        guard let value else { return "Not verified" }
        return value ? "Yes" : "No"
    }
}

private struct StatusSection<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            VStack(alignment: .leading, spacing: 6) { content }
        }
    }
}

private struct StatusRow: View {
    let label: String
    let value: String

    init(_ label: String, _ value: String) {
        self.label = label
        self.value = value
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(label).foregroundStyle(.secondary).frame(width: 140, alignment: .trailing)
            Text(value)
                .font(.system(.caption, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
