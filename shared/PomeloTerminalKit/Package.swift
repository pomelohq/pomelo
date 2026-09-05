// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "PomeloTerminalKit",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "PomeloTerminalKit", targets: ["PomeloTerminalKit"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.2.0"),
    ],
    targets: [
        .target(name: "PomeloTerminalKit",
                dependencies: [.product(name: "SwiftTerm", package: "SwiftTerm")],
                path: "Sources/PomeloTerminalKit"),
    ]
)
