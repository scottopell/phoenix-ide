import SwiftUI
import WebKit

struct WebViewWrapper: NSViewRepresentable {
    let origin: PhoenixOrigin
    let onWebViewReady: (WKWebView) -> Void
    let onDeployment: (Result<DeploymentInfo, Error>) -> Void
    let onAuthenticationRequired: () -> Void

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
            onWebViewReady: onWebViewReady,
            onDeployment: onDeployment,
            onAuthenticationRequired: onAuthenticationRequired
        )
    }

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
        let origin: PhoenixOrigin
        let onWebViewReady: (WKWebView) -> Void
        let onDeployment: (Result<DeploymentInfo, Error>) -> Void
        let onAuthenticationRequired: () -> Void
        weak var webView: WKWebView?

        init(
            origin: PhoenixOrigin,
            onWebViewReady: @escaping (WKWebView) -> Void,
            onDeployment: @escaping (Result<DeploymentInfo, Error>) -> Void,
            onAuthenticationRequired: @escaping () -> Void
        ) {
            self.origin = origin
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
            if navigationAction.navigationType == .linkActivated && !origin.exactlyMatches(url) {
                let safeExternalSchemes = ["http", "https", "mailto", "tel"]
                if let scheme = url.scheme?.lowercased(), safeExternalSchemes.contains(scheme) {
                    NSWorkspace.shared.open(url)
                }
                decisionHandler(.cancel)
            } else {
                decisionHandler(.allow)
            }
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            onWebViewReady(webView)
            verifyDeployment(webView)
        }

        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation!,
            withError error: Error
        ) {
            onDeployment(.failure(error))
        }

        func webView(
            _ webView: WKWebView,
            requestMediaCapturePermissionFor securityOrigin: WKSecurityOrigin,
            initiatedByFrame frame: WKFrameInfo,
            type: WKMediaCaptureType,
            decisionHandler: @escaping (WKPermissionDecision) -> Void
        ) {
            guard let candidate = URL(string: "\(securityOrigin.protocol)://\(securityOrigin.host):\(securityOrigin.port)") else {
                decisionHandler(.deny)
                return
            }
            decisionHandler(origin.exactlyMatches(candidate) ? .grant : .deny)
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.name == "phoenixDeployment",
                  let body = message.body as? [String: Any],
                  let status = body["status"] as? Int else { return }
            if status == 401 {
                onAuthenticationRequired()
                return
            }
            guard status == 200, let value = body["body"] else {
                onDeployment(.failure(WebViewVerificationError.httpStatus(status)))
                return
            }
            do {
                let data = try JSONSerialization.data(withJSONObject: value)
                onDeployment(.success(try JSONDecoder().decode(DeploymentInfo.self, from: data)))
            } catch {
                onDeployment(.failure(error))
            }
        }

        private func verifyDeployment(_ webView: WKWebView) {
            webView.evaluateJavaScript("window.__phoenixMacReportDeployment?.()")
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
