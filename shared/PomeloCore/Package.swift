// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "PomeloCore",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "PomeloCore", targets: ["PomeloCore"]),
    ],
    targets: [
        .target(name: "PomeloCore", path: "Sources/PomeloCore"),
    ]
)
