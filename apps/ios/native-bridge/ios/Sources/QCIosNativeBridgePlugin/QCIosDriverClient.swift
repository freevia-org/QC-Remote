import Foundation
import IOKit

/// Thin client for the bundled QC DriverKit user client. USB session ownership
/// remains in the DEXT; this class only passes bounded report structures.
final class QCIosDriverClient {
    enum ClientError: Error {
        case serviceUnavailable
        case openFailed(kern_return_t)
        case callFailed(kern_return_t)
        case disconnected
        case malformedResponse
    }

    private let stateLock = NSLock()
    private var connection: io_connect_t = 0

    var isOpen: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return connection != 0
    }

    static func isServiceAvailable() -> Bool {
        guard let matching = IOServiceNameMatching("QCRemoteUSBDriver") else { return false }
        let service = IOServiceGetMatchingService(kIOMainPortDefault, matching)
        guard service != IO_OBJECT_NULL else { return false }
        IOObjectRelease(service)
        return true
    }

    static func canOpenService() -> Bool {
        guard let matching = IOServiceNameMatching("QCRemoteUSBDriver") else { return false }
        let service = IOServiceGetMatchingService(kIOMainPortDefault, matching)
        guard service != IO_OBJECT_NULL else { return false }
        defer { IOObjectRelease(service) }

        var connection: io_connect_t = 0
        let status = IOServiceOpen(service, mach_task_self_, 0, &connection)
        guard status == kIOReturnSuccess else { return false }
        IOServiceClose(connection)
        return true
    }

    func open() throws {
        stateLock.lock()
        if connection != 0 {
            stateLock.unlock()
            return
        }
        stateLock.unlock()

        guard let matching = IOServiceNameMatching("QCRemoteUSBDriver") else {
            throw ClientError.serviceUnavailable
        }
        let service = IOServiceGetMatchingService(kIOMainPortDefault, matching)
        guard service != IO_OBJECT_NULL else { throw ClientError.serviceUnavailable }
        defer { IOObjectRelease(service) }

        var opened: io_connect_t = 0
        let status = IOServiceOpen(service, mach_task_self_, 0, &opened)
        guard status == kIOReturnSuccess else { throw ClientError.openFailed(status) }

        stateLock.lock()
        if connection == 0 {
            connection = opened
            stateLock.unlock()
        } else {
            stateLock.unlock()
            IOServiceClose(opened)
        }
    }

    func close() {
        stateLock.lock()
        let active = connection
        connection = 0
        stateLock.unlock()
        if active != 0 { IOServiceClose(active) }
    }

    func diagnostics() throws -> [String: Int] {
        var response = [UInt8](repeating: 0, count: 24)
        let count = try call(selector: 2, input: nil, output: &response)
        guard count == 20 || count == 24 else { throw ClientError.malformedResponse }
        let version = readUInt32(response, 0)
        guard version == 1 && count == 20 || version == 2 && count == 24 else {
            throw ClientError.malformedResponse
        }
        return [
            "version": version,
            "interfaceNumber": readUInt32(response, 4),
            "inputEndpointAddress": readUInt32(response, 8),
            "inputPacketSize": readUInt32(response, 12),
            "reportLength": readUInt32(response, 16),
            "reserved": count == 24 ? readUInt32(response, 20) : 0
        ]
    }

    func writeReport(_ report: Data) throws -> [String: Int] {
        guard report.count == 129, report.first == 2 else { throw ClientError.malformedResponse }
        var response = [UInt8](repeating: 0, count: 8)
        let count = try report.withUnsafeBytes { input in
            try call(selector: 1, input: input.baseAddress, inputCount: report.count, output: &response)
        }
        guard count == 8 else { throw ClientError.malformedResponse }
        return ["bytesTransferred": readUInt32(response, 0), "usbStatus": readUInt32(response, 4)]
    }

    /// This call waits for the next interrupt-IN report; invoke it off the UI
    /// queue and cancel it by closing the user-client connection.
    func readReport() throws -> Data {
        var response = [UInt8](repeating: 0, count: 129)
        let count = try call(selector: 0, input: nil, output: &response)
        guard count == 129, response.first == 1 else { throw ClientError.malformedResponse }
        return Data(response)
    }

    private func call(selector: UInt32, input: UnsafeRawPointer?, inputCount: Int = 0, output: inout [UInt8]) throws -> Int {
        stateLock.lock()
        let active = connection
        stateLock.unlock()
        guard active != 0 else { throw ClientError.disconnected }
        var outputCount = output.count
        let status = output.withUnsafeMutableBytes { buffer in
            IOConnectCallStructMethod(
                active,
                selector,
                input,
                inputCount,
                buffer.baseAddress,
                &outputCount
            )
        }
        guard status == kIOReturnSuccess else { throw ClientError.callFailed(status) }
        return outputCount
    }

    private func readUInt32(_ bytes: [UInt8], _ offset: Int) -> Int {
        Int(UInt32(bytes[offset])
            | UInt32(bytes[offset + 1]) << 8
            | UInt32(bytes[offset + 2]) << 16
            | UInt32(bytes[offset + 3]) << 24)
    }
}
