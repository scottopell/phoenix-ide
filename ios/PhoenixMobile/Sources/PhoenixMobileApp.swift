import SwiftUI

@main
struct PhoenixMobileApp: App {
    #if DEBUG
    private let fixtureRequested = FixtureAppLaunch.isRequested(in: ProcessInfo.processInfo.arguments)
    private let fixtureSelection = FixtureAppLaunch.selection(from: ProcessInfo.processInfo.arguments)
    #endif

    @State private var model: AppModel? = nil
    @Environment(\.scenePhase) private var scenePhase

    init() {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-ui-testing-reset") {
            AppModel.resetPersistentStateForUITesting()
        }
        guard !fixtureRequested else { return }
        #endif
        let model = AppModel()
        _model = State(initialValue: model)
        // BGTask registration must complete before launch finishes.
        BackgroundRefresh.register(model: model)
    }

    var body: some Scene {
        WindowGroup {
            appRoot
        }
        .onChange(of: scenePhase) { _, phase in
            guard let model else { return }
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

    @ViewBuilder
    private var appRoot: some View {
        #if DEBUG
        if fixtureRequested {
            if let fixtureSelection {
                if fixtureSelection == .catalog {
                    FixtureCatalogView()
                } else {
                    FixtureRootView(selection: fixtureSelection)
                }
            } else {
                InvalidFixtureView()
            }
        } else if let model {
            RootView()
                .environment(model)
        }
        #else
        if let model {
            RootView()
                .environment(model)
        }
        #endif
    }
}
