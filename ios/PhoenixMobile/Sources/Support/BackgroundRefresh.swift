import BackgroundTasks
import Foundation

/// BGAppRefresh plumbing for the stopgap nudge tier (see AttentionMonitor's
/// header: APNs on durable inbox observations is the intended end state;
/// this file exists to be deleted when that lands).
///
/// iOS owns the schedule: `earliestBeginDate` is a floor, not a promise,
/// and runs are skipped entirely under low power or poor app-usage signal.
/// Correctness never depends on a run happening — a missed window just
/// means the user finds out when they next open the app, exactly as before
/// this tier existed.
enum BackgroundRefresh {
    /// Must match BGTaskSchedulerPermittedIdentifiers in project.yml.
    static let taskIdentifier = "com.phoenix.mobile.refresh"

    /// Call exactly once, before app launch completes (App.init).
    static func register(model: AppModel) {
        BGTaskScheduler.shared.register(
            forTaskWithIdentifier: taskIdentifier, using: nil
        ) { task in
            guard let task = task as? BGAppRefreshTask else {
                task.setTaskCompleted(success: false)
                return
            }
            let work = Task { @MainActor in
                let ok = await model.runBackgroundAttentionCheck()
                task.setTaskCompleted(success: ok)
                if model.backgroundNudgesEnabled {
                    scheduleNext()
                }
            }
            task.expirationHandler = {
                work.cancel()
            }
        }
    }

    static func scheduleNext() {
        let request = BGAppRefreshTaskRequest(identifier: taskIdentifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        // Submit failures (simulator, duplicate pending request) are
        // non-fatal by design — see header.
        try? BGTaskScheduler.shared.submit(request)
    }

    static func cancelPending() {
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: taskIdentifier)
    }
}
