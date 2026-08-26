// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "PomeloApp",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.2.0"),
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0"),
        .package(url: "https://github.com/li3zhen1/Grape", from: "1.1.0"),
        // SQL editor: CodeEditSourceEditor vendored (LocalPackages/) at a newer build —
        // its completion window works when embedded (the 0.15.2 tag's did not). Needs an
        // Xcode build (asset catalog + Metal).
        .package(path: "LocalPackages/CodeEditSourceEditor"),
        .package(path: "LocalPackages/CodeEditLanguages"),
        .package(path: "LocalPackages/CodeEditTextView"),
    ],
    targets: [
        .target(name: "CPom"),
        .executableTarget(
            name: "PomeloApp",
            dependencies: ["CPom", .product(name: "SwiftTerm", package: "SwiftTerm"), .product(name: "Sparkle", package: "Sparkle"),
                           .product(name: "Grape", package: "Grape"),
                           .product(name: "CodeEditSourceEditor", package: "CodeEditSourceEditor"),
                           .product(name: "CodeEditLanguages", package: "CodeEditLanguages"),
                           .product(name: "CodeEditTextView", package: "CodeEditTextView")],
            linkerSettings: [
                .unsafeFlags([
                    "-LVendor", "-lpom",
                    "-framework", "CoreFoundation",
                    "-framework", "Security",
                    "-lresolv",
                ])
            ]
        ),
        .testTarget(
            name: "PomeloAppTests",
            dependencies: ["PomeloApp"],
            linkerSettings: [
                .unsafeFlags([
                    "-LVendor", "-lpom",
                    "-framework", "CoreFoundation",
                    "-framework", "Security",
                    "-lresolv",
                ])
            ]
        ),
    ]
)
