// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Canopy",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Canopy", targets: ["Canopy"]),
        .library(name: "CanopyCore", targets: ["CanopyCore"]),
    ],
    targets: [
        // Platform-neutral model: layout tree, solver, drop resolution,
        // navigation, commands, persistence. Foundation only.
        .target(name: "CanopyCore"),
        // The macOS app: Accessibility, event taps, ScreenCaptureKit, AppKit UI.
        .executableTarget(
            name: "Canopy",
            dependencies: ["CanopyCore"],
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("Carbon"),
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("ServiceManagement"),
            ]
        ),
        .testTarget(name: "CanopyCoreTests", dependencies: ["CanopyCore"]),
    ]
)
