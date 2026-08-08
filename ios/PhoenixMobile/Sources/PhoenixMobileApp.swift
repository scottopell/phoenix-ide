import SwiftUI

@main
struct PhoenixMobileApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

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
