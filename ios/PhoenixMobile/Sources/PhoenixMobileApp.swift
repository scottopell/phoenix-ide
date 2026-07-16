import SwiftUI

@main
struct PhoenixMobileApp: App {
    @State private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase

    init() {
        let model = AppModel()
        _model = State(initialValue: model)
        // BGTask registration must complete before launch finishes.
        BackgroundRefresh.register(model: model)
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active:
                model.foregrounded()
            case .background:
                model.backgrounded()
            default:
                break
            }
        }
    }
}
