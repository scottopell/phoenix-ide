// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "PhoenixMacCore",
    platforms: [.macOS(.v15)],
    products: [.library(name: "PhoenixMacCore", targets: ["PhoenixMacCore"])],
    targets: [
        .target(
            name: "PhoenixMacCore",
            path: "Phoenix",
            exclude: [
                "AppDelegate.swift", "Assets.xcassets", "ErrorView.swift", "GlobalHotkey.swift",
                "Info.plist", "LoadingView.swift", "Phoenix.entitlements", "PhoenixApp.swift",
                "ServerManager.swift", "ServerStatusView.swift", "WebViewWrapper.swift",
            ],
            sources: ["Configuration.swift"]
        ),
        .testTarget(
            name: "PhoenixMacCoreTests",
            dependencies: ["PhoenixMacCore"],
            path: "PhoenixTests"
        ),
    ]
)
