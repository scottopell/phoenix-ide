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
    private var browserOperation: ServerManager.ConnectionOperationToken?
    private let hotkey = GlobalHotkeyManager()
    let serverManager = ServerManager()
    private let persistence = SettingsPersistence()
    private var cancellables = Set<AnyCancellable>()
    private var pendingConversationID: UUID?
    private var pendingConversationValidationTask: Task<Void, Never>?
    private var isPrimaryWebViewAuthenticated = false
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
        let hasSavedModeSelection = persistence.loadConnectionDraft().hasSavedModeSelection
        if hasSavedModeSelection {
            serverManager.connect()
        } else if FirstRunDecision.shouldOpenSettings(hasSavedModeSelection: hasSavedModeSelection) {
            DispatchQueue.main.async {
                NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
            }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if ReopenDecision.shouldShowMainWindow(mainWindowIsVisible: window?.isVisible == true) {
            showWindow()
        }
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
        serverManager.beginTermination()
        serverManager.stop { [browserEnvironment] in
            browserEnvironment.shutdown()
            sender.reply(toApplicationShouldTerminate: true)
        }
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
        let currentOperation = serverManager.currentOperationToken
        let staleBrowserOperation = browserOperation != nil && browserOperation != currentOperation
        if state.canDisplayWebView, let origin = serverManager.webOrigin {
            let operation = currentOperation
            if staleBrowserOperation, let browserOperation {
                browserEnvironment.closeOperationOwnedSurfaces(for: browserOperation)
                self.browserOperation = nil
                webView = nil
                isPrimaryWebViewAuthenticated = false
            }
            if webView != nil { return }
            isPrimaryWebViewAuthenticated = false
            browserOperation = operation
            let wrapper = WebViewWrapper(
                origin: origin,
                operation: operation,
                browserEnvironment: browserEnvironment,
                onWebViewReady: { [weak self] value, operation in
                    guard let self, self.isCurrentBrowserOperation(operation) else { return }
                    self.webView = value
                },
                onDeployment: { [weak self] result, operation in
                    guard let self, self.isCurrentBrowserOperation(operation) else { return }
                    switch result {
                    case .success(let deployment):
                        self.isPrimaryWebViewAuthenticated = true
                        self.serverManager.deploymentReceived(deployment, operation: operation)
                        self.validateQueuedConversationNavigationIfPossible()
                    case .failure(let error): self.serverManager.deploymentVerificationFailed(error.localizedDescription, operation: operation)
                    }
                },
                onAuthenticationRequired: { [weak self] operation in
                    guard let self, self.isCurrentBrowserOperation(operation) else { return }
                    self.isPrimaryWebViewAuthenticated = false
                    self.pendingConversationValidationTask?.cancel()
                    self.serverManager.deploymentRequiresAuthentication(operation: operation)
                    self.validateQueuedConversationNavigationIfPossible()
                }
            )
            window.contentView = NSHostingView(rootView: wrapper)
            return
        }

        if let browserOperation, browserOperation != currentOperation {
            pendingConversationValidationTask?.cancel()
            browserEnvironment.closeOperationOwnedSurfaces(for: browserOperation)
            self.browserOperation = nil
        }
        if let current = webView {
            current.navigationDelegate = nil
            current.uiDelegate = nil
        }
        webView = nil
        isPrimaryWebViewAuthenticated = false
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
        case .status:
            showServerStatusWindow()
            NSApp.activate(ignoringOtherApps: true)
        case .conversation(let id):
            showWindow()
            validateAndQueueConversationNavigation(id)
        case .open:
            showWindow()
        }
    }

    private func isCurrentBrowserOperation(_ operation: ServerManager.ConnectionOperationToken) -> Bool {
        browserOperation == operation && serverManager.currentOperationToken == operation
    }

    private func openPendingConversation() {
        guard isPrimaryWebViewAuthenticated,
              let webView,
              let id = pendingConversationID,
              let origin = serverManager.webOrigin else { return }
        pendingConversationID = nil
        webView.load(URLRequest(url: origin.url(path: "/c/\(id.uuidString.lowercased())")))
    }

    private func validateAndQueueConversationNavigation(_ id: UUID) {
        pendingConversationID = id
        validateQueuedConversationNavigationIfPossible()
    }

    private func validateQueuedConversationNavigationIfPossible() {
        guard DeepLinkNavigationDecision.shouldValidateQueuedConversation(
            pendingConversationID: pendingConversationID,
            hasAuthenticatedPrimaryWebView: isPrimaryWebViewAuthenticated,
            hasPrimaryWebView: webView != nil,
            hasConfiguredOrigin: serverManager.webOrigin != nil
        ), let queued = pendingConversationID else { return }
        let operation = serverManager.currentOperationToken
        guard let validationWebView = webView, let validationOrigin = serverManager.webOrigin else { return }
        pendingConversationValidationTask?.cancel()
        pendingConversationValidationTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let conversationID = try await self.validatedConversationIDForNavigation(
                    queued,
                    webView: validationWebView,
                    origin: validationOrigin
                )
                guard !Task.isCancelled,
                      self.isCurrentBrowserOperation(operation),
                      self.browserOperation == operation,
                      self.serverManager.webOrigin == validationOrigin else { return }
                guard DeepLinkNavigationDecision.validationResultIsCurrent(
                    validatedID: queued,
                    pendingConversationID: self.pendingConversationID
                ) else { return }
                self.pendingConversationID = conversationID
                self.openPendingConversation()
            } catch {
                guard !Task.isCancelled,
                      self.isCurrentBrowserOperation(operation),
                      self.browserOperation == operation,
                      self.serverManager.webOrigin == validationOrigin else { return }
                guard DeepLinkNavigationDecision.validationResultIsCurrent(
                    validatedID: queued,
                    pendingConversationID: self.pendingConversationID
                ) else { return }
                let decision = DeepLinkValidationOutcome.evaluate(error as? DeepLinkValidationError ?? .decoding(error.localizedDescription))
                if !DeepLinkNavigationDecision.validationResultIsCurrent(
                    validatedID: queued,
                    pendingConversationID: self.pendingConversationID
                ) {
                    return
                }
                if decision.shouldClearAuthenticationGate {
                    self.isPrimaryWebViewAuthenticated = false
                }
                if !decision.shouldRetainPendingConversation {
                    self.pendingConversationID = nil
                }
                let message = error.localizedDescription
                NSLog("Phoenix deep link rejected for %s: %s", queued.uuidString.lowercased(), message)
                if !decision.shouldRetainPendingConversation {
                    self.presentDeepLinkError(message)
                }
            }
        }
    }

    private func presentDeepLinkError(_ message: String) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Conversation could not be opened"
        alert.informativeText = message
        alert.runModal()
    }

    private func validatedConversationIDForNavigation(
        _ id: UUID,
        webView: WKWebView,
        origin: PhoenixOrigin
    ) async throws -> UUID {
        let script = Self.conversationRouteValidationScript(uuid: id.uuidString.lowercased())
        let result = try await webView.evaluateJavaScript(script)
        guard let payload = result as? [String: Any],
              let status = payload["status"] as? Int else {
            throw DeepLinkValidationError.decoding("missing status payload")
        }
        if status == 401 {
            throw DeepLinkValidationError.authenticationRequired
        }
        if status == 404 {
            throw DeepLinkValidationError.conversationMissing(id)
        }
        guard status == 200 else {
            throw DeepLinkValidationError.invalidHTTPStatus(status)
        }
        guard let body = payload["body"],
              let validated = DeepLinkConversationValidation.extractConversationID(fromRouteBody: body),
              validated.uuidString.lowercased() == id.uuidString.lowercased() else {
            throw DeepLinkValidationError.decoding("conversation response did not contain the requested UUID")
        }
        guard let finalURLString = payload["finalURL"] as? String,
              let finalURL = URL(string: finalURLString),
              origin.exactlyMatches(finalURL) else {
            throw DeepLinkValidationError.decoding("validation escaped the configured origin")
        }
        return validated
    }

    private static func conversationRouteValidationScript(uuid: String) -> String {
        let path = "/api/conversations/\(uuid)/route"
        return """
        (async () => {
          const response = await window.fetch(\"\(path)\", { credentials: 'same-origin' });
          let body = null;
          try { body = await response.json(); } catch (_) {}
          return { status: response.status, body, finalURL: response.url };
        })();
        """
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
