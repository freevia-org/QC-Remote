// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "QCIosNativeBridge",
    platforms: [.iOS(.v15)],
    products: [.library(name: "QcRemoteIosNativeBridge", targets: ["QCIosNativeBridgePlugin"])],
    dependencies: [.package(url: "https://github.com/ionic-team/capacitor-swift-pm.git", from: "8.0.0")],
    targets: [
        .target(
            name: "QCIosNativeBridgePlugin",
            dependencies: [.product(name: "Capacitor", package: "capacitor-swift-pm")],
            path: "ios/Sources/QCIosNativeBridgePlugin"
        )
    ]
)
