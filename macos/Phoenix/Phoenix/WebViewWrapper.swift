import SwiftUI
import WebKit
import UserNotifications
import CryptoKit

enum WebKitStoragePartition: Equatable {
    case attached(PhoenixOrigin)
    case bundled

    init(serverMode: ServerModeKind, origin: PhoenixOrigin) {
        switch serverMode {
        case .attached: self = .attached(origin)
        case .bundled: self = .bundled
        }
    }

    static let bundledPersistentIdentifier = UUID(uuidString: "DB535D27-91C2-42A0-9DF7-DC959C80D4D4")!

    private var attachedPersistentIdentifier: UUID {
        guard case .attached(let origin) = self else { return Self.bundledPersistentIdentifier }
        let digest = Array(SHA256.hash(data: Data("phoenix-attached-webkit:\(origin.canonicalStorageKey)".utf8)))
        let bytes: uuid_t = (
            digest[0], digest[1], digest[2], digest[3],
            digest[4], digest[5], digest[6], digest[7],
            digest[8], digest[9], digest[10], digest[11],
            digest[12], digest[13], digest[14], digest[15]
        )
        return UUID(uuid: bytes)
    }

    @MainActor
    var dataStore: WKWebsiteDataStore {
        switch self {
        case .attached: WKWebsiteDataStore(forIdentifier: attachedPersistentIdentifier)
        case .bundled: WKWebsiteDataStore(forIdentifier: Self.bundledPersistentIdentifier)
        }
    }
}

struct WebViewWrapper: NSViewRepresentable {
    let origin: PhoenixOrigin
    let storagePartition: WebKitStoragePartition
    let operation: ServerManager.ConnectionOperationToken
    let browserEnvironment: BrowserEnvironment
    let onWebViewReady: (WKWebView, ServerManager.ConnectionOperationToken) -> Void
    let onDeployment: (Result<DeploymentInfo, Error>, ServerManager.ConnectionOperationToken) -> Void
    let onAuthenticationRequired: (ServerManager.ConnectionOperationToken) -> Void

    private static let authenticationBridgeScript = """
    (() => {
      const deploymentDocumentGeneration = crypto.randomUUID();
      let deploymentRequestSequence = 0;
      const reportDeployment = async () => {
        const sequence = ++deploymentRequestSequence;
        try {
          const response = await window.fetch('/api/deployment', { credentials: 'same-origin' });
          const body = response.ok ? await response.json() : null;
          window.webkit.messageHandlers.phoenixDeployment.postMessage({ generation: deploymentDocumentGeneration, sequence, status: response.status, body });
        } catch (_) {
          window.webkit.messageHandlers.phoenixDeployment.postMessage({ generation: deploymentDocumentGeneration, sequence, status: 0, body: null });
        }
      };
      window.__phoenixMacReportDeployment = reportDeployment;
      const originalFetch = window.fetch.bind(window);
      window.fetch = async (...args) => {
        const response = await originalFetch(...args);
        const target = typeof args[0] === 'string' ? args[0] : args[0]?.url;
        if (response.ok && target && new URL(target, window.location.href).pathname === '/api/auth/login') {
          queueMicrotask(reportDeployment);
        }
        return response;
      };
    })();
    """

    func makeNSView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = storagePartition.dataStore
        configuration.preferences.setValue(true, forKey: "developerExtrasEnabled")
        configuration.mediaTypesRequiringUserActionForPlayback = []
        configuration.userContentController.add(context.coordinator, name: "phoenixDeployment")
        configuration.userContentController.addUserScript(WKUserScript(
            source: Self.authenticationBridgeScript,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true
        ))

        let webView = WKWebView(frame: .zero, configuration: configuration)
        context.coordinator.webView = webView
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.load(URLRequest(url: origin.url))
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {}

