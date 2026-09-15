// swift-tools-version: 5.9

import PackageDescription

let settings: [SwiftSetting] = [
	.enableExperimentalFeature("StrictConcurrency")
]

let package = Package(
	name: "SwiftTreeSitter",
	platforms: [
		.macOS(.v10_13),
		.macCatalyst(.v13),
		.iOS(.v12),
		.tvOS(.v12),
		.watchOS(.v5),
		.visionOS(.v1),
	],
	products: [
		.library(name: "SwiftTreeSitter", targets: ["SwiftTreeSitter"]),
		.library(name: "SwiftTreeSitterLayer", targets: ["SwiftTreeSitterLayer"]),
	],
	dependencies: [
		// Local, WASM-enabled tree-sitter (TREE_SITTER_FEATURE_WASM + vendored wasmtime).
		.package(path: "../tree-sitter")
	],
	targets: [
		.target(
			name: "SwiftTreeSitter",
			dependencies: [
				.product(name: "TreeSitter", package: "tree-sitter")
			],
			swiftSettings: settings
		),
		.target(
			name: "SwiftTreeSitterLayer",
			dependencies: ["SwiftTreeSitter"],
			swiftSettings: settings
		),
	]
)
