// swift-tools-version:6.0
// Build the Rust core first: `cargo build --release -p orbit-ffi` from the repo root, or use
// scripts/macos/build_app.sh, which does both.
import Foundation
import PackageDescription

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let rustLib = "\(packageDir)/../../target/release"

let package = Package(
    name: "Orbit",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Orbit", targets: ["Orbit"]),
        .executable(name: "OrbitBenchSink", targets: ["OrbitBenchSink"]),
    ],
    targets: [
        .systemLibrary(name: "COrbit", path: "Sources/COrbit"),
        .executableTarget(
            name: "Orbit",
            dependencies: ["COrbit"],
            path: "Sources/Orbit",
            swiftSettings: [.unsafeFlags(["-Osize"], .when(configuration: .release))],
            linkerSettings: [
                .unsafeFlags(["-L", rustLib, "-Xlinker", "-dead_strip"]),
                .linkedLibrary("orbit_ffi"),
                .linkedFramework("AppKit"),
                .linkedFramework("Carbon"),
                .linkedFramework("CoreServices"),
                .linkedFramework("IOKit"),
                .linkedFramework("ServiceManagement"),
            ]
        ),
        // A small window the benchmark driver can focus, to catch keys that would have leaked.
        .executableTarget(name: "OrbitBenchSink", path: "Sources/OrbitBenchSink"),
    ],
    swiftLanguageModes: [.v5]
)
