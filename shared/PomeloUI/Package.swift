// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "PomeloUI",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "PomeloUI", targets: ["PomeloUI"]),
    ],
    targets: [
        .target(name: "PomeloUI", path: "Sources/PomeloUI"),
    ]
)
