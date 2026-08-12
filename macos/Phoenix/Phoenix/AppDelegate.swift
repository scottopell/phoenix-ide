import Cocoa
import SwiftUI
import WebKit
import Combine

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var statusWindow: NSWindow?
    private var webView: WKWebView?
    private let browserEnvironment = BrowserEnvironment()
    private let hotkey = GlobalHotkeyManager()
    let serverManager = ServerManager()
    private var cancellables = Set<AnyCancellable>()
    private var pendingConversationID: UUID?
    private var hotkeyError: HotkeyError?

    func applicationDidFinishLaunching(_ notification: Notification) {
        if case .failure(let error) = hotkey.register(action: { [weak self] in self?.showWindow() }) {
            hotkeyError = error
            NSLog("Phoenix hotkey registration failed: \(error.localizedDescription)")
        }
        serverManager.$state
            .receive(on: DispatchQueue.main)
            .sink { [weak self] state in self?.updateWindowContent(for: state) }
            .store(in: &cancellables)
        showWindow()
        serverManager.connect()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if window?.isVisible != true { showWindow() }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard case .bundled = serverManager.mode else {
            browserEnvironment.shutdown()
            return .terminateNow
        }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Quit Phoenix and stop the bundled server?"
        alert.informativeText = "Any active conversations, tools, terminal sessions, and browser sessions owned by this bundled Phoenix may be interrupted."
        alert.addButton(withTitle: "Quit and Stop Phoenix")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return .terminateCancel }
        browserEnvironment.shutdown()
        serverManager.stop { sender.reply(toApplicationShouldTerminate: true) }
        return .terminateLater
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        urls.forEach(handleIncomingURL)
    }

    func showWindow() {
        if let window {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        let created = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1100, height: 760),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        created.title = "Phoenix"
        created.center()
        created.isReleasedWhenClosed = false
        created.delegate = self
        window = created
        updateWindowContent(for: serverManager.state)
        created.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        presentHotkeyErrorIfNeeded()
    }

    private func presentHotkeyErrorIfNeeded() {
        guard let hotkeyError else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Global shortcut unavailable"
        alert.informativeText = hotkeyError.localizedDescription
        alert.runModal()
        self.hotkeyError = nil
    }

    @objc func reloadWebView() { webView?.reload() }

    @objc func showServerStatusWindow() {
        if let statusWindow {
            statusWindow.makeKeyAndOrderFront(nil)
            return
        }
        let created = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 620),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        created.title = "Phoenix Connection Status"
        created.contentView = NSHostingView(rootView: ServerStatusView(serverManager: serverManager))
        created.center()
        created.isReleasedWhenClosed = false
        created.delegate = self
        statusWindow = created
        created.makeKeyAndOrderFront(nil)
    }

    private func updateWindowContent(for state: ConnectionState) {
        guard let window else { return }
        if state.canDisplayWebView, let origin = serverManager.webOrigin {
            if webView != nil { return }
            let operation = serverManager.currentOperationToken
            let wrapper = WebViewWrapper(
                origin: origin,
                operation: operation,
                browserEnvironment: browserEnvironment,
                onWebViewReady: { [weak self] value, _ in
                    self?.webView = value
                    self?.openPendingConversation()
                },
                onDeployment: { [weak self] result, operation in
                    switch result {
                    case .success(let deployment): self?.serverManager.deploymentReceived(deployment, operation: operation)
                    case .failure(let error): self?.serverManager.deploymentVerificationFailed(error.localizedDescription, operation: operation)
                    }
                },
                onAuthenticationRequired: { [weak self] operation in
                    self?.serverManager.deploymentRequiresAuthentication(operation: operation)
                }
            )
            window.contentView = NSHostingView(rootView: wrapper)
            return
        }

        webView = nil
        if let failure = state.failureViewModel {
            window.contentView = NSHostingView(rootView: ErrorView(message: failure.message) { [weak self] in
                guard failure.allowsReconnect else { return }
                self?.serverManager.reconnect()
            })
        } else {
            window.contentView = NSHostingView(rootView: LoadingView(message: state.displayName))
        }
    }

    private func handleIncomingURL(_ url: URL) {
        guard let action = PhoenixURLAction(url: url) else { return }
        switch action {
        case .status: showServerStatusWindow()
        case .conversation(let id):
            pendingConversationID = id
            showWindow()
            openPendingConversation()
        case .open: showWindow()
        }
    }

    private func openPendingConversation() {
        guard let webView, let id = pendingConversationID, let origin = serverManager.webOrigin else { return }
        pendingConversationID = nil
        webView.load(URLRequest(url: origin.url(path: "/c/\(id.uuidString.lowercased())")))
    }
}

extension AppDelegate: NSWindowDelegate {
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if sender === statusWindow {
            statusWindow = nil
            return true
        }
        sender.orderOut(nil)
        return false
    }
}
