// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "CodeEditLanguages",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "CodeEditLanguages", targets: ["CodeEditLanguages"])
    ],
    dependencies: [
        // Local SwiftTreeSitter -> local WASM-enabled tree-sitter.
        .package(path: "../SwiftTreeSitter")
    ],
    targets: [
        .target(
            name: "TreeSitterGrammars",
            path: "Sources/TreeSitterGrammars",
            publicHeadersPath: "include",
            cSettings: [
                .headerSearchPath("vendored-headers")
            ]
        ),
        .target(
            name: "CodeEditLanguages",
            dependencies: [
                "TreeSitterGrammars",
                .product(name: "SwiftTreeSitter", package: "SwiftTreeSitter")
            ],
            resources: [.copy("Resources")],
            linkerSettings: []
        )
    ]
)
