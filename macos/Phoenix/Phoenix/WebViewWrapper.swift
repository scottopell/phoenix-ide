import SwiftUI
import WebKit
import UserNotifications

struct WebViewWrapper: NSViewRepresentable {
    let origin: PhoenixOrigin
    let operation: ServerManager.ConnectionOperationToken
    let browserEnvironment: BrowserEnvironment
    let onWebViewReady: (WKWebView, ServerManager.ConnectionOperationToken) -> Void
    let onDeployment: (Result<DeploymentInfo, Error>, ServerManager.ConnectionOperationToken) -> Void
    let onAuthenticationRequired: (ServerManager.ConnectionOperationToken) -> Void

    private static let authenticationBridgeScript = """
    (() => {
      const reportDeployment = async () => {
        try {
          const response = await window.fetch('/api/deployment', { credentials: 'same-origin' });
          const body = response.ok ? await response.json() : null;
          window.webkit.messageHandlers.phoenixDeployment.postMessage({ status: response.status, body });
        } catch (_) {
          window.webkit.messageHandlers.phoenixDeployment.postMessage({ status: 0, body: null });
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

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
        let origin: PhoenixOrigin
        let role: BrowserSurfaceRole
        let operation: ServerManager.ConnectionOperationToken
        let browserEnvironment: BrowserEnvironment
        let onWebViewReady: (WKWebView, ServerManager.ConnectionOperationToken) -> Void
        let onDeployment: (Result<DeploymentInfo, Error>, ServerManager.ConnectionOperationToken) -> Void
        let onAuthenticationRequired: (ServerManager.ConnectionOperationToken) -> Void
        weak var webView: WKWebView?

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
            decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = navigationAction.request.url else {
                decisionHandler(.cancel)
                return
            }
            if role == .authPopup {
                switch PhoenixWebViewPolicy.popupNavigationDecision(url) {
                case .allowManagedChild: decisionHandler(.allow)
                case .externalize:
                    openExternallyIfSafe(url)
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
            decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void
        ) {
            if navigationResponse.canShowMIMEType {
                decisionHandler(.allow)
            } else {
                decisionHandler(.download)
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
            guard role == .primary else { return }
            onDeployment(.failure(error), operation)
        }

        func webView(
            _ webView: WKWebView,
            requestMediaCapturePermissionFor securityOrigin: WKSecurityOrigin,
            initiatedByFrame frame: WKFrameInfo,
            type: WKMediaCaptureType,
            decisionHandler: @escaping (WKPermissionDecision) -> Void
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
            decideNotificationPermissionFor securityOrigin: WKSecurityOrigin,
            decisionHandler: @escaping (WKPermissionDecision) -> Void
        ) {
            guard role == .primary else {
                decisionHandler(.deny)
                return
            }
            let notificationPolicy = PhoenixWebViewPolicy.notificationDecision(
                for: SecurityOriginDescriptor(scheme: securityOrigin.protocol, host: securityOrigin.host, port: securityOrigin.port),
                expectedOrigin: origin
            )
            guard notificationPolicy == .grant else {
                decisionHandler(.deny)
                return
            }
            UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) { granted, _ in
                DispatchQueue.main.async {
                    decisionHandler(granted ? .grant : .deny)
                }
            }
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
            guard role == .primary else { return }
            guard message.name == "phoenixDeployment",
                  let body = message.body as? [String: Any],
                  let status = body["status"] as? Int else { return }
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
            browserEnvironment.downloadManager.attach(download: download)
        }
    }
}

@MainActor
final class BrowserEnvironment {
    let popupManager = PopupWindowManager()
    let downloadManager = DownloadManager()

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
        let childWebView = WKWebView(frame: .zero, configuration: configuration)
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
        windows[identifier] = ManagedPopupWindow(window: window, coordinator: coordinator, webView: childWebView)
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        return childWebView
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
        let window: NSWindow
        let coordinator: WebViewWrapper.Coordinator
        let webView: WKWebView
    }
}

@MainActor
final class DownloadManager {
    private struct ActiveDownload {
        let download: WKDownload
        let delegate: DownloadDelegate
    }

    private var activeDownloads: [ObjectIdentifier: ActiveDownload] = [:]

    func attach(download: WKDownload) {
        let delegate = DownloadDelegate { [weak self] finishedDownload in
            self?.activeDownloads.removeValue(forKey: ObjectIdentifier(finishedDownload))
        }
        activeDownloads[ObjectIdentifier(download)] = ActiveDownload(download: download, delegate: delegate)
        download.delegate = delegate
    }

    func cancelAll() {
        let downloads = activeDownloads.values.map(\.download)
        activeDownloads.removeAll()
        downloads.forEach { download in
            download.cancel { _ in }
        }
    }

    private final class DownloadDelegate: NSObject, WKDownloadDelegate {
        private let onFinish: (WKDownload) -> Void

        init(onFinish: @escaping (WKDownload) -> Void) {
            self.onFinish = onFinish
        }

        func download(
            _ download: WKDownload,
            decideDestinationUsing response: URLResponse,
            suggestedFilename: String,
            completionHandler: @escaping (URL?) -> Void
        ) {
            let downloadsDirectory = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
                ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads", isDirectory: true)
            let destination = PhoenixDownloadNaming.uniqueDestination(
                in: downloadsDirectory,
                suggestedFilename: suggestedFilename,
                fileExists: { FileManager.default.fileExists(atPath: $0.path) }
            )
            completionHandler(destination)
        }

        func downloadDidFinish(_ download: WKDownload) {
            onFinish(download)
        }

        func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
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
