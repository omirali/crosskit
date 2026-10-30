// swift-tools-version: 6.2
import PackageDescription

// Общее у приложений Crosskit (ADR 0058): вопрос службе по сокету, её ответы как типы и то,
// что из ответа рисуется. Пакет не знает ни SwiftUI, ни AppKit — их знают приложения.
// macOS 15, а не 26: панель держится macOS 15 и сможет взять отсюда то же самое.
let package = Package(
    name: "CrosskitKit",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "CrosskitKit", targets: ["CrosskitKit"]),
    ],
    targets: [
        .target(name: "CrosskitKit"),
        .testTarget(name: "CrosskitKitTests", dependencies: ["CrosskitKit"]),
    ]
)
