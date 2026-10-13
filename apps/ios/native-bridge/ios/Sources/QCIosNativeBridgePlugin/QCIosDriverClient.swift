import Foundation
import QCIosDriverKitClient

/// Thin client for the bundled QC DriverKit user client. USB session ownership
/// remains in the DEXT; this class only passes bounded report structures.
final class QCIosDriverClient {
    enum ClientError: Error {
        case serviceUnavailable
        case openFailed(Int32)
        case callFailed(Int32)
        case disconnected
        case malformedResponse
    }

    private let stateLock = NSLock()
    private var connection: UInt32 = 0

    var isOpen: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return connection != 0
    }

    static func isServiceAvailable() -> Bool {
        qc_driver_service_available() != 0
    }

    static func canOpenService() -> Bool {
        var connection: UInt32 = 0
        guard qc_driver_open(&connection) == 0 else { return false }
        _ = qc_driver_close(connection)
        return true
    }

    func open() throws {
        stateLock.lock()
        if connection != 0 {
            stateLock.unlock()
            return
        }
        stateLock.unlock()

        var opened: UInt32 = 0
        let status = qc_driver_open(&opened)
        guard status == 0, opened != 0 else {
            if status == Int32(bitPattern: 0xe00002f0) { throw ClientError.serviceUnavailable }
            throw ClientError.openFailed(status)
        }

        stateLock.lock()
        if connection == 0 {
            connection = opened
            stateLock.unlock()
        } else {
            stateLock.unlock()
            _ = qc_driver_close(opened)
        }
    }

    func close() {
        stateLock.lock()
        let active = connection
        connection = 0
        stateLock.unlock()
        if active != 0 {
            // Interrupt-IN reads have no timeout. Abort the pipe before closing
            // the user client so a blocked reader can leave its dispatch queue.
            var response = [UInt8](repeating: 0, count: 4)
            var responseCount = UInt32(response.count)
            _ = response.withUnsafeMutableBytes { buffer in
                qc_driver_call(active, 3, nil, 0, buffer.baseAddress, &responseCount)
            }
            _ = qc_driver_close(active)
        }
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
        var outputCount = UInt32(output.count)
        let status = output.withUnsafeMutableBytes { buffer in
            qc_driver_call(active, selector, input, UInt32(inputCount), buffer.baseAddress, &outputCount)
        }
        guard status == 0 else { throw ClientError.callFailed(status) }
        return Int(outputCount)
    }

    private func readUInt32(_ bytes: [UInt8], _ offset: Int) -> Int {
        Int(UInt32(bytes[offset])
            | UInt32(bytes[offset + 1]) << 8
            | UInt32(bytes[offset + 2]) << 16
            | UInt32(bytes[offset + 3]) << 24)
    }
}