    static func dismantleNSView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "phoenixDeployment")
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(
            origin: origin,
            operation: operation,
            browserEnvironment: browserEnvironment,
            onWebViewReady: onWebViewReady,
            onDeployment: onDeployment,
            onAuthenticationRequired: onAuthenticationRequired
        )
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
        let origin: PhoenixOrigin
        let role: BrowserSurfaceRole
        let operation: ServerManager.ConnectionOperationToken
        let browserEnvironment: BrowserEnvironment
        let onWebViewReady: (WKWebView, ServerManager.ConnectionOperationToken) -> Void
        let onDeployment: (Result<DeploymentInfo, Error>, ServerManager.ConnectionOperationToken) -> Void
        let onAuthenticationRequired: (ServerManager.ConnectionOperationToken) -> Void
        weak var webView: WKWebView?
        private var latestDeploymentGeneration: String?
        private var retiredDeploymentGenerations = Set<String>()
        private var latestDeploymentSequence = 0

        init(
            origin: PhoenixOrigin,
            role: BrowserSurfaceRole = .primary,
            operation: ServerManager.ConnectionOperationToken,
            browserEnvironment: BrowserEnvironment,
            onWebViewReady: @escaping (WKWebView, ServerManager.ConnectionOperationToken) -> Void,
            onDeployment: @escaping (Result<DeploymentInfo, Error>, ServerManager.ConnectionOperationToken) -> Void,
            onAuthenticationRequired: @escaping (ServerManager.ConnectionOperationToken) -> Void
        ) {
            self.origin = origin
            self.role = role
            self.operation = operation
            self.browserEnvironment = browserEnvironment
            self.onWebViewReady = onWebViewReady
            self.onDeployment = onDeployment
            self.onAuthenticationRequired = onAuthenticationRequired
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = navigationAction.request.url else {
                decisionHandler(.cancel)
                return
            }
            if role == .authPopup {
                switch PhoenixWebViewPolicy.popupNavigationDecision(url, expectedOrigin: origin) {
                case .allowManagedChild: decisionHandler(.allow)
                case .externalize:
                    openExternallyIfSafe(url)
                    browserEnvironment.popupManager.webViewDidClose(webView)
                    decisionHandler(.cancel)
                case .cancel: decisionHandler(.cancel)
                }
                return
            }
            if navigationAction.shouldPerformDownload {
                decisionHandler(.download)
                return
            }
            if navigationAction.targetFrame == nil {
                switch PhoenixWebViewPolicy.popupDecision(
                    requestURL: url,
                    sourceURL: navigationAction.sourceFrame.request.url,
                    expectedOrigin: origin
                ) {
                case .allowManagedChild:
                    decisionHandler(.allow)
                case .externalize:
                    openExternallyIfSafe(url)
                    decisionHandler(.cancel)
                case .cancel:
                    decisionHandler(.cancel)
                }
                return
            }
            if navigationAction.targetFrame?.isMainFrame == true && !origin.exactlyMatches(url) {
                if navigationAction.navigationType == .linkActivated {
                    openExternallyIfSafe(url)
                }
                decisionHandler(.cancel)
                return
            }
            if navigationAction.navigationType == .linkActivated && !origin.exactlyMatches(url) {
                openExternallyIfSafe(url)
                decisionHandler(.cancel)
            } else {
                decisionHandler(.allow)
            }
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationResponse: WKNavigationResponse,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationResponsePolicy) -> Void
        ) {
            switch PhoenixNavigationResponsePolicy.decide(
                role: role,
                responseURL: navigationResponse.response.url,
                canShowMIMEType: navigationResponse.canShowMIMEType,
                expectedOrigin: origin
            ) {
            case .allow:
                decisionHandler(.allow)
            case .download:
                decisionHandler(.download)
            case .externalize(let url):
                openExternallyIfSafe(url)
                browserEnvironment.popupManager.webViewDidClose(webView)
                decisionHandler(.cancel)
            case .cancel:
                decisionHandler(.cancel)
            }
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            guard role == .primary else { return }
            onWebViewReady(webView, operation)
            verifyDeployment(webView)
        }

        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation!,
            withError error: Error
        ) {
            guard role == .primary, WebViewNavigationFailureDecision.shouldReport(error) else { return }
            onDeployment(.failure(error), operation)
        }

        func webView(
            _ webView: WKWebView,
            didFail navigation: WKNavigation!,
            withError error: Error
        ) {
            guard role == .primary, WebViewNavigationFailureDecision.shouldReport(error) else { return }
            onDeployment(.failure(error), operation)
        }

        func webView(
            _ webView: WKWebView,
            requestMediaCapturePermissionFor securityOrigin: WKSecurityOrigin,
            initiatedByFrame frame: WKFrameInfo,
            type: WKMediaCaptureType,
            decisionHandler: @escaping @MainActor @Sendable (WKPermissionDecision) -> Void
        ) {
            guard role == .primary else {
                decisionHandler(.deny)
                return
            }
            let policy = PhoenixWebViewPolicy.mediaCaptureDecision(
                for: SecurityOriginDescriptor(scheme: securityOrigin.protocol, host: securityOrigin.host, port: securityOrigin.port),
                captureType: mediaCaptureKind(for: type),
                expectedOrigin: origin
            )
            decisionHandler(policy == .grant ? .grant : .deny)
        }

        @available(macOS 15.0, *)
        func webView(
            _ webView: WKWebView,
            decideNotificationPermissionFor securityOrigin: WKSecurityOrigin
        ) async -> WKPermissionDecision {
            guard role == .primary else {
                return .deny
            }
            let notificationPolicy = PhoenixWebViewPolicy.notificationDecision(
                for: SecurityOriginDescriptor(scheme: securityOrigin.protocol, host: securityOrigin.host, port: securityOrigin.port),
                expectedOrigin: origin
            )
            guard notificationPolicy == .grant else {
                return .deny
            }
            let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound])) ?? false
            return granted ? .grant : .deny
        }

        func webView(
            _ webView: WKWebView,
            runJavaScriptConfirmPanelWithMessage message: String,
            initiatedByFrame frame: WKFrameInfo
        ) async -> Bool {
            guard role == .primary,
                  frame.isMainFrame,
                  let frameURL = frame.request.url,
                  origin.exactlyMatches(frameURL) else { return false }
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = message
            alert.addButton(withTitle: "OK")
            alert.addButton(withTitle: "Cancel")
            return alert.runModal() == .alertFirstButtonReturn
        }

        func webView(
            _ webView: WKWebView,
            createWebViewWith configuration: WKWebViewConfiguration,
            for navigationAction: WKNavigationAction,
            windowFeatures: WKWindowFeatures
        ) -> WKWebView? {
            switch PhoenixWebViewPolicy.popupDecision(
            requestURL: navigationAction.request.url,
            sourceURL: navigationAction.sourceFrame.request.url,
            expectedOrigin: origin
            ) {
            case .allowManagedChild:
                return browserEnvironment.popupManager.makeChildWebView(
                    configuration: configuration,
                    origin: origin,
                    role: .authPopup,
                    operation: operation,
                    browserEnvironment: browserEnvironment,
                    onDeployment: onDeployment,
                    onAuthenticationRequired: onAuthenticationRequired
                )
            case .externalize:
                if let url = navigationAction.request.url {
                    openExternallyIfSafe(url)
                }
                return nil
            case .cancel:
                return nil
            }
        }

        func webView(_ webView: WKWebView, navigationAction: WKNavigationAction, didBecome download: WKDownload) {
            attachDownloadDelegate(to: download)
        }

        func webView(_ webView: WKWebView, navigationResponse: WKNavigationResponse, didBecome download: WKDownload) {
            attachDownloadDelegate(to: download)
        }

        func webViewDidClose(_ webView: WKWebView) {
            browserEnvironment.popupManager.webViewDidClose(webView)
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard role == .primary,
                  message.webView === webView,
                  message.frameInfo.isMainFrame,
                  let sourceURL = message.frameInfo.request.url,
                  origin.exactlyMatches(sourceURL) else { return }
            guard message.name == "phoenixDeployment",
                  let body = message.body as? [String: Any],
                  let generation = body["generation"] as? String,
                  let sequence = body["sequence"] as? Int,
                  let status = body["status"] as? Int else { return }
            guard !retiredDeploymentGenerations.contains(generation) else { return }
            if generation == latestDeploymentGeneration {
                guard sequence > latestDeploymentSequence else { return }
            } else {
                if let latestDeploymentGeneration {
                    retiredDeploymentGenerations.insert(latestDeploymentGeneration)
                }
                latestDeploymentGeneration = generation
                latestDeploymentSequence = 0
            }
            latestDeploymentSequence = sequence
            if status == 401 {
                onAuthenticationRequired(operation)
                return
            }
            guard status == 200, let value = body["body"] else {
                onDeployment(.failure(WebViewVerificationError.httpStatus(status)), operation)
                return
            }
            do {
                let data = try JSONSerialization.data(withJSONObject: value)
                onDeployment(.success(try JSONDecoder().decode(DeploymentInfo.self, from: data)), operation)
            } catch {
                onDeployment(.failure(error), operation)
            }
        }

        private func verifyDeployment(_ webView: WKWebView) {
            webView.evaluateJavaScript("window.__phoenixMacReportDeployment?.()")
        }

        private func mediaCaptureKind(for type: WKMediaCaptureType) -> MediaCaptureKind {
            switch type {
            case .camera:
                return .camera
            case .microphone:
                return .microphone
            case .cameraAndMicrophone:
                return .cameraAndMicrophone
            @unknown default:
                return .unknown
            }
        }

        private func openExternallyIfSafe(_ url: URL) {
            if PhoenixWebViewPolicy.safeToExternalize(url) {
                NSWorkspace.shared.open(url)
            }
        }

        private func attachDownloadDelegate(to download: WKDownload) {
            guard role == .primary else {
                download.cancel { _ in }
                return
            }
            browserEnvironment.downloadManager.attach(download: download, operation: operation)
        }
    }
}

