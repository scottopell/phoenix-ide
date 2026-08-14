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
                "Assets.xcassets",
                "Info.plist",
                "Phoenix.entitlements",
                "PhoenixApp.swift",
            ],
            sources: [
                "AppDelegate.swift",
                "Configuration.swift",
                "ErrorView.swift",
                "GlobalHotkey.swift",
                "LoadingView.swift",
                "ServerManager.swift",
                "ServerStatusView.swift",
                "WebViewWrapper.swift",
            ]
        ),
        .testTarget(
            name: "PhoenixMacCoreTests",
            dependencies: ["PhoenixMacCore"],
            path: "PhoenixTests"
        ),
    ]
)
