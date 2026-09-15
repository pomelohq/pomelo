// swift-tools-version: 5.8
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription
import Foundation

// Absolute path to the vendored wasmtime, resolved per build machine so a checkout
// links + finds libwasmtime without a portable-path dance. Release packaging embeds
// the dylib in the .app and rewrites the rpath.
let pkgRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let wasmtimeLib = "\(pkgRoot)/Vendor/wasmtime/lib"

let package = Package(
    name: "TreeSitter",
    products: [
        // Products define the executables and libraries a package produces, and make them visible to other packages.
        .library(
            name: "TreeSitter",
            targets: ["TreeSitter"]),
    ],
    targets: [
        .target(name: "TreeSitter",
                path: "lib",
                exclude: [
                        "src/unicode/ICU_SHA",
                        "src/unicode/README.md",
                        "src/unicode/LICENSE",
                        "src/wasm/stdlib-symbols.txt",
                        "src/stdlib-symbols.txt",
                        // Compiled to wasm separately (its symbols come from the prebuilt
                        // wasm-stdlib.h blob); it uses wasm-only builtins, not native.
                        "src/wasm/stdlib.c",
                        "src/lib.c",
                ],
                sources: ["src"],
                publicHeadersPath: "include",
                cSettings: [
                        .headerSearchPath("src"),
                        // Enable the runtime WASM grammar loader (wasm_store.c). Headers
                        // come from the vendored wasmtime C API; libwasmtime is linked and
                        // embedded by the app target.
                        .define("TREE_SITTER_FEATURE_WASM"),
                        .headerSearchPath("../Vendor/wasmtime/include"),
                        .define("_POSIX_C_SOURCE", to: "200112L"),
                        .define("_DEFAULT_SOURCE"),
                        .define("_DARWIN_C_SOURCE"),
                ],
                linkerSettings: [
                        .unsafeFlags([
                                "-L\(wasmtimeLib)", "-lwasmtime",
                                "-Xlinker", "-rpath", "-Xlinker", wasmtimeLib,
                        ]),
                ]),
    ],
    cLanguageStandard: .c11
)