@MainActor
final class BrowserEnvironment {
    let popupManager = PopupWindowManager()
    let downloadManager = DownloadManager()

    func closeOperationOwnedSurfaces(for operation: ServerManager.ConnectionOperationToken) {
        popupManager.closeAll(for: operation)
        downloadManager.cancelAll(for: operation)
    }

    func shutdown() {
        popupManager.closeAll()
        downloadManager.cancelAll()
    }
}

@MainActor
final class PopupWindowManager: NSObject, NSWindowDelegate {
    private var windows: [ObjectIdentifier: ManagedPopupWindow] = [:]

    func makeChildWebView(
        configuration: WKWebViewConfiguration,
        origin: PhoenixOrigin,
        role: BrowserSurfaceRole,
        operation: ServerManager.ConnectionOperationToken,
        browserEnvironment: BrowserEnvironment,
        onDeployment: @escaping (Result<DeploymentInfo, Error>, ServerManager.ConnectionOperationToken) -> Void,
        onAuthenticationRequired: @escaping (ServerManager.ConnectionOperationToken) -> Void
    ) -> WKWebView {
        let isolatedConfiguration = configuration.copy() as! WKWebViewConfiguration
        isolatedConfiguration.userContentController = WKUserContentController()
        let childWebView = WKWebView(frame: .zero, configuration: isolatedConfiguration)
        let coordinator = WebViewWrapper.Coordinator(
            origin: origin,
            role: role,
            operation: operation,
            browserEnvironment: browserEnvironment,
            onWebViewReady: { _, _ in },
            onDeployment: { _, _ in },
            onAuthenticationRequired: { _ in }
        )
        coordinator.webView = childWebView
        childWebView.navigationDelegate = coordinator
        childWebView.uiDelegate = coordinator

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 980, height: 720),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Phoenix Sign In"
        window.isReleasedWhenClosed = false
        window.contentView = childWebView
        window.delegate = self
        let identifier = ObjectIdentifier(window)
        windows[identifier] = ManagedPopupWindow(operation: operation, window: window, coordinator: coordinator, webView: childWebView)
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        return childWebView
    }

    func closeAll(for operation: ServerManager.ConnectionOperationToken) {
        let matching = windows.filter { $0.value.operation == operation }
        let identifiers = Set(matching.keys)
        let activeWindows = matching.values.map(\.window)
        windows = windows.filter { !identifiers.contains($0.key) }
        activeWindows.forEach { $0.close() }
    }

    func closeAll() {
        let activeWindows = windows.values.map(\.window)
        windows.removeAll()
        activeWindows.forEach { $0.close() }
    }

    func webViewDidClose(_ webView: WKWebView) {
        guard let entry = windows.first(where: { $0.value.webView === webView }) else { return }
        entry.value.window.close()
        windows.removeValue(forKey: entry.key)
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow else { return }
        windows.removeValue(forKey: ObjectIdentifier(window))
    }

    private struct ManagedPopupWindow {
        let operation: ServerManager.ConnectionOperationToken
        let window: NSWindow
        let coordinator: WebViewWrapper.Coordinator
        let webView: WKWebView
    }
}

