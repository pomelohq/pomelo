// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "PomeloRemote",
    platforms: [.iOS(.v17)],
    products: [
        .library(name: "PomeloRemoteKit", targets: ["PomeloRemoteKit"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", from: "1.2.0"),
        .package(path: "../../shared/PomeloTerminalKit"),
        .package(path: "../../shared/PomeloUI"),
        .package(path: "../../shared/PomeloCore"),
    ],
    targets: [
        .target(name: "PomeloRemoteKit",
                dependencies: ["SwiftTerm",
                               .product(name: "PomeloTerminalKit", package: "PomeloTerminalKit"),
                               .product(name: "PomeloUI", package: "PomeloUI"),
                               .product(name: "PomeloCore", package: "PomeloCore")],
                path: "Sources/PomeloRemoteKit"),
        .testTarget(name: "PomeloRemoteKitTests", dependencies: ["PomeloRemoteKit"], path: "Tests/PomeloRemoteKitTests"),
    ]
)
