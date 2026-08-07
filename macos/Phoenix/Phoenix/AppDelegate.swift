import Cocoa
import SwiftUI
import WebKit
import Combine

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var statusWindow: NSWindow?
    private var webView: WKWebView?
    private let hotkey = GlobalHotkeyManager()
    let serverManager = ServerManager()
    private var cancellables = Set<AnyCancellable>()
    private var pendingPrompts: [PendingPrompt] = []
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
        if !flag { showWindow() }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard case .bundled = serverManager.mode else { return .terminateNow }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Quit Phoenix and stop the bundled server?"
        alert.informativeText = "Any active conversations, tools, terminal sessions, and browser sessions owned by this bundled Phoenix may be interrupted."
        alert.addButton(withTitle: "Quit and Stop Phoenix")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return .terminateCancel }
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
            let wrapper = WebViewWrapper(
                origin: origin,
                onWebViewReady: { [weak self] value in
                    self?.webView = value
                    self?.processPendingPrompts()
                },
                onDeployment: { [weak self] result in
                    switch result {
                    case .success(let deployment): self?.serverManager.deploymentReceived(deployment)
                    case .failure(let error): self?.serverManager.deploymentVerificationFailed(error.localizedDescription)
                    }
                },
                onAuthenticationRequired: { [weak self] in
                    self?.serverManager.deploymentRequiresAuthentication()
                }
            )
            window.contentView = NSHostingView(rootView: wrapper)
            return
        }

        webView = nil
        switch state {
        case .failed(let message), .unavailable(let message), .tlsFailure(let message),
             .wrongService(let message), .unsupportedOwnership(let message):
            window.contentView = NSHostingView(rootView: ErrorView(message: message) { [weak self] in
                self?.serverManager.reconnect()
            })
        default:
            window.contentView = NSHostingView(rootView: LoadingView(message: state.displayName))
        }
    }

    private func handleIncomingURL(_ url: URL) {
        guard let action = PhoenixURLAction(url: url) else { return }
        switch action {
        case .status: showServerStatusWindow()
        case .new(let prompt, let cwd):
            showWindow()
            if !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                pendingPrompts.append(PendingPrompt(prompt: prompt, cwd: cwd))
                processPendingPrompts()
            }
        case .open: showWindow()
        }
    }

    private func processPendingPrompts() {
        guard let webView, !pendingPrompts.isEmpty else { return }
        let pending = pendingPrompts.removeFirst()
        let cwd = NSString(string: pending.cwd ?? NSHomeDirectory()).expandingTildeInPath
        let body: [String: Any] = [
            "cwd": cwd,
            "text": "",
            "message_id": UUID().uuidString,
            "images": [],
            "mode": "direct",
            "seed_label": "External prompt",
        ]
        let script = """
        const response = await fetch('/api/conversations/new', {
          method: 'POST', credentials: 'same-origin',
          headers: {'Content-Type': 'application/json'}, body: JSON.stringify(payload)
        });
        if (!response.ok) throw new Error(`conversation create failed: ${response.status}`);
        const result = await response.json();
        localStorage.setItem(`seed-draft:${result.conversation.id}`, prompt);
        window.location.assign(`/c/${result.conversation.slug}`);
        """
        webView.callAsyncJavaScript(
            script,
            arguments: ["payload": body, "prompt": pending.prompt],
            in: nil,
            in: .page,
            completionHandler: { [weak self] (result: Result<Any, Error>) in
            if case .failure(let error) = result {
                NSLog("Phoenix URL handoff failed: \(error.localizedDescription)")
            }
            self?.processPendingPrompts()
        })
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

private struct PendingPrompt {
    let prompt: String
    let cwd: String?
}
