// swift-tools-version:5.9
// Copyright (c) 2026 nilbox

import PackageDescription

let package = Package(
    name: "nilbox-vmm",
    platforms: [.macOS(.v12)],
    targets: [
        .executableTarget(
            name: "nilbox-vmm",
            path: "Sources/nilbox-vmm",
            linkerSettings: [
                .linkedFramework("Virtualization")
            ]
        )
    ]
)