struct DownloadDestinationReservationState {
    private(set) var reservedPaths: Set<String> = []

    mutating func reserveDestination(in directory: URL, suggestedFilename: String, fileExists: (URL) -> Bool) -> URL {
        let sanitized = PhoenixDownloadNaming.sanitizedFilename(suggestedFilename)
        var suffix = 0
        while true {
            let candidateName = PhoenixDownloadNaming.collisionSafeFilename(
                sanitized,
                collisionIndex: suffix == 0 ? nil : suffix
            )
            let candidate = directory.appendingPathComponent(candidateName)
            let reservationKey = Self.reservationKey(for: candidate)
            if !reservedPaths.contains(reservationKey) && !fileExists(candidate) {
                reservedPaths.insert(reservationKey)
                return candidate
            }
            suffix += 1
        }
    }

    mutating func release(_ url: URL?) {
        guard let url else { return }
        reservedPaths.remove(Self.reservationKey(for: url))
    }

    private static func reservationKey(for url: URL) -> String {
        url.standardizedFileURL.path.precomposedStringWithCanonicalMapping.lowercased()
    }
}

@MainActor
final class DownloadManager {
    private struct ActiveDownload {
        let operation: ServerManager.ConnectionOperationToken
        let download: WKDownload
        let delegate: DownloadDelegate
        var reservedDestination: URL?
    }

