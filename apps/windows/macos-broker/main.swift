import AppKit

// This menu app is independent of the editor. All device access and policy
// stay in the existing authenticated broker and its control CLI.
final class BrokerMenu: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private var item: NSStatusItem!
    private let status = NSMenuItem(title: "Starting broker…", action: nil, keyEquivalent: "")
    private let detail = NSMenuItem(title: "", action: nil, keyEquivalent: "")
    private var timer: Timer?
    private var animationTimer: Timer?
    private var iconTone = "inactive"
    private var iconFrame = 0
    private var icons: [String: NSImage] = [:]
    private var latestMenu: [String: Any] = [:]
    private var quitting = false
    private var busy = false
    private let queue = DispatchQueue(label: "org.freevia.qcremote.broker-menu.control")
    private let broker = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/qc-device-broker")
    private var directoryArguments: [String] {
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--directory"), i + 1 < args.count {
            return ["--directory", args[i + 1]]
        }
        return []
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        item = NSStatusBar.system.statusItem(withLength: 28)
        item.autosaveName = "QCDeviceBroker.StatusItem"
        item.behavior = []
        item.isVisible = true
        item.button?.title = ""
        loadIcons()
        updateIcon()
        item.button?.setAccessibilityLabel("QC Device Broker")
        item.button?.toolTip = "QC Device Broker"
        let menu = NSMenu()
        status.isEnabled = false
        detail.isEnabled = false
        menu.addItem(status)
        menu.addItem(detail)
        menu.addItem(.separator())
        menu.delegate = self
        menu.autoenablesItems = false
        item.menu = menu
        // LaunchAgent/background starts must preserve a recorded explicit Quit.
        if CommandLine.arguments.contains("--keep-user-quit") {
            queue.async {
                let process = Process()
                process.executableURL = self.broker
                process.arguments = ["--resident", "--keep-user-quit"] + self.directoryArguments
                process.standardOutput = FileHandle.nullDevice
                process.standardError = FileHandle.nullDevice
                try? process.run()
                DispatchQueue.main.async { self.refresh() }
            }
        } else {
            run("start")
        }
        timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in self?.refresh() }
        animationTimer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in
            guard let self = self, self.iconTone == "syncing" else { return }
            self.iconFrame = (self.iconFrame + 1) % 4
            self.updateIcon()
        }
        RunLoop.main.add(animationTimer!, forMode: .common)
        if CommandLine.arguments.contains("--show-menu") {
            DispatchQueue.main.async { self.showMenu() }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showMenu()
        return false
    }

    private func showMenu() {
        item.isVisible = true
        NSApp.activate(ignoringOtherApps: true)
        item.button?.performClick(nil)
    }

    private func control(_ command: String) -> (Int32, [String: Any]) {
        execute(command == "menu" ? ["--tray-menu"] + directoryArguments :
            ["--control", command] + directoryArguments + (command == "quit" ? ["--confirm"] : []))
    }

    private func execute(_ arguments: [String]) -> (Int32, [String: Any]) {
        let process = Process()
        let output = Pipe()
        process.executableURL = broker
        process.arguments = arguments
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return (process.terminationStatus, (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:])
        } catch { return (1, ["detail": error.localizedDescription]) }
    }

    private func run(_ command: String) {
        guard !busy else { return }
        busy = true
        queue.async {
            let (code, result) = self.control(command)
            DispatchQueue.main.async {
                self.busy = false
                if command == "quit" && (code == 0 || code == 3) {
                    NSApp.terminate(nil)
                    return
                } else if code != 0 {
                    self.detail.title = result["detail"] as? String ?? "Broker request failed"
                    let alert = NSAlert()
                    alert.messageText = "QC Device Broker"
                    alert.informativeText = self.detail.title
                    alert.runModal()
                }
                self.refresh()
            }
        }
    }

    private func refresh() {
        guard !busy else { return }
        busy = true
        queue.async {
            let (code, result) = self.control("menu")
            DispatchQueue.main.async {
                self.busy = false
                if self.quitting && code != 0 { NSApp.terminate(nil); return }
                self.latestMenu = code == 0 ? (result["result"] as? [String: Any] ?? [:]) : [:]
                let presentation = self.latestMenu["presentation"] as? [String: Any] ?? [:]
                self.status.title = presentation["tooltip"] as? String ?? "Broker menu unavailable"
                self.detail.title = result["detail"] as? String ?? "Start the broker to connect"
                self.iconTone = self.latestMenu["tone"] as? String ?? "error"
                self.updateIcon()
                self.item.button?.toolTip = self.status.title
                self.writeMenuDiagnostics()
            }
        }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        guard menu === item.menu else { return }
        menu.removeAllItems()
        if let presentation = latestMenu["presentation"] as? [String: Any],
           let rows = presentation["rows"] as? [Any] {
            append(rows, to: menu)
        } else {
            let message = NSMenuItem(title: detail.title, action: nil, keyEquivalent: "")
            message.isEnabled = false
            menu.addItem(message)
            for (title, selector) in [("Start Broker", #selector(startBroker)),
                                      ("Open QC Remote", #selector(openEditor)),
                                      ("Quit", #selector(quitBroker))] {
                let row = NSMenuItem(title: title, action: selector, keyEquivalent: "")
                row.target = self
                menu.addItem(row)
            }
        }
    }

    private func actionRow(_ label: String, _ action: Any, enabled: Bool = true) -> NSMenuItem {
        let row = NSMenuItem(title: label, action: #selector(menuAction(_:)), keyEquivalent: "")
        row.target = self
        row.isEnabled = enabled
        row.representedObject = action
        return row
    }

    private func append(_ rows: [Any], to menu: NSMenu) {
        menu.autoenablesItems = false
        for value in rows {
            if let name = value as? String, name == "Separator" {
                menu.addItem(.separator())
                continue
            }
            guard let row = value as? [String: Any] else { continue }
            if let text = row["Text"] as? String {
                let item = NSMenuItem(title: text, action: nil, keyEquivalent: "")
                item.isEnabled = false
                menu.addItem(item)
            } else if let action = row["Action"] as? [String: Any], let value = action["action"] {
                menu.addItem(actionRow(action["label"] as? String ?? "", value,
                    enabled: action["enabled"] as? Bool == true))
            } else if let choices = row["Choices"] as? [String: Any] {
                let parent = NSMenuItem(title: choices["label"] as? String ?? "", action: nil, keyEquivalent: "")
                let submenu = NSMenu()
                submenu.autoenablesItems = false
                for choice in choices["choices"] as? [[Any]] ?? [] where choice.count == 3 {
                    let item = actionRow(choice[0] as? String ?? "", choice[2])
                    item.state = choice[1] as? Bool == true ? .on : .off
                    submenu.addItem(item)
                }
                parent.submenu = submenu
                menu.addItem(parent)
            } else if let nested = row["Submenu"] as? [String: Any] {
                let parent = NSMenuItem(title: nested["label"] as? String ?? "", action: nil, keyEquivalent: "")
                let submenu = NSMenu()
                append(nested["rows"] as? [Any] ?? [], to: submenu)
                parent.submenu = submenu
                menu.addItem(parent)
            }
        }
    }

    @objc private func menuAction(_ sender: NSMenuItem) {
        guard let action = sender.representedObject, !busy else { return }
        if action as? String == "OpenApp" { openEditor(); return }
        var confirmed = false
        let encode: (Any) -> Data? = { try? JSONSerialization.data(withJSONObject: ["action": $0], options: .sortedKeys) }
        for entry in latestMenu["confirmations"] as? [[String: Any]] ?? [] {
            guard let candidate = entry["action"], encode(candidate) == encode(action),
                  let question = entry["question"] as? [String: Any] else { continue }
            let alert = NSAlert()
            alert.messageText = question["title"] as? String ?? "Confirm"
            alert.informativeText = question["text"] as? String ?? ""
            alert.addButton(withTitle: "Unlink")
            alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return }
            confirmed = true
        }
        guard let data = try? JSONSerialization.data(withJSONObject: ["action": action, "confirmed": confirmed]),
              let payload = String(data: data, encoding: .utf8) else { return }
        busy = true
        queue.async {
            let (code, result) = self.execute(["--tray-action", payload] + self.directoryArguments)
            DispatchQueue.main.async {
                self.busy = false
                if code != 0 {
                    let alert = NSAlert()
                    alert.messageText = "QC Device Broker"
                    alert.informativeText = result["detail"] as? String ?? "Action failed"
                    alert.runModal()
                } else if action as? String == "ReleaseAndExit" {
                    self.quitting = true
                }
                self.refresh()
            }
        }
    }

    private func loadIcons() {
        let names = ["ready", "attention", "error", "inactive"] + (0..<4).map { "syncing-\($0)" }
        for name in names {
            guard let url = Bundle.main.resourceURL?.appendingPathComponent("BrokerLED/\(name).rgba"),
                  let data = try? Data(contentsOf: url), data.count == 32 * 32 * 4,
                  let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 32, pixelsHigh: 32,
                    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                    colorSpaceName: .deviceRGB, bitmapFormat: .alphaNonpremultiplied,
                    bytesPerRow: 32 * 4, bitsPerPixel: 32), let bytes = bitmap.bitmapData else { continue }
            data.copyBytes(to: bytes, count: data.count)
            let image = NSImage(size: NSSize(width: 20, height: 20))
            image.addRepresentation(bitmap)
            image.isTemplate = false
            icons[name] = image
        }
    }

    private func updateIcon() {
        let name = iconTone == "syncing" ? "syncing-\(iconFrame)" : iconTone
        item.button?.image = icons[name]
        // Keep the broker reachable if a damaged install loses its artwork.
        item.button?.title = icons[name] == nil ? "QC" : ""
    }

    private func writeMenuDiagnostics() {
        let directory = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("org.freevia.qcremote")
        let diagnostics: [String: Any] = [
            "processId": ProcessInfo.processInfo.processIdentifier,
            "visible": item.isVisible,
            "hasButton": item.button != nil,
            "iconTone": iconTone,
            "iconLoaded": item.button?.image != nil,
            "windowVisible": item.button?.window?.isVisible ?? false,
            "frame": item.button?.window.map { NSStringFromRect($0.frame) } ?? "none",
            "screen": item.button?.window?.screen.map { NSStringFromRect($0.frame) } ?? "none"
        ]
        if let data = try? JSONSerialization.data(withJSONObject: diagnostics) {
            try? data.write(to: directory.appendingPathComponent("broker-menu-status.json"), options: .atomic)
        }
    }

    @objc private func startBroker() { run("start") }
    @objc private func connect() { run("connect") }
    @objc private func disconnect() { run("release") }
    @objc private func openEditor() {
        // Installed sibling first; nested helper works from the editor bundle too.
        var editor = Bundle.main.bundleURL.deletingLastPathComponent().appendingPathComponent("QC Remote.app")
        if !FileManager.default.fileExists(atPath: editor.path) {
            editor = Bundle.main.bundleURL.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        }
        NSWorkspace.shared.openApplication(at: editor, configuration: NSWorkspace.OpenConfiguration(), completionHandler: nil)
    }
    @objc private func quitBroker() {
        let alert = NSAlert()
        alert.messageText = "Quit QC Device Broker?"
        alert.informativeText = "This disconnects the Quad Cortex and all broker clients. QC Remote can stay open."
        alert.addButton(withTitle: "Quit Broker")
        alert.addButton(withTitle: "Cancel")
        if alert.runModal() == .alertFirstButtonReturn { run("quit") }
    }
}

let application = NSApplication.shared
// LaunchServices enforces one instance when opened as an app.
if NSRunningApplication.runningApplications(withBundleIdentifier: "org.freevia.qcremote.broker-menu")
    .contains(where: { $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }) {
    exit(0)
}
application.setActivationPolicy(.accessory)
let delegate = BrokerMenu()
application.delegate = delegate
application.run()
