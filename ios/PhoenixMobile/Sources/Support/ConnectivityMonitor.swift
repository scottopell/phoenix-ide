import Foundation
import Network
import Observation

/// Publishes network reachability. Drives the offline banner and triggers
/// outbox drains + SSE reconnects when connectivity returns.
@MainActor
@Observable
final class ConnectivityMonitor {
    private(set) var isOnline = true
    private(set) var isConstrained = false

    /// Callbacks fired when the path transitions offline -> online.
    /// Weakly-held observation would be nicer, but the set of listeners is
    /// tiny (list store + active session) and they unregister on teardown.
    private var onRestore: [UUID: () -> Void] = [:]
    private var onLoss: [UUID: () -> Void] = [:]

    private let monitor = NWPathMonitor()
    #if DEBUG
    private var testingOverride = false
    #endif

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            Task { @MainActor [weak self] in
                self?.apply(path)
            }
        }
        monitor.start(queue: DispatchQueue(label: "phoenix.connectivity"))
    }

    private func apply(_ path: NWPath) {
        #if DEBUG
        guard !testingOverride else { return }
        #endif
        let nowOnline = path.status == .satisfied
        isConstrained = path.isConstrained || path.isExpensive
        let wasOnline = isOnline
        isOnline = nowOnline
        if !wasOnline && nowOnline {
            for callback in onRestore.values { callback() }
        } else if wasOnline && !nowOnline {
            for callback in onLoss.values { callback() }
        }
    }

    func addPathObserver(
        onRestore restore: @escaping () -> Void,
        onLoss loss: @escaping () -> Void
    ) -> UUID {
        let token = UUID()
        onRestore[token] = restore
        onLoss[token] = loss
        return token
    }

    func removePathObserver(_ token: UUID) {
        onRestore[token] = nil
        onLoss[token] = nil
    }

    /// Register a connectivity-restored callback. Returns a token; call
    /// `removeRestoreObserver` with it when the owner goes away.
    func addRestoreObserver(_ callback: @escaping () -> Void) -> UUID {
        let token = UUID()
        onRestore[token] = callback
        return token
    }

    func removeRestoreObserver(_ token: UUID) {
        onRestore[token] = nil
    }

    #if DEBUG
    func setOnlineForTesting(_ online: Bool) {
        testingOverride = true
        let wasOnline = isOnline
        isOnline = online
        if !wasOnline && online {
            for callback in onRestore.values { callback() }
        } else if wasOnline && !online {
            for callback in onLoss.values { callback() }
        }
    }
    #endif
}
