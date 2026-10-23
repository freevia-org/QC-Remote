import AppKit

// ServiceManagement owns this launcher, not the long-lived broker. Exit zero
// after LaunchServices opens the independent menu so disabling login startup
// does not kill a working device connection or the menu app.
let launcher = Bundle.main.bundleURL
let editor = launcher.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
let menu = editor.appendingPathComponent("Contents/Helpers/QC Device Broker.app")
guard FileManager.default.fileExists(atPath: menu.appendingPathComponent("Contents/MacOS/QCDeviceBroker").path) else {
    fputs("QC Device Broker is missing from the application bundle.\n", stderr)
    exit(1)
}
let config = NSWorkspace.OpenConfiguration()
config.activates = false
config.arguments = ["--keep-user-quit"]
NSWorkspace.shared.openApplication(at: menu, configuration: config) { app, error in
    if let error = error {
        fputs("Could not start QC Device Broker: \(error.localizedDescription)\n", stderr)
        exit(1)
    }
    exit(app == nil ? 1 : 0)
}
RunLoop.main.run()
