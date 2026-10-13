import Foundation
import Capacitor
import Security
import UIKit
import CoreMIDI
import Darwin

private let qcMidiTransportTypeProperty = "transportType" as CFString
private let qcMidiTransportTypeUSB = "USB"
private let qcParamErr: OSStatus = -50
private let qcMemFullErr: OSStatus = -108

@_silgen_name("qc_ios_report_size")
private func qcIosReportSize() -> Int

@objc(QCIosNativeBridgePlugin)
public class QCIosNativeBridgePlugin: CAPPlugin, CAPBridgedPlugin {
    public let identifier = "QCIosNativeBridgePlugin"
    public let jsName = "QCIosNativeBridge"
    public let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "secureGet", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "secureSet", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "secureRemove", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setScreenAwake", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getMidiInputChannel", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setMidiInputChannel", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "platformCapabilities", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "scanUsbMidi", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "connectUsbMidi", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "disconnectUsbMidi", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "sendUsbMidiControlChange", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "sendUsbMidiProgramChange", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "scanUsbDriver", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "connectUsbDriver", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "disconnectUsbDriver", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "usbDriverDiagnostics", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "gatewayReadUsb", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "gatewayWriteUsb", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "driverCurrentPreset", returnType: CAPPluginReturnPromise)
    ]

    private var midiClient: MIDIClientRef?
    private var midiOutput: MIDIPortRef?
    private var midiDestination: MIDIEndpointRef?
    private var midiDeviceID: Int32?
    private let midiQueue = DispatchQueue(label: "org.freevia.qcremote.ios.usb-midi")
    private let driverClient = QCIosDriverClient()
    private let driverQueue = DispatchQueue(label: "org.freevia.qcremote.ios.driverkit")
    private let driverReadQueue = DispatchQueue(label: "org.freevia.qcremote.ios.driverkit-read", qos: .userInitiated)
    private var protocolSession: QCIosProtocolSession?
    private var driverConnection: UInt64 = 0
    private var driverGeneration: UInt64 = 0
    private var driverReaderGeneration: UInt64 = 0
    private var driverStateSequence: UInt64 = 0
    private var driverSessionID = ""
    private var driverSynchronized = false
    private var driverWritesQuarantined = false
    private var presetWorkflowMutationAttempts = Set<UInt64>()
    private final class PendingDirectBackup {
        let call: CAPPluginCall
        let session: QCIosProtocolSession
        let name: String
        var generation: UInt64
        var requestDispatched = false
        var savedResult: [String: Any]?
        var waitingForRecoverySync = false
        init(call: CAPPluginCall, session: QCIosProtocolSession, name: String, generation: UInt64) {
            self.call = call; self.session = session; self.name = name; self.generation = generation
        }
    }
    private var pendingDirectBackup: PendingDirectBackup?
    private struct PendingGatewayRead {
        let call: CAPPluginCall
        let method: String
        let params: [String: Any]
        let plan: [String: Any]
        let deadlineMs: UInt64
        let firstReadValue: [String: Any]?
    }
    private var pendingGatewayReads: [String: PendingGatewayRead] = [:]
    private final class PendingGatewayWrite {
        let call: CAPPluginCall
        let method: String
        let plan: [String: Any]
        let verification: Any
        let verificationID: UInt64
        var relevantSequence: UInt64
        var wakeScheduled = false
        init(call: CAPPluginCall, method: String, plan: [String: Any], verification: Any, verificationID: UInt64, relevantSequence: UInt64) {
            self.call = call; self.method = method; self.plan = plan; self.verification = verification
            self.verificationID = verificationID; self.relevantSequence = relevantSequence
        }
    }
    private var pendingGatewayWrites: [UInt64: PendingGatewayWrite] = [:]

    private var service: String {
        Bundle.main.bundleIdentifier ?? "org.freevia.qcremote"
    }

    private var midiInputChannel: Int {
        let stored = UserDefaults.standard.integer(forKey: "qc.remote.ios.midi-input-channel")
        return (1...16).contains(stored) ? stored : 1
    }

    @objc public func getMidiInputChannel(_ call: CAPPluginCall) {
        call.resolve(["channel": midiInputChannel])
    }

    @objc public func setMidiInputChannel(_ call: CAPPluginCall) {
        guard let channel = call.getInt("channel"), (1...16).contains(channel) else {
            call.reject("MIDI channel must be between 1 and 16.", "INVALID_MIDI_CHANNEL")
            return
        }
        UserDefaults.standard.set(channel, forKey: "qc.remote.ios.midi-input-channel")
        call.resolve(["channel": channel])
    }

    @objc public func secureGet(_ call: CAPPluginCall) {
        guard let key = call.getString("key"), !key.isEmpty else {
            call.reject("A Keychain key is required.", "INVALID_KEY")
            return
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            call.resolve(["value": NSNull()])
            return
        }
        guard status == errSecSuccess, let data = result as? Data,
              let value = String(data: data, encoding: .utf8) else {
            call.reject("The Keychain item could not be read.", "KEYCHAIN_READ_FAILED")
            return
        }
        call.resolve(["value": value])
    }

    @objc public func secureSet(_ call: CAPPluginCall) {
        guard let key = call.getString("key"), !key.isEmpty,
              let value = call.getString("value"), let data = value.data(using: .utf8) else {
            call.reject("A Keychain key and value are required.", "INVALID_ITEM")
            return
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key
        ]
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        let updateStatus = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if updateStatus == errSecItemNotFound {
            var insert = query
            attributes.forEach { insert[$0.key] = $0.value }
            let status = SecItemAdd(insert as CFDictionary, nil)
            guard status == errSecSuccess else {
                call.reject("The Keychain item could not be saved.", "KEYCHAIN_WRITE_FAILED")
                return
            }
        } else if updateStatus != errSecSuccess {
            call.reject("The Keychain item could not be updated.", "KEYCHAIN_WRITE_FAILED")
            return
        }
        call.resolve()
    }

    @objc public func secureRemove(_ call: CAPPluginCall) {
        guard let key = call.getString("key"), !key.isEmpty else {
            call.reject("A Keychain key is required.", "INVALID_KEY")
            return
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            call.reject("The Keychain item could not be removed.", "KEYCHAIN_REMOVE_FAILED")
            return
        }
        call.resolve()
    }

    @objc public func setScreenAwake(_ call: CAPPluginCall) {
        let enabled = call.getBool("enabled") ?? false
        DispatchQueue.main.async {
            UIApplication.shared.isIdleTimerDisabled = enabled
            call.resolve(["enabled": enabled])
        }
    }

    @objc public func platformCapabilities(_ call: CAPPluginCall) {
        let isPad = UIDevice.current.userInterfaceIdiom == .pad
        let driverHardwareSupported = isPad && isMSeriesIpad()
        let driverOsSupported = isPad && ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 16
        let driverBundled = hasBundledDriverExtension()
        let driverReady = driverHardwareSupported && driverOsSupported && driverBundled && QCIosDriverClient.canOpenService()
        call.resolve([
            "deviceFamily": isPad ? "ipad" : "iphone",
            "supportsUsbMidi": true,
            "sharedProtocolRuntimeBundled": qcIosReportSize() == 129,
            "driverHardwareSupported": driverHardwareSupported,
            "driverOsSupported": driverOsSupported,
            "fullUsbDriverBundled": driverBundled,
            "fullUsbReady": driverReady,
            "driverEntitlementRequired": true,
            "fullUsbRequirement": "An M-series iPad running iPadOS 16 or later, with the QC Remote USB driver bundled, Apple DriverKit entitlements granted, and the driver enabled in Settings",
            "driverCompatibilityNote": "Apple documents that DriverKit does not support USB devices that manipulate audio. Quad Cortex combines control HID with USB audio. This direct-control prototype is unapproved and unvalidated for release, and may interfere with QC audio or MIDI."
        ])
    }

    private func isMSeriesIpad() -> Bool {
        var size = 0
        guard sysctlbyname("hw.machine", nil, &size, nil, 0) == 0, size > 0 else { return false }
        var machine = [CChar](repeating: 0, count: size)
        guard sysctlbyname("hw.machine", &machine, &size, nil, 0) == 0 else { return false }
        let identifier = String(cString: machine)
        let supported = Set([
            "iPad13,4", "iPad13,5", "iPad13,6", "iPad13,7", "iPad13,8", "iPad13,9", "iPad13,10", "iPad13,11", "iPad13,16", "iPad13,17",
            "iPad14,3", "iPad14,4", "iPad14,5", "iPad14,6", "iPad14,8", "iPad14,9", "iPad14,10", "iPad14,11",
            "iPad15,3", "iPad15,4", "iPad15,5", "iPad15,6",
            "iPad16,3", "iPad16,4", "iPad16,5", "iPad16,6", "iPad16,8", "iPad16,9", "iPad16,10", "iPad16,11",
            "iPad17,1", "iPad17,2", "iPad17,3", "iPad17,4", "iPad17,5", "iPad17,6"
        ])
        return supported.contains(identifier)
    }

    private func hasBundledDriverExtension() -> Bool {
        guard let enumerator = FileManager.default.enumerator(at: Bundle.main.bundleURL, includingPropertiesForKeys: nil) else {
            return false
        }
        for case let url as URL in enumerator where url.lastPathComponent == "QCRemoteUSBDriver.dext" {
            return true
        }
        return false
    }

    @objc public func scanUsbMidi(_ call: CAPPluginCall) {
        midiQueue.async {
            let devices = self.usbMidiDevices()
            call.resolve(["devices": devices.map { $0.json }])
        }
    }

    @objc public func connectUsbMidi(_ call: CAPPluginCall) {
        guard let id = call.getInt("deviceId") else {
            call.reject("Choose a Quad Cortex USB-MIDI device.", "INVALID_DEVICE")
            return
        }
        midiQueue.async {
            guard let device = self.usbMidiDevices().first(where: { Int($0.id) == id }),
                  let destination = device.destination else {
                call.reject("The selected Quad Cortex USB-MIDI device is no longer connected.", "MIDI_DEVICE_MISSING")
                return
            }
            self.closeMidiSession()
            var client: MIDIClientRef = 0
            let clientStatus = MIDIClientCreate("QC Remote" as CFString, nil, nil, &client)
            guard clientStatus == noErr else {
                call.reject("Core MIDI could not create a device session (\(clientStatus)).", "MIDI_CLIENT_FAILED")
                return
            }
            var output: MIDIPortRef = 0
            let portStatus = MIDIOutputPortCreate(client, "QC Remote Output" as CFString, &output)
            guard portStatus == noErr else {
                MIDIClientDispose(client)
                call.reject("Core MIDI could not create an output port (\(portStatus)).", "MIDI_PORT_FAILED")
                return
            }
            self.midiClient = client
            self.midiOutput = output
            self.midiDestination = destination
            self.midiDeviceID = device.id
            call.resolve(["connected": true, "deviceId": Int(device.id), "name": device.name])
        }
    }

    @objc public func disconnectUsbMidi(_ call: CAPPluginCall) {
        midiQueue.async {
            self.closeMidiSession()
            call.resolve()
        }
    }

    @objc public func sendUsbMidiControlChange(_ call: CAPPluginCall) {
        guard let channel = call.getInt("channel"), (1...16).contains(channel),
              let controller = call.getInt("controller"), (0...127).contains(controller),
              let value = call.getInt("value"), (0...127).contains(value) else {
            call.reject("MIDI channel must be between 1 and 16; controller and value must each be between 0 and 127.", "INVALID_MIDI_MESSAGE")
            return
        }
        midiQueue.async {
            self.sendMidiBytes([0xB0 | UInt8(channel - 1), UInt8(controller), UInt8(value)], call: call)
        }
    }

    @objc public func sendUsbMidiProgramChange(_ call: CAPPluginCall) {
        guard let channel = call.getInt("channel"), (1...16).contains(channel),
              let bankMsb = call.getInt("bankMsb"), (0...1).contains(bankMsb),
              let bankLsb = call.getInt("bankLsb"), (0...12).contains(bankLsb),
              let program = call.getInt("program"), (0...127).contains(program) else {
            call.reject("Preset MIDI requires channel 1-16, bank MSB 0-1, setlist 0-12, and program 0-127.", "INVALID_MIDI_MESSAGE")
            return
        }
        midiQueue.async {
            let controlChangeStatus = UInt8(0xB0 | (channel - 1))
            let programChangeStatus = UInt8(0xC0 | (channel - 1))
            guard self.sendMidiBytes([controlChangeStatus, 0, UInt8(bankMsb)], call: call, resolve: false) else { return }
            Thread.sleep(forTimeInterval: 0.008)
            guard self.sendMidiBytes([controlChangeStatus, 32, UInt8(bankLsb)], call: call, resolve: false) else { return }
            Thread.sleep(forTimeInterval: 0.008)
            _ = self.sendMidiBytes([programChangeStatus, UInt8(program)], call: call)
        }
    }

    @objc public func scanUsbDriver(_ call: CAPPluginCall) {
        guard UIDevice.current.userInterfaceIdiom == .pad, isMSeriesIpad(),
              ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 16 else {
            call.resolve(["available": false, "reason": "USBDriverKit requires an M-series iPad running iPadOS 16 or later."])
            return
        }
        driverQueue.async {
            call.resolve([
                "available": QCIosDriverClient.isServiceAvailable(),
                "connected": self.driverClient.isOpen,
                "synchronized": self.driverSynchronized
            ])
        }
    }

    @objc public func connectUsbDriver(_ call: CAPPluginCall) {
        guard UIDevice.current.userInterfaceIdiom == .pad, isMSeriesIpad(),
              ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 16 else {
            call.reject("USBDriverKit requires an M-series iPad running iPadOS 16 or later.", "IPAD_REQUIRED")
            return
        }
        driverQueue.async {
            do {
                if self.driverClient.isOpen, self.protocolSession != nil {
                    let diagnostics = (try? self.driverClient.diagnostics()) ?? [:]
                    call.resolve(["connected": true, "synchronized": self.driverSynchronized,
                                  "deviceId": 1, "name": "Quad Cortex USB", "diagnostics": diagnostics])
                    return
                }
                try self.driverClient.open()
                let session = try QCIosProtocolSession()
                let now = self.monotonicMilliseconds()
                let connection = try session.opened(nowMs: now)
                self.driverGeneration &+= 1
                self.driverStateSequence = 0
                self.driverConnection = connection
                self.driverSessionID = UUID().uuidString.lowercased()
                self.protocolSession = session
                self.driverSynchronized = false
                let diagnostics = try self.driverClient.diagnostics()
                self.scheduleDriverProtocolPump(generation: self.driverGeneration, session: session)
                call.resolve(["connected": true, "synchronized": false, "deviceId": 1, "name": "Quad Cortex USB", "diagnostics": diagnostics])
            } catch {
                self.protocolSession = nil
                self.driverClient.close()
                call.reject("Could not open the QC DriverKit connection: \(error)", "DRIVER_OPEN_FAILED")
            }
        }
    }

    @objc public func disconnectUsbDriver(_ call: CAPPluginCall) {
        driverQueue.async {
            if let backup = self.pendingDirectBackup {
                self.failDirectBackup(backup, message: "The QC disconnected during backup export.", code: "DRIVER_DISCONNECTED")
            }
            self.driverGeneration &+= 1
            self.protocolSession = nil
            self.driverConnection = 0
            self.driverSessionID = ""
            self.driverSynchronized = false
            self.rejectPendingGatewayReads("The Quad Cortex connection ended before the read completed.", code: "DRIVER_DISCONNECTED")
            self.rejectPendingGatewayWrites("The Quad Cortex connection ended before write verification completed.")
            self.driverClient.close()
            self.notifyListeners("qcConnection", data: ["state": "disconnected"], retainUntilConsumed: false)
            call.resolve()
        }
    }

    @objc public func usbDriverDiagnostics(_ call: CAPPluginCall) {
        driverQueue.async {
            do {
                var diagnostics = try self.driverClient.diagnostics()
                diagnostics["writesQuarantined"] = self.driverWritesQuarantined ? 1 : 0
                call.resolve(diagnostics)
            }
            catch { call.reject("QC DriverKit diagnostics are unavailable: \(error)", "DRIVER_DIAGNOSTICS_FAILED") }
        }
    }

    @objc public func gatewayReadUsb(_ call: CAPPluginCall) {
        guard let method = call.getString("method"), !method.isEmpty else {
            call.reject("A gateway read method is required.", "INVALID_METHOD")
            return
        }
        let params = call.getObject("params") ?? [:]
        driverQueue.async {
            guard let session = self.protocolSession, self.driverClient.isOpen else {
                call.reject("Connect the Quad Cortex through the iPad USB driver first.", "DRIVER_NOT_CONNECTED")
                return
            }
            guard self.pendingDirectBackup == nil else {
                call.reject("A direct QC backup is in progress. Wait for it to finish before reading device state.", "BACKUP_IN_PROGRESS")
                return
            }
            guard self.driverSynchronized else {
                call.reject("Wait for the Quad Cortex to finish synchronizing before sending direct USB commands.", "DEVICE_NOT_SYNCHRONIZED")
                return
            }
            do {
                let startedAtMs = self.monotonicMilliseconds()
                let plan = try session.planGatewayRead(method: method, params: params, startedAtMs: startedAtMs)
                let timeoutMs = (plan["timeoutMs"] as? NSNumber)?.uint64Value ?? 5_000
                let readID = UUID().uuidString
                self.pendingGatewayReads[readID] = PendingGatewayRead(
                    call: call,
                    method: method,
                    params: params,
                    plan: plan,
                    deadlineMs: startedAtMs &+ timeoutMs,
                    firstReadValue: nil
                )
                self.sendGatewayReadPlan(plan, session: session, readID: readID)
                self.driverQueue.asyncAfter(deadline: .now() + .milliseconds(Int(min(timeoutMs, UInt64(Int.max))))) {
                    guard let pending = self.pendingGatewayReads.removeValue(forKey: readID) else { return }
                    pending.call.reject("The Quad Cortex did not return a valid \(method) response before the read timed out.", "READ_TIMEOUT")
                }
            } catch {
                call.reject("Could not plan the Quad Cortex read: \(error)", "READ_PLAN_FAILED")
            }
        }
    }

    @objc public func gatewayWriteUsb(_ call: CAPPluginCall) {
        guard let method = call.getString("method"), !method.isEmpty else {
            call.reject("A gateway write method is required.", "INVALID_METHOD")
            return
        }
        let params = call.getObject("params") ?? [:]
        driverQueue.async {
            guard let session = self.protocolSession, self.driverClient.isOpen else {
                call.reject("Connect the Quad Cortex through the iPad USB driver first.", "DRIVER_NOT_CONNECTED")
                return
            }
            guard self.pendingDirectBackup == nil else {
                call.reject("A direct QC backup is in progress. Wait for it to finish before sending another command.", "BACKUP_IN_PROGRESS")
                return
            }
            guard self.driverSynchronized else {
                call.reject("Wait for the Quad Cortex to finish synchronizing before sending direct USB commands.", "DEVICE_NOT_SYNCHRONIZED")
                return
            }
            var persistentMutation = false
            var reportMayHaveReachedDevice = false
            do {
                let now = self.monotonicMilliseconds()
                guard !self.driverWritesQuarantined else {
                    call.reject("A previous direct USB mutation has an unknown outcome. Disconnect and reconnect the Quad Cortex, then wait for synchronization before writing again.", "WRITE_QUARANTINED")
                    return
                }
                if method == "device.createBackup" {
                    guard let name = params["name"] as? String else {
                        call.reject("A backup name is required.", "INVALID_ARGUMENT")
                        return
                    }
                    let backup = PendingDirectBackup(call: call, session: session, name: name,
                                                     generation: self.driverGeneration)
                    let action = try session.beginBackup(name: name, nowMs: now)
                    guard action["kind"] as? String == "write" else {
                        throw QCIosProtocolSession.BridgeError.invalidResponse
                    }
                    self.pendingDirectBackup = backup
                    do {
                        try self.deliverPresetWorkflowAction(action, session: session) {
                            backup.requestDispatched = true
                        }
                        backup.requestDispatched = true
                        self.scheduleDirectBackupAdvance(backup, afterMs: 100)
                    } catch {
                        if backup.requestDispatched { self.driverWritesQuarantined = true }
                        self.pendingDirectBackup = nil
                        session.cancelBackup()
                        throw error
                    }
                    return
                }
                if Self.presetWorkflowMethods.contains(method) {
                    let preparation = try session.beginPresetWorkflow(method: method, params: params, nowMs: now)
                    self.advancePresetWorkflow(call: call, session: session, preparationId: preparation,
                                               generation: self.driverGeneration)
                    return
                }
                let plan = try session.planGatewayWrite(method: method, params: params, startedAtMs: now)
                if plan["noOp"] as? Bool == true {
                    call.resolve(["detail": plan["detail"] ?? "No device change was needed.", "accepted": true, "verified": true, "changed": false, "requestSent": false, "verification": "authoritative_preflight"])
                    return
                }
                if plan["lane"] as? String == "midi" {
                    guard let controller = (plan["controller"] as? NSNumber)?.intValue,
                          let value = (plan["value"] as? NSNumber)?.intValue,
                          (0...127).contains(controller), (0...127).contains(value) else {
                        throw QCIosProtocolSession.BridgeError.invalidResponse
                    }
                    try self.sendDirectMidiControlChange(controller: controller, value: value, channel: self.midiInputChannel)
                    call.resolve(["detail": plan["detail"] ?? "MIDI control change sent; the Quad Cortex state cannot be confirmed over this connection.",
                                  "accepted": true, "verified": false, "verification": "accepted_unverified"])
                    return
                }
                guard let dispatch = plan["dispatchTicket"] as? [String: Any],
                      let operation = plan["operationTicket"] as? [String: Any],
                      let messages = plan["messages"] as? [[String: Any]], !messages.isEmpty else {
                    throw QCIosProtocolSession.BridgeError.invalidResponse
                }
                let afterSequence = self.driverStateSequence
                var reportCount = 0
                persistentMutation = plan["realtime"] as? Bool != true
                for message in messages {
                    guard let type = (message["messageType"] as? NSNumber)?.uint16Value,
                          let payloadValues = message["payload"] as? [NSNumber] else {
                        throw QCIosProtocolSession.BridgeError.invalidResponse
                    }
                    let bytes = payloadValues.map(\.intValue)
                    guard bytes.allSatisfy({ (0...255).contains($0) }) else { throw QCIosProtocolSession.BridgeError.invalidResponse }
                    let reports = try session.encodeMessage(messageType: type, payload: Data(bytes.map(UInt8.init)))
                    let delivery = try session.prepareDelivery(dispatchTicket: dispatch, messageType: type, reportCount: reports.count, nowMs: self.monotonicMilliseconds())
                    _ = try session.deliveryAdmitted(ticket: delivery)
                    for (index, report) in reports.enumerated() {
                        _ = try session.beginReport(ticket: delivery, index: index, nowMs: self.monotonicMilliseconds())
                        do {
                            // A thrown USB call can still mean the host controller
                            // accepted some or all of the report. Treat this point
                            // as an unknown mutation outcome until state is resynced.
                            reportMayHaveReachedDevice = true
                            let result = try self.driverClient.writeReport(report)
                            let transferred = result["bytesTransferred"] ?? 0
                            let usbStatus = result["usbStatus"] ?? -1
                            let outcome: UInt8 = transferred == 129
                                ? (usbStatus == Int(-536854449) ? 1 : 0)
                                : 2
                            guard outcome != 2 else { throw QCIosDriverClient.ClientError.malformedResponse }
                            _ = try session.finishReport(ticket: delivery, index: index, outcome: outcome, nowMs: self.monotonicMilliseconds())
                            reportCount += 1
                        } catch {
                            try? session.finishReport(ticket: delivery, index: index, outcome: 2, nowMs: self.monotonicMilliseconds())
                            try? session.stopDelivery(ticket: delivery, reason: 3, nowMs: self.monotonicMilliseconds())
                            throw error
                        }
                    }
                    if let pause = plan["interMessageIntervalMs"] as? NSNumber, pause.intValue > 0 {
                        Thread.sleep(forTimeInterval: Double(pause.intValue) / 1_000)
                    }
                }
                guard try session.operationDeliveryComplete(operationTicket: operation, frameCount: reportCount) else {
                    throw QCIosProtocolSession.BridgeError.operationFailed
                }
                let verification = plan["verification"] ?? NSNull()
                let verificationKind = (verification as? [String: Any])?["kind"] as? String ?? "none"
                let realtime = plan["realtime"] as? Bool ?? false
                guard !realtime, verificationKind != "none",
                      let context = plan["verificationContext"] as? [String: Any] else {
                    call.resolve(["detail": plan["detail"] ?? "QC accepted the report transport; device state has not been read back.", "accepted": true, "verified": false, "verification": "accepted_unverified"])
                    return
                }
                let verificationID = try session.beginGatewayVerification(
                    context: context, verification: verification, method: method,
                    afterSequence: afterSequence, startedAtMs: (plan["startedAtMs"] as? NSNumber)?.uint64Value ?? now
                )
                let pending = PendingGatewayWrite(call: call, method: method, plan: plan, verification: verification,
                                                  verificationID: verificationID, relevantSequence: afterSequence)
                self.pendingGatewayWrites[verificationID] = pending
                self.scheduleGatewayWriteVerification(verificationID, afterMs: (plan["confirmationTimeoutMs"] as? NSNumber)?.uint64Value ?? 3_000)
            } catch {
                if persistentMutation && reportMayHaveReachedDevice {
                    self.driverWritesQuarantined = true
                }
                call.reject("The Quad Cortex write could not be delivered: \(error)", "WRITE_DELIVERY_FAILED")
            }
        }
    }

    private static let presetWorkflowMethods: Set<String> = [
        "device.createSetlist", "device.deleteSetlist", "device.duplicateSetlist",
        "device.deletePreset", "device.movePreset", "device.copyPreset",
        "device.savePresetAs", "device.renameCurrentPreset",
    ]

    private func advancePresetWorkflow(call: CAPPluginCall, session: QCIosProtocolSession,
                                       preparationId: UInt64, generation: UInt64) {
        guard driverGeneration == generation, protocolSession === session, driverClient.isOpen else {
            if presetWorkflowMutationAttempts.contains(preparationId) { driverWritesQuarantined = true }
            presetWorkflowMutationAttempts.remove(preparationId)
            session.cancelPresetWorkflow(preparationId: preparationId)
            call.reject("The Quad Cortex disconnected during the preset workflow.", "DRIVER_DISCONNECTED")
            return
        }
        do {
            let action = try session.advancePresetWorkflow(preparationId: preparationId, nowMs: monotonicMilliseconds())
            switch action["kind"] as? String {
            case "wait":
                let delay = max(1, (action["delayMs"] as? NSNumber)?.intValue ?? 10)
                driverQueue.asyncAfter(deadline: .now() + .milliseconds(delay)) {
                    self.advancePresetWorkflow(call: call, session: session, preparationId: preparationId, generation: generation)
                }
            case "write":
                let isMutation = action["read"] as? Bool != true
                try deliverPresetWorkflowAction(action, session: session) {
                    if isMutation { self.presetWorkflowMutationAttempts.insert(preparationId) }
                }
                driverQueue.asyncAfter(deadline: .now() + .milliseconds(10)) {
                    self.advancePresetWorkflow(call: call, session: session, preparationId: preparationId, generation: generation)
                }
            case "finished":
                if let error = action["error"] as? String {
                    if presetWorkflowMutationAttempts.contains(preparationId) { driverWritesQuarantined = true }
                    presetWorkflowMutationAttempts.remove(preparationId)
                    call.reject(error, "PRESET_WORKFLOW_FAILED")
                } else {
                    presetWorkflowMutationAttempts.remove(preparationId)
                    call.resolve((action["result"] as? [String: Any]) ?? ["accepted": true, "verified": false])
                }
            default:
                throw QCIosProtocolSession.BridgeError.invalidResponse
            }
        } catch {
            if presetWorkflowMutationAttempts.contains(preparationId) { driverWritesQuarantined = true }
            presetWorkflowMutationAttempts.remove(preparationId)
            session.cancelPresetWorkflow(preparationId: preparationId)
            call.reject("The Quad Cortex preset workflow failed: \(error)", "PRESET_WORKFLOW_FAILED")
        }
    }

    private func deliverPresetWorkflowAction(
        _ action: [String: Any],
        session: QCIosProtocolSession,
        onReportAttempt: (() -> Void)? = nil
    ) throws {
        guard let dispatch = action["dispatchTicket"] as? [String: Any],
              let operation = action["operationTicket"] as? [String: Any],
              let messages = action["messages"] as? [[String: Any]], !messages.isEmpty else {
            throw QCIosProtocolSession.BridgeError.invalidResponse
        }
        var reportCount = 0
        for message in messages {
            guard let type = (message["messageType"] as? NSNumber)?.uint16Value,
                  let payloadValues = message["payload"] as? [NSNumber] else {
                throw QCIosProtocolSession.BridgeError.invalidResponse
            }
            let bytes = payloadValues.map(\.intValue)
            guard bytes.allSatisfy({ (0...255).contains($0) }) else { throw QCIosProtocolSession.BridgeError.invalidResponse }
            let reports = try session.encodeMessage(messageType: type, payload: Data(bytes.map(UInt8.init)))
            let delivery = try session.prepareDelivery(dispatchTicket: dispatch, messageType: type,
                                                       reportCount: reports.count, nowMs: monotonicMilliseconds())
            _ = try session.deliveryAdmitted(ticket: delivery)
            for (index, report) in reports.enumerated() {
                _ = try session.beginReport(ticket: delivery, index: index, nowMs: monotonicMilliseconds())
                do {
                    onReportAttempt?()
                    let result = try driverClient.writeReport(report)
                    let transferred = result["bytesTransferred"] ?? 0
                    let usbStatus = result["usbStatus"] ?? -1
                    let outcome: UInt8 = transferred == 129 ? (usbStatus == Int(-536854449) ? 1 : 0) : 2
                    guard outcome != 2 else { throw QCIosDriverClient.ClientError.malformedResponse }
                    _ = try session.finishReport(ticket: delivery, index: index, outcome: outcome, nowMs: monotonicMilliseconds())
                    reportCount += 1
                } catch {
                    try? session.finishReport(ticket: delivery, index: index, outcome: 2, nowMs: monotonicMilliseconds())
                    try? session.stopDelivery(ticket: delivery, reason: 3, nowMs: monotonicMilliseconds())
                    throw error
                }
            }
            if let pause = action["interMessageIntervalMs"] as? NSNumber, pause.intValue > 0 {
                Thread.sleep(forTimeInterval: Double(pause.intValue) / 1_000)
            }
        }
        guard try session.operationDeliveryComplete(operationTicket: operation, frameCount: reportCount) else {
            throw QCIosProtocolSession.BridgeError.operationFailed
        }
    }

    private func scheduleDirectBackupAdvance(_ backup: PendingDirectBackup, afterMs: Int) {
        driverQueue.asyncAfter(deadline: .now() + .milliseconds(max(1, afterMs))) {
            guard self.pendingDirectBackup === backup,
                  !backup.waitingForRecoverySync else { return }
            guard self.driverGeneration == backup.generation,
                  self.protocolSession === backup.session,
                  self.driverClient.isOpen else {
                self.failDirectBackup(backup, message: "The QC disconnected during backup export.", code: "DRIVER_DISCONNECTED")
                return
            }
            do {
                let action = try backup.session.advanceBackup(nowMs: self.monotonicMilliseconds())
                switch action["action"] as? String {
                case "wait":
                    self.scheduleDirectBackupAdvance(backup, afterMs: 100)
                case "write":
                    try self.deliverPresetWorkflowAction(action, session: backup.session)
                    backup.requestDispatched = true
                    self.scheduleDirectBackupAdvance(backup, afterMs: 100)
                case "failed":
                    self.failDirectBackup(backup,
                        message: action["error"] as? String ?? "The QC backup transfer timed out.",
                        code: "BACKUP_FAILED")
                default:
                    throw QCIosProtocolSession.BridgeError.invalidResponse
                }
            } catch {
                self.failDirectBackup(backup, message: "The direct QC backup failed: \(error)", code: "BACKUP_FAILED")
            }
        }
    }

    private func failDirectBackup(_ backup: PendingDirectBackup, message: String, code: String) {
        guard pendingDirectBackup === backup else { return }
        if backup.requestDispatched { driverWritesQuarantined = true }
        backup.session.cancelBackup()
        pendingDirectBackup = nil
        backup.call.reject(message, code)
    }

    private func saveDirectBackup(_ progress: [String: Any], backup: PendingDirectBackup) throws -> [String: Any] {
        guard progress["complete"] as? Bool == true,
              let document = progress["document"] as? [String: Any],
              JSONSerialization.isValidJSONObject(document) else {
            throw QCIosProtocolSession.BridgeError.invalidResponse
        }
        let bytes = try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
        guard bytes.count <= 32 * 1_024 * 1_024 else {
            throw NSError(domain: "QCIosBackup", code: 1, userInfo: [NSLocalizedDescriptionKey: "The native backup exceeds the 32 MiB safety limit."])
        }
        guard let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first else {
            throw NSError(domain: "QCIosBackup", code: 2, userInfo: [NSLocalizedDescriptionKey: "The iOS backup folder is unavailable."])
        }
        let folder = documents.appendingPathComponent("Quad Cortex Backups", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let stem = backupFileStem(backup.name)
        var url = folder.appendingPathComponent("\(stem).json")
        if FileManager.default.fileExists(atPath: url.path) {
            let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "-")
            url = folder.appendingPathComponent("\(stem) \(stamp).json")
        }
        try bytes.write(to: url, options: .atomic)
        return ["cancelled": false, "path": url.path, "name": url.lastPathComponent]
    }

    private func backupFileStem(_ name: String) -> String {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: " -_"))
        let stem = name.unicodeScalars.map { allowed.contains($0) ? String($0) : "_" }.joined()
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return stem.isEmpty ? "QC Device Backup" : String(stem.prefix(64))
    }

    private func handleDirectBackupProgress(_ progress: [String: Any]?) {
        guard let progress, let backup = pendingDirectBackup else { return }
        guard progress["complete"] as? Bool == true else { return }
        do {
            let saved = try saveDirectBackup(progress, backup: backup)
            try backup.session.finishBackup()
            backup.savedResult = saved
            backup.waitingForRecoverySync = true
            try restartDriverProtocolAfterBackup(backup)
        } catch {
            failDirectBackup(backup,
                message: "The QC backup transfer completed, but saving or restoring the USB session failed: \(error)",
                code: "BACKUP_RECOVERY_FAILED")
        }
    }

    private func restartDriverProtocolAfterBackup(_ backup: PendingDirectBackup) throws {
        guard pendingDirectBackup === backup, driverGeneration == backup.generation else {
            throw QCIosProtocolSession.BridgeError.operationFailed
        }
        driverGeneration &+= 1
        driverClient.close()
        try driverClient.open()
        let session = try QCIosProtocolSession()
        let now = monotonicMilliseconds()
        let connection = try session.opened(nowMs: now)
        driverStateSequence = 0
        driverConnection = connection
        driverSessionID = UUID().uuidString.lowercased()
        protocolSession = session
        driverSynchronized = false
        backup.generation = driverGeneration
        scheduleDriverProtocolPump(generation: driverGeneration, session: session)
        notifyListeners("qcConnection", data: ["state": "available", "synchronized": false,
                                                "detail": "Backup saved. Reconnecting the QC protocol session."],
                        retainUntilConsumed: false)
        driverQueue.asyncAfter(deadline: .now() + .seconds(30)) {
            guard self.pendingDirectBackup === backup, backup.waitingForRecoverySync else { return }
            self.pendingDirectBackup = nil
            backup.call.reject("Backup saved to \(backup.savedResult?["path"] as? String ?? "Files"), but the QC protocol session did not resynchronize.", "BACKUP_RECOVERY_FAILED")
        }
    }

    @objc public func driverCurrentPreset(_ call: CAPPluginCall) {
        driverQueue.async {
            guard let session = self.protocolSession else {
                call.reject("Connect the Quad Cortex through the iPad USB driver first.", "DRIVER_NOT_CONNECTED")
                return
            }
            do {
                let snapshot = try session.currentPreset()
                call.resolve(["snapshot": snapshot ?? NSNull()])
            } catch {
                call.reject("Could not read the local Quad Cortex snapshot: \(error)", "SNAPSHOT_READ_FAILED")
            }
        }
    }

    private struct UsbMidiDevice {
        let id: Int32
        let name: String
        let manufacturer: String
        let destination: MIDIEndpointRef?
        var json: [String: Any] {
            ["deviceId": Int(id), "name": name, "manufacturer": manufacturer, "connected": false]
        }
    }

    private func usbMidiDevices() -> [UsbMidiDevice] {
        var result: [UsbMidiDevice] = []
        for index in 0..<MIDIGetNumberOfDevices() {
            let device = MIDIGetDevice(index)
            let transport = midiString(device, key: qcMidiTransportTypeProperty)
            guard transport.caseInsensitiveCompare(qcMidiTransportTypeUSB) == .orderedSame else { continue }
            let name = midiString(device, key: kMIDIPropertyName)
            let manufacturer = midiString(device, key: kMIDIPropertyManufacturer)
            var uniqueID: Int32 = 0
            guard MIDIObjectGetIntegerProperty(device, kMIDIPropertyUniqueID, &uniqueID) == noErr else { continue }
            var destination: MIDIEndpointRef?
            for entityIndex in 0..<MIDIDeviceGetNumberOfEntities(device) {
                let entity = MIDIDeviceGetEntity(device, entityIndex)
                for endpointIndex in 0..<MIDIEntityGetNumberOfDestinations(entity) {
                    let endpoint = MIDIEntityGetDestination(entity, endpointIndex)
                    if endpoint != 0 { destination = endpoint; break }
                }
                if destination != nil { break }
            }
            guard destination != nil else { continue }
            let lowerName = name.lowercased()
            let lowerManufacturer = manufacturer.lowercased()
            guard lowerName.contains("quad cortex") || lowerName.contains("cortex mini")
                    || lowerManufacturer.contains("neural dsp") else { continue }
            result.append(UsbMidiDevice(id: uniqueID, name: name, manufacturer: manufacturer, destination: destination))
        }
        return result
    }

    private func midiString(_ object: MIDIObjectRef, key: CFString) -> String {
        var value: Unmanaged<CFString>?
        guard MIDIObjectGetStringProperty(object, key, &value) == noErr, let value else { return "" }
        return value.takeRetainedValue() as String
    }

    private func sendDirectMidiControlChange(controller: Int, value: Int, channel: Int) throws {
        guard (1...16).contains(channel), (0...127).contains(controller), (0...127).contains(value) else {
            throw QCIosDriverClient.ClientError.malformedResponse
        }
        try midiQueue.sync {
            if midiOutput == nil || midiDestination == nil {
                guard let device = usbMidiDevices().first, let destination = device.destination else {
                    throw QCIosDriverClient.ClientError.serviceUnavailable
                }
                var client: MIDIClientRef = 0
                let clientStatus = MIDIClientCreate("QC Remote" as CFString, nil, nil, &client)
                guard clientStatus == noErr else { throw NSError(domain: NSOSStatusErrorDomain, code: Int(clientStatus)) }
                var output: MIDIPortRef = 0
                let portStatus = MIDIOutputPortCreate(client, "QC Remote Output" as CFString, &output)
                guard portStatus == noErr else {
                    MIDIClientDispose(client)
                    throw NSError(domain: NSOSStatusErrorDomain, code: Int(portStatus))
                }
                midiClient = client
                midiOutput = output
                midiDestination = destination
                midiDeviceID = device.id
            }
            var packetList = MIDIPacketList()
            let bytes = [UInt8(0xB0 | (channel - 1)), UInt8(controller), UInt8(value)]
            let status = bytes.withUnsafeBufferPointer { buffer -> OSStatus in
                guard let baseAddress = buffer.baseAddress else { return qcParamErr }
                var packet = MIDIPacketListInit(&packetList)
                packet = MIDIPacketListAdd(&packetList, MemoryLayout<MIDIPacketList>.size, packet, 0, bytes.count, baseAddress)
                guard packet != nil, let output = midiOutput, let destination = midiDestination else { return qcMemFullErr }
                return MIDISend(output, destination, &packetList)
            }
            guard status == noErr else { throw NSError(domain: NSOSStatusErrorDomain, code: Int(status)) }
        }
    }

    @discardableResult
    private func sendMidiBytes(_ bytes: [UInt8], call: CAPPluginCall, resolve: Bool = true) -> Bool {
        guard let output = midiOutput, let destination = midiDestination else {
            call.reject("Connect the Quad Cortex USB-MIDI device first.", "MIDI_NOT_CONNECTED")
            return false
        }
        var packetList = MIDIPacketList()
        let status = bytes.withUnsafeBufferPointer { buffer -> OSStatus in
            guard let baseAddress = buffer.baseAddress else { return qcParamErr }
            var packet = MIDIPacketListInit(&packetList)
            packet = MIDIPacketListAdd(&packetList, MemoryLayout<MIDIPacketList>.size, packet, 0, bytes.count, baseAddress)
            guard packet != nil else { return qcMemFullErr }
            return MIDISend(output, destination, &packetList)
        }
        guard status == noErr else {
            call.reject("Core MIDI could not send the message (\(status)).", "MIDI_WRITE_FAILED")
            return false
        }
        if resolve { call.resolve(["accepted": true, "verified": false, "transport": "usb-midi"]) }
        return true
    }

    private func closeMidiSession() {
        if let output = midiOutput { MIDIPortDispose(output) }
        if let client = midiClient { MIDIClientDispose(client) }
        midiOutput = nil
        midiClient = nil
        midiDestination = nil
        midiDeviceID = nil
    }

    private func monotonicMilliseconds() -> UInt64 {
        UInt64(max(0, ProcessInfo.processInfo.systemUptime * 1_000))
    }

    private func startDriverReader(generation: UInt64, session: QCIosProtocolSession, connection: UInt64) {
        guard driverReaderGeneration != generation else { return }
        driverReaderGeneration = generation
        driverReadQueue.async {
            while self.driverGenerationIsCurrent(generation) {
                do {
                    let epoch = try session.reportEpoch(connection: connection)
                    if epoch == 0 {
                        Thread.sleep(forTimeInterval: 0.01)
                        continue
                    }
                    let report = try self.driverClient.readReport()
                    self.driverQueue.async {
                        guard self.driverGeneration == generation,
                              self.driverConnection == connection else { return }
                        do {
                            if let event = try session.pushReport(report, connection: connection, epoch: epoch, receivedAtMs: self.monotonicMilliseconds()) {
                                self.publishDriverEvent(event)
                            }
                            self.pumpDriverProtocol(generation: generation, session: session)
                        } catch {
                            self.notifyListeners("qcConnection", data: ["state": "disconnected", "detail": "QC USB report decoding failed: \(error)"], retainUntilConsumed: false)
                            self.stopDriverSession(generation: generation)
                        }
                    }
                } catch {
                    if self.driverGenerationIsCurrent(generation) {
                        self.driverQueue.async {
                            guard self.driverGeneration == generation else { return }
                            self.notifyListeners("qcConnection", data: ["state": "disconnected", "detail": "QC USB report read failed: \(error)"], retainUntilConsumed: false)
                            self.stopDriverSession(generation: generation)
                        }
                    }
                    return
                }
            }
        }
    }

    private func driverGenerationIsCurrent(_ generation: UInt64) -> Bool {
        driverQueue.sync { driverGeneration == generation && protocolSession != nil }
    }

    private func scheduleDriverProtocolPump(generation: UInt64, session: QCIosProtocolSession) {
        driverQueue.asyncAfter(deadline: .now() + .milliseconds(100)) {
            guard self.driverGeneration == generation, self.protocolSession === session else { return }
            self.pumpDriverProtocol(generation: generation, session: session)
            self.scheduleDriverProtocolPump(generation: generation, session: session)
        }
    }

    private func pumpDriverProtocol(generation: UInt64, session: QCIosProtocolSession) {
        guard driverGeneration == generation, protocolSession === session else { return }
        let now = monotonicMilliseconds()
        do {
            if let handshake = try session.nextHandshake(nowMs: now, sessionID: driverSessionID) {
                let epoch = try session.reportEpoch(connection: driverConnection)
                guard epoch != 0 else { throw QCIosProtocolSession.BridgeError.operationFailed }
                let accepted = try writeProtocolReports(handshake["reports"])
                if !accepted {
                    notifyListeners("qcConnection", data: ["state": "disconnected", "detail": "The QC handshake report was not fully transferred."], retainUntilConsumed: false)
                    stopDriverSession(generation: generation)
                    return
                }
                startDriverReader(generation: generation, session: session, connection: driverConnection)
            }
            guard let batch = try session.nextLifecycleBatch(nowMs: now, unixTimeMs: UInt64(Date().timeIntervalSince1970 * 1_000)) else { return }
            let ticket = try JSONSerialization.data(withJSONObject: batch["ticket"] ?? NSNull())
            var accepted = true
            for message in batch["messages"] as? [[String: Any]] ?? [] {
                guard try writeProtocolReports(message["reports"]) else { accepted = false; break }
            }
            _ = try session.lifecycleBatchDispatched(ticket: String(decoding: ticket, as: UTF8.self), accepted: accepted, nowMs: monotonicMilliseconds())
            if !accepted {
                notifyListeners("qcConnection", data: ["state": "disconnected", "detail": "A QC startup report was not fully transferred."], retainUntilConsumed: false)
                stopDriverSession(generation: generation)
            }
        } catch {
            notifyListeners("qcConnection", data: ["state": "disconnected", "detail": "QC protocol startup failed: \(error)"], retainUntilConsumed: false)
            stopDriverSession(generation: generation)
        }
    }

    private func sendGatewayReadPlan(_ plan: [String: Any], session: QCIosProtocolSession, readID: String) {
        do {
            guard let messages = plan["messages"] as? [[String: Any]], !messages.isEmpty else {
                throw QCIosProtocolSession.BridgeError.invalidResponse
            }
            for (messageIndex, message) in messages.enumerated() {
                guard let type = (message["messageType"] as? NSNumber)?.uint16Value,
                      let payloadValues = message["payload"] as? [NSNumber] else {
                    throw QCIosProtocolSession.BridgeError.invalidResponse
                }
                let payloadBytes = payloadValues.map(\.intValue)
                guard payloadBytes.allSatisfy({ (0...255).contains($0) }) else {
                    throw QCIosProtocolSession.BridgeError.invalidResponse
                }
                let reports = try session.encodeMessage(messageType: type, payload: Data(payloadBytes.map(UInt8.init)))
                for report in reports {
                    let result = try driverClient.writeReport(report)
                    guard result["bytesTransferred"] == 129 else {
                        throw QCIosDriverClient.ClientError.malformedResponse
                    }
                }
                if messageIndex + 1 < messages.count { Thread.sleep(forTimeInterval: 0.008) }
            }
        } catch {
            pendingGatewayReads.removeValue(forKey: readID)?.call.reject("Quad Cortex read request could not be sent: \(error)", "READ_WRITE_FAILED")
        }
    }

    private func writeProtocolReports(_ value: Any?) throws -> Bool {
        guard let reports = value as? [[NSNumber]], !reports.isEmpty else { return false }
        for values in reports {
            let raw = values.map(\.intValue)
            guard raw.count == 129, raw.allSatisfy({ (0...255).contains($0) }) else { return false }
            let result = try driverClient.writeReport(Data(raw.map(UInt8.init)))
            guard result["bytesTransferred"] == 129 else { return false }
        }
        return true
    }

    private func publishDriverEvent(_ event: [String: Any]) {
        resolvePendingGatewayReads(with: event)
        let backupProgress = event["backupProgress"] as? [String: Any]
        if let lifecycle = event["lifecycle"] as? [String: Any] {
            let phase = lifecycle["phase"] as? String ?? "syncing"
            let connected = phase == "connected"
                && (lifecycle["sessionValidated"] as? Bool) == true
                && (lifecycle["synchronized"] as? Bool) == true
                && lifecycle["error"] is NSNull
            let becameReady = connected && !driverSynchronized
            driverSynchronized = connected
            if becameReady { driverWritesQuarantined = false }
            if driverSynchronized, let backup = pendingDirectBackup,
               backup.waitingForRecoverySync, backup.generation == driverGeneration {
                pendingDirectBackup = nil
                backup.call.resolve(backup.savedResult ?? ["cancelled": false])
            }
        }
        guard let states = event["states"] as? [[String: Any]], !states.isEmpty else {
            handleDirectBackupProgress(backupProgress)
            return
        }
        driverStateSequence &+= 1
        advanceGatewayWriteVerifications(states: states, sequence: driverStateSequence)
        var frame: [String: Any] = [
            "streamId": driverSessionID,
            "sequence": driverStateSequence,
            "observedAt": Date().timeIntervalSince1970 * 1_000,
            "states": states
        ]
        if let tempoClock = event["tempoClock"] as? [String: Any] { frame["tempoClock"] = tempoClock }
        notifyListeners("qcStateBatch", data: frame, retainUntilConsumed: false)
        handleDirectBackupProgress(backupProgress)
    }

    private func advanceGatewayWriteVerifications(states: [[String: Any]], sequence: UInt64) {
        guard let session = protocolSession else { return }
        for (id, pending) in Array(pendingGatewayWrites) {
            let expectedKinds = (pending.plan["observedStateKinds"] as? [String]) ?? []
            if states.contains(where: { state in expectedKinds.contains(state["kind"] as? String ?? "") }) {
                pending.relevantSequence = max(pending.relevantSequence, sequence)
            }
            do {
                let action = try session.advanceGatewayVerification(id: id, observationSequence: pending.relevantSequence,
                                                                    nowMs: monotonicMilliseconds(), method: pending.method,
                                                                    verification: pending.verification)
                handleGatewayWriteVerificationAction(action, pending: pending, id: id)
            } catch {
                pendingGatewayWrites.removeValue(forKey: id)
                driverWritesQuarantined = true
                pending.call.reject("The Quad Cortex write was sent, but its state verification failed: \(error)", "WRITE_VERIFICATION_FAILED")
            }
        }
    }

    private func scheduleGatewayWriteVerification(_ id: UInt64, afterMs: UInt64) {
        guard let pending = pendingGatewayWrites[id], !pending.wakeScheduled else { return }
        pending.wakeScheduled = true
        driverQueue.asyncAfter(deadline: .now() + .milliseconds(Int(min(afterMs, UInt64(Int.max))))) {
            guard let current = self.pendingGatewayWrites[id], current === pending else { return }
            current.wakeScheduled = false
            guard let session = self.protocolSession else {
                self.pendingGatewayWrites.removeValue(forKey: id)
                self.driverWritesQuarantined = true
                current.call.reject("The Quad Cortex disconnected before write verification completed.", "DRIVER_DISCONNECTED")
                return
            }
            do {
                let action = try session.advanceGatewayVerification(id: id, observationSequence: current.relevantSequence,
                                                                    nowMs: self.monotonicMilliseconds(), method: current.method,
                                                                    verification: current.verification)
                self.handleGatewayWriteVerificationAction(action, pending: current, id: id)
            } catch {
                self.pendingGatewayWrites.removeValue(forKey: id)
                self.driverWritesQuarantined = true
                current.call.reject("The Quad Cortex write was sent, but its state verification failed: \(error)", "WRITE_VERIFICATION_FAILED")
            }
        }
    }

    private func handleGatewayWriteVerificationAction(_ action: [String: Any], pending: PendingGatewayWrite, id: UInt64) {
        switch action["kind"] as? String {
        case "wait":
            let delay = (action["delayMs"] as? NSNumber)?.uint64Value ?? 1_000
            scheduleGatewayWriteVerification(id, afterMs: max(1, delay))
        case "verified":
            pendingGatewayWrites.removeValue(forKey: id)
            var result: [String: Any] = ["detail": pending.plan["detail"] ?? "The Quad Cortex state matches the requested change.",
                                         "accepted": true, "verified": true, "verification": "authoritative_readback"]
            if let value = action["pluginBlockReadback"] { result["pluginBlockReadback"] = value }
            if let value = action["outcome"] { result["outcome"] = value }
            pending.call.resolve(result)
        case "timedOut":
            pendingGatewayWrites.removeValue(forKey: id)
            driverWritesQuarantined = true
            pending.call.resolve(["detail": pending.plan["detail"] ?? "QC accepted the report transport; no confirming state update arrived.",
                                  "accepted": true, "verified": false, "verification": "accepted_unverified"])
        default:
            pendingGatewayWrites.removeValue(forKey: id)
            driverWritesQuarantined = true
            pending.call.reject("The shared QC verifier returned an unknown result.", "WRITE_VERIFICATION_FAILED")
        }
    }

    private func resolvePendingGatewayReads(with event: [String: Any]) {
        let now = monotonicMilliseconds()
        for (readID, pending) in Array(pendingGatewayReads) {
            if now >= pending.deadlineMs {
                pendingGatewayReads.removeValue(forKey: readID)
                pending.call.reject("The Quad Cortex did not return a valid \(pending.method) response before the read timed out.", "READ_TIMEOUT")
                continue
            }
            do {
                let observation = try protocolSession?.observeGatewayRead(plan: pending.plan, event: event, nowMs: now)
                guard let observation, let status = observation["status"] as? String else { continue }
                switch status {
                case "matched":
                    guard let value = observation["value"] as? [String: Any] else {
                        pendingGatewayReads.removeValue(forKey: readID)
                        pending.call.reject("The correlated QC read returned an invalid value.", "MALFORMED_RESPONSE")
                        continue
                    }
                    if pending.firstReadValue == nil,
                       let followupMethod = pending.plan["followupMethod"] as? String {
                        do {
                            guard let session = protocolSession else { throw QCIosProtocolSession.BridgeError.unavailable }
                            let nextPlan = try session.planGatewayRead(method: followupMethod, params: [:], startedAtMs: now)
                            pendingGatewayReads[readID] = PendingGatewayRead(
                                call: pending.call,
                                method: pending.method,
                                params: pending.params,
                                plan: nextPlan,
                                deadlineMs: pending.deadlineMs,
                                firstReadValue: value
                            )
                            sendGatewayReadPlan(nextPlan, session: session, readID: readID)
                            continue
                        } catch {
                            pendingGatewayReads.removeValue(forKey: readID)
                            pending.call.reject("The follow-up QC read could not be started: \(error)", "READ_FOLLOWUP_FAILED")
                            continue
                        }
                    }
                    let finalValue: [String: Any]
                    if let firstValue = pending.firstReadValue {
                        do {
                            guard let session = protocolSession else { throw QCIosProtocolSession.BridgeError.unavailable }
                            finalValue = try session.composeGlobalTempoSettings(global: firstValue, preset: value)
                        } catch {
                            pendingGatewayReads.removeValue(forKey: readID)
                            pending.call.reject("The paired QC tempo readbacks could not be combined: \(error)", "MALFORMED_RESPONSE")
                            continue
                        }
                    } else {
                        finalValue = value
                    }
                    pendingGatewayReads.removeValue(forKey: readID)
                    pending.call.resolve(finalValue)
                case "expired":
                    pendingGatewayReads.removeValue(forKey: readID)
                    pending.call.reject("The Quad Cortex read response arrived after its deadline.", "READ_TIMEOUT")
                default:
                    if now >= pending.deadlineMs {
                        pendingGatewayReads.removeValue(forKey: readID)
                        pending.call.reject("The Quad Cortex did not return a valid response before the read timed out.", "READ_TIMEOUT")
                    }
                }
            } catch {
                // Malformed and unrelated reports do not consume a pending correlated read.
                continue
            }
        }
    }

    private func stopDriverSession(generation: UInt64) {
        guard driverGeneration == generation else { return }
        driverGeneration &+= 1
        protocolSession = nil
        driverConnection = 0
        driverSessionID = ""
        driverSynchronized = false
        rejectPendingGatewayReads("The Quad Cortex connection ended before the read completed.", code: "DRIVER_DISCONNECTED")
        rejectPendingGatewayWrites("The Quad Cortex connection ended before write verification completed.")
        driverClient.close()
    }

    private func rejectPendingGatewayReads(_ message: String, code: String) {
        let pending = Array(pendingGatewayReads.values)
        pendingGatewayReads.removeAll()
        for read in pending { read.call.reject(message, code) }
    }

    private func rejectPendingGatewayWrites(_ message: String) {
        let pending = Array(pendingGatewayWrites.values)
        pendingGatewayWrites.removeAll()
        if !pending.isEmpty { driverWritesQuarantined = true }
        for write in pending { write.call.reject(message, "DRIVER_DISCONNECTED") }
    }
}