    private var activeDownloads: [ObjectIdentifier: ActiveDownload] = [:]
    private var reservations = DownloadDestinationReservationState()

    func attach(download: WKDownload, operation: ServerManager.ConnectionOperationToken) {
        let delegate = DownloadDelegate(
            reserveDestination: { [weak self] download, response, suggestedFilename in
                self?.reserveDestination(for: download, response: response, suggestedFilename: suggestedFilename)
            },
            onFinish: { [weak self] finishedDownload in
                self?.finishDownload(finishedDownload)
            },
            onFailure: { [weak self] failedDownload, error in
                self?.presentDownloadFailure(error)
                self?.finishDownload(failedDownload)
            }
        )
        activeDownloads[ObjectIdentifier(download)] = ActiveDownload(operation: operation, download: download, delegate: delegate, reservedDestination: nil)
        download.delegate = delegate
    }

    func cancelAll(for operation: ServerManager.ConnectionOperationToken) {
        let downloads = activeDownloads.values
            .filter { $0.operation == operation }
            .map(\.download)
        downloads.forEach { [weak self] download in
            download.cancel { _ in
                Task { @MainActor in self?.finishDownload(download) }
            }
        }
    }

    func cancelAll() {
        let downloads = activeDownloads.values.map(\.download)
        downloads.forEach { [weak self] download in
            download.cancel { _ in
                Task { @MainActor in self?.finishDownload(download) }
            }
        }
    }

    private func reserveDestination(for download: WKDownload, response _: URLResponse, suggestedFilename: String) -> URL {
        let downloadsDirectory = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads", isDirectory: true)
        let identifier = ObjectIdentifier(download)
        if let existingReservation = activeDownloads[identifier]?.reservedDestination {
            reservations.release(existingReservation)
        }
        let destination = reservations.reserveDestination(
            in: downloadsDirectory,
            suggestedFilename: suggestedFilename,
            fileExists: { FileManager.default.fileExists(atPath: $0.path) }
        )
        if var entry = activeDownloads[identifier] {
            entry.reservedDestination = destination
            activeDownloads[identifier] = entry
        }
        return destination
    }

    private func presentDownloadFailure(_ error: Error) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Download failed"
        alert.informativeText = error.localizedDescription
        alert.runModal()
    }

    private func finishDownload(_ download: WKDownload) {
        let identifier = ObjectIdentifier(download)
        let reservation = activeDownloads.removeValue(forKey: identifier)?.reservedDestination
        reservations.release(reservation)
    }

    private final class DownloadDelegate: NSObject, WKDownloadDelegate {
        private let reserveDestination: (WKDownload, URLResponse, String) -> URL?
        private let onFinish: (WKDownload) -> Void
        private let onFailure: (WKDownload, Error) -> Void

        init(
            reserveDestination: @escaping (WKDownload, URLResponse, String) -> URL?,
            onFinish: @escaping (WKDownload) -> Void,
            onFailure: @escaping (WKDownload, Error) -> Void
        ) {
            self.reserveDestination = reserveDestination
            self.onFinish = onFinish
            self.onFailure = onFailure
        }

        @available(macOS 11.3, *)
        func download(
            _ download: WKDownload,
            decideDestinationUsing response: URLResponse,
            suggestedFilename: String
        ) async -> URL? {
            reserveDestination(download, response, suggestedFilename)
        }


        func downloadDidFinish(_ download: WKDownload) {
            onFinish(download)
        }

        func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
            onFailure(download, error)
        }

        func downloadDidCancel(_ download: WKDownload) {
            onFinish(download)
        }
    }
}

enum WebViewVerificationError: LocalizedError {
    case httpStatus(Int)

    var errorDescription: String? {
        switch self {
        case .httpStatus(let status): "Deployment verification failed with HTTP status \(status)."
        }
    }
}
