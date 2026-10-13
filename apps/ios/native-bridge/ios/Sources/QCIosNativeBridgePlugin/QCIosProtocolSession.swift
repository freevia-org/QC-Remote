import Foundation

@_silgen_name("qc_ios_session_create")
private func qcIosSessionCreate() -> UnsafeMutableRawPointer?
@_silgen_name("qc_ios_session_destroy")
private func qcIosSessionDestroy(_ session: UnsafeMutableRawPointer?)
@_silgen_name("qc_ios_session_opened")
private func qcIosSessionOpened(_ session: UnsafeMutableRawPointer?, _ nowMs: UInt64) -> UInt64
@_silgen_name("qc_ios_report_epoch")
private func qcIosReportEpoch(_ session: UnsafeMutableRawPointer?, _ connection: UInt64) -> UInt64
@_silgen_name("qc_ios_push_report")
private func qcIosPushReport(
    _ session: UnsafeMutableRawPointer?, _ connection: UInt64, _ epoch: UInt64,
    _ report: UnsafePointer<UInt8>?, _ reportLength: Int, _ receivedAtMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_next_handshake")
private func qcIosNextHandshake(
    _ session: UnsafeMutableRawPointer?, _ nowMs: UInt64, _ sessionID: UnsafePointer<CChar>?,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_next_lifecycle_batch")
private func qcIosNextLifecycleBatch(
    _ session: UnsafeMutableRawPointer?, _ nowMs: UInt64, _ unixTimeMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_lifecycle_dispatched")
private func qcIosLifecycleDispatched(
    _ session: UnsafeMutableRawPointer?, _ ticket: UnsafePointer<CChar>?, _ accepted: Bool,
    _ nowMs: UInt64, _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_plan_gateway_write")
private func qcIosPlanGatewayWrite(
    _ session: UnsafeMutableRawPointer?, _ method: UnsafePointer<CChar>?,
    _ paramsJson: UnsafePointer<CChar>?, _ startedAtMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_begin_preset_workflow")
private func qcIosBeginPresetWorkflow(
    _ session: UnsafeMutableRawPointer?, _ method: UnsafePointer<CChar>?,
    _ paramsJson: UnsafePointer<CChar>?, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_advance_preset_workflow")
private func qcIosAdvancePresetWorkflow(
    _ session: UnsafeMutableRawPointer?, _ preparationId: UInt64, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_cancel_preset_workflow")
private func qcIosCancelPresetWorkflow(_ session: UnsafeMutableRawPointer?, _ preparationId: UInt64)
@_silgen_name("qc_ios_begin_backup")
private func qcIosBeginBackup(
    _ session: UnsafeMutableRawPointer?, _ name: UnsafePointer<CChar>?, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_advance_backup")
private func qcIosAdvanceBackup(
    _ session: UnsafeMutableRawPointer?, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_cancel_backup")
private func qcIosCancelBackup(_ session: UnsafeMutableRawPointer?)
@_silgen_name("qc_ios_finish_backup")
private func qcIosFinishBackup(_ session: UnsafeMutableRawPointer?) -> Bool
@_silgen_name("qc_ios_begin_gateway_verification")
private func qcIosBeginGatewayVerification(
    _ session: UnsafeMutableRawPointer?, _ contextJson: UnsafePointer<CChar>?,
    _ verificationJson: UnsafePointer<CChar>?, _ method: UnsafePointer<CChar>?,
    _ afterSequence: UInt64, _ startedAtMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_advance_gateway_verification")
private func qcIosAdvanceGatewayVerification(
    _ session: UnsafeMutableRawPointer?, _ verificationID: UInt64,
    _ observationSequence: UInt64, _ nowMs: UInt64, _ method: UnsafePointer<CChar>?,
    _ verificationJson: UnsafePointer<CChar>?, _ output: UnsafeMutablePointer<CChar>?,
    _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_plan_gateway_read")
private func qcIosPlanGatewayRead(
    _ session: UnsafeMutableRawPointer?, _ method: UnsafePointer<CChar>?,
    _ paramsJson: UnsafePointer<CChar>?, _ startedAtMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_observe_gateway_read")
private func qcIosObserveGatewayRead(
    _ projectionJson: UnsafePointer<CChar>?, _ expectedType: UInt16,
    _ actualType: UInt16, _ payload: UnsafePointer<UInt8>?, _ payloadLength: Int,
    _ startedAtMs: UInt64, _ timeoutMs: UInt64,
    _ ingressFirstMs: UInt64, _ ingressLastMs: UInt64, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_prepare_delivery")
private func qcIosPrepareDelivery(
    _ session: UnsafeMutableRawPointer?, _ dispatchJson: UnsafePointer<CChar>?,
    _ messageType: UInt16, _ reportCount: Int, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_delivery_admitted")
private func qcIosDeliveryAdmitted(
    _ session: UnsafeMutableRawPointer?, _ ticketJson: UnsafePointer<CChar>?,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_begin_report")
private func qcIosBeginReport(
    _ session: UnsafeMutableRawPointer?, _ ticketJson: UnsafePointer<CChar>?,
    _ index: Int, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_finish_report")
private func qcIosFinishReport(
    _ session: UnsafeMutableRawPointer?, _ ticketJson: UnsafePointer<CChar>?,
    _ index: Int, _ outcome: UInt8, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_stop_delivery")
private func qcIosStopDelivery(
    _ session: UnsafeMutableRawPointer?, _ ticketJson: UnsafePointer<CChar>?,
    _ reason: UInt8, _ nowMs: UInt64,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_operation_delivery_complete")
private func qcIosOperationDeliveryComplete(
    _ session: UnsafeMutableRawPointer?, _ operationJson: UnsafePointer<CChar>?,
    _ frameCount: Int, _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_encode_message")
private func qcIosEncodeMessage(
    _ messageType: UInt16, _ payload: UnsafePointer<UInt8>?, _ payloadLength: Int,
    _ output: UnsafeMutablePointer<UInt8>?, _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_current_preset")
private func qcIosCurrentPreset(
    _ session: UnsafeMutableRawPointer?, _ output: UnsafeMutablePointer<CChar>?,
    _ outputCapacity: Int
) -> Int
@_silgen_name("qc_ios_compose_global_tempo_settings")
private func qcIosComposeGlobalTempoSettings(
    _ globalJson: UnsafePointer<CChar>?, _ presetJson: UnsafePointer<CChar>?,
    _ output: UnsafeMutablePointer<CChar>?, _ outputCapacity: Int
) -> Int

/// Serial Swift access to the shared, platform-neutral Quad Cortex session runtime.
/// USB ownership remains with the iPad DriverKit client.
final class QCIosProtocolSession {
    private let queue = DispatchQueue(label: "org.freevia.qcremote.ios.qc-protocol")
    private let session: UnsafeMutableRawPointer
    private let initialJsonCapacity = 8 * 1024

    enum BridgeError: Error {
        case unavailable
        case operationFailed
        case invalidResponse
    }

    init() throws {
        guard let session = qcIosSessionCreate() else { throw BridgeError.unavailable }
        self.session = session
    }

    deinit {
        qcIosSessionDestroy(session)
    }

    func opened(nowMs: UInt64) throws -> UInt64 {
        try queue.sync {
            let connection = qcIosSessionOpened(session, nowMs)
            guard connection != 0 else { throw BridgeError.operationFailed }
            return connection
        }
    }

    func reportEpoch(connection: UInt64) throws -> UInt64 {
        try queue.sync {
            let epoch = qcIosReportEpoch(session, connection)
            guard epoch != 0 else { throw BridgeError.operationFailed }
            return epoch
        }
    }

    /// `nil` means the report was accepted but did not complete a logical frame.
    func pushReport(_ report: Data, connection: UInt64, epoch: UInt64, receivedAtMs: UInt64) throws -> [String: Any]? {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                report.withUnsafeBytes { bytes in
                    qcIosPushReport(
                        session, connection, epoch, bytes.bindMemory(to: UInt8.self).baseAddress,
                        report.count, receivedAtMs, output, capacity
                    )
                }
            }
            if result is NSNull { return nil }
            guard let event = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return event
        }
    }

    func nextHandshake(nowMs: UInt64, sessionID: String) throws -> [String: Any]? {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                sessionID.withCString { sessionIDPointer in
                    qcIosNextHandshake(session, nowMs, sessionIDPointer, output, capacity)
                }
            }
            if result is NSNull { return nil }
            guard let result = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return result["reports"] == nil ? nil : result
        }
    }

    func nextLifecycleBatch(nowMs: UInt64, unixTimeMs: UInt64) throws -> [String: Any]? {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                qcIosNextLifecycleBatch(session, nowMs, unixTimeMs, output, capacity)
            }
            if result is NSNull { return nil }
            guard let result = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return result["ticket"] == nil ? nil : result
        }
    }

    func lifecycleBatchDispatched(ticket: String, accepted: Bool, nowMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                ticket.withCString { ticketPointer in
                    qcIosLifecycleDispatched(session, ticketPointer, accepted, nowMs, output, capacity)
                }
            }
            guard let result = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return result
        }
    }

    func planGatewayWrite(method: String, params: [String: Any], startedAtMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            guard JSONSerialization.isValidJSONObject(params),
                  let data = try? JSONSerialization.data(withJSONObject: params),
                  let paramsJson = String(data: data, encoding: .utf8) else {
                throw BridgeError.invalidResponse
            }
            let result = try withJsonOutput { output, capacity in
                method.withCString { methodPointer in
                    paramsJson.withCString { paramsPointer in
                        qcIosPlanGatewayWrite(session, methodPointer, paramsPointer, startedAtMs, output, capacity)
                    }
                }
            }
            guard let result = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return result
        }
    }

    func beginPresetWorkflow(method: String, params: [String: Any], nowMs: UInt64) throws -> UInt64 {
        try queue.sync {
            let paramsJson = try jsonString(params)
            let result = try withJsonOutput { output, capacity in
                method.withCString { methodPointer in
                    paramsJson.withCString { paramsPointer in
                        qcIosBeginPresetWorkflow(session, methodPointer, paramsPointer, nowMs, output, capacity)
                    }
                }
            }
            guard let value = result as? [String: Any],
                  let id = (value["preparationId"] as? NSNumber)?.uint64Value else {
                throw BridgeError.invalidResponse
            }
            return id
        }
    }

    func advancePresetWorkflow(preparationId: UInt64, nowMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                qcIosAdvancePresetWorkflow(session, preparationId, nowMs, output, capacity)
            }
            guard let value = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return value
        }
    }

    func cancelPresetWorkflow(preparationId: UInt64) {
        queue.sync { qcIosCancelPresetWorkflow(session, preparationId) }
    }

    func beginBackup(name: String, nowMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                name.withCString { namePointer in
                    qcIosBeginBackup(session, namePointer, nowMs, output, capacity)
                }
            }
            guard let value = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return value
        }
    }

    func advanceBackup(nowMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                qcIosAdvanceBackup(session, nowMs, output, capacity)
            }
            guard let value = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return value
        }
    }

    func cancelBackup() {
        queue.sync { qcIosCancelBackup(session) }
    }

    func finishBackup() throws {
        try queue.sync {
            guard qcIosFinishBackup(session) else { throw BridgeError.operationFailed }
        }
    }

    func beginGatewayVerification(context: [String: Any], verification: Any, method: String, afterSequence: UInt64, startedAtMs: UInt64) throws -> UInt64 {
        try queue.sync {
            let contextJson = try jsonString(context)
            let verificationJson = try jsonString(verification)
            let result = try withJsonOutput { output, capacity in
                contextJson.withCString { contextPointer in
                    verificationJson.withCString { verificationPointer in
                        method.withCString { methodPointer in
                            qcIosBeginGatewayVerification(session, contextPointer, verificationPointer, methodPointer, afterSequence, startedAtMs, output, capacity)
                        }
                    }
                }
            }
            guard let value = result as? [String: Any], let id = (value["verificationId"] as? NSNumber)?.uint64Value else { throw BridgeError.invalidResponse }
            return id
        }
    }

    func advanceGatewayVerification(id: UInt64, observationSequence: UInt64, nowMs: UInt64, method: String, verification: Any) throws -> [String: Any] {
        try queue.sync {
            let verificationJson = try jsonString(verification)
            let result = try withJsonOutput { output, capacity in
                verificationJson.withCString { verificationPointer in
                    method.withCString { methodPointer in
                        qcIosAdvanceGatewayVerification(session, id, observationSequence, nowMs, methodPointer, verificationPointer, output, capacity)
                    }
                }
            }
            guard let value = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return value
        }
    }

    func planGatewayRead(method: String, params: [String: Any], startedAtMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            guard JSONSerialization.isValidJSONObject(params),
                  let data = try? JSONSerialization.data(withJSONObject: params),
                  let paramsJson = String(data: data, encoding: .utf8) else {
                throw BridgeError.invalidResponse
            }
            let result = try withJsonOutput { output, capacity in
                method.withCString { methodPointer in
                    paramsJson.withCString { paramsPointer in
                        qcIosPlanGatewayRead(session, methodPointer, paramsPointer, startedAtMs, output, capacity)
                    }
                }
            }
            guard let plan = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return plan
        }
    }

    func observeGatewayRead(plan: [String: Any], event: [String: Any], nowMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            guard let projection = plan["projection"],
                  let projectionData = try? JSONSerialization.data(withJSONObject: projection, options: [.fragmentsAllowed]),
                  let projectionJson = String(data: projectionData, encoding: .utf8),
                  let expectedType = (plan["responseType"] as? NSNumber)?.uint16Value,
                  let actualType = (event["messageType"] as? NSNumber)?.uint16Value,
                  let payloadValues = event["payload"] as? [NSNumber],
                  let startedAtMs = (plan["startedAtMs"] as? NSNumber)?.uint64Value,
                  let timeoutMs = (plan["timeoutMs"] as? NSNumber)?.uint64Value,
                  let observation = event["observation"] as? [String: Any],
                  let ingress = observation["ingress"] as? [String: Any],
                  let firstMs = (ingress["firstReportAtMs"] as? NSNumber)?.uint64Value,
                  let lastMs = (ingress["lastReportAtMs"] as? NSNumber)?.uint64Value else {
                throw BridgeError.invalidResponse
            }
            let payloadInts = payloadValues.map(\.intValue)
            guard payloadInts.allSatisfy({ (0...255).contains($0) }) else { throw BridgeError.invalidResponse }
            let payload = payloadInts.map(UInt8.init)
            let result = try withJsonOutput { output, capacity in
                projectionJson.withCString { projectionPointer in
                    payload.withUnsafeBufferPointer { buffer in
                        qcIosObserveGatewayRead(
                            projectionPointer, expectedType, actualType, buffer.baseAddress,
                            payload.count, startedAtMs, timeoutMs, firstMs, lastMs,
                            nowMs, output, capacity
                        )
                    }
                }
            }
            guard let observation = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return observation
        }
    }

    func prepareDelivery(dispatchTicket: [String: Any], messageType: UInt16, reportCount: Int, nowMs: UInt64) throws -> [String: Any] {
        try queue.sync {
            let dispatchJson = try jsonString(dispatchTicket)
            let result = try withJsonOutput { output, capacity in
                dispatchJson.withCString { pointer in
                    qcIosPrepareDelivery(session, pointer, messageType, reportCount, nowMs, output, capacity)
                }
            }
            guard let ticket = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return ticket
        }
    }

    func deliveryAdmitted(ticket: [String: Any]) throws -> [String: Any] {
        try withDeliveryTicket(ticket) { pointer, output, capacity in
            qcIosDeliveryAdmitted(session, pointer, output, capacity)
        }
    }

    func beginReport(ticket: [String: Any], index: Int, nowMs: UInt64) throws -> [String: Any] {
        try withDeliveryTicket(ticket) { pointer, output, capacity in
            qcIosBeginReport(session, pointer, index, nowMs, output, capacity)
        }
    }

    func finishReport(ticket: [String: Any], index: Int, outcome: UInt8, nowMs: UInt64) throws -> [String: Any] {
        try withDeliveryTicket(ticket) { pointer, output, capacity in
            qcIosFinishReport(session, pointer, index, outcome, nowMs, output, capacity)
        }
    }

    func stopDelivery(ticket: [String: Any], reason: UInt8, nowMs: UInt64) throws -> [String: Any] {
        try withDeliveryTicket(ticket) { pointer, output, capacity in
            qcIosStopDelivery(session, pointer, reason, nowMs, output, capacity)
        }
    }

    func operationDeliveryComplete(operationTicket: [String: Any], frameCount: Int) throws -> Bool {
        try queue.sync {
            let operationJson = try jsonString(operationTicket)
            let result = try withJsonOutput { output, capacity in
                operationJson.withCString { pointer in
                    qcIosOperationDeliveryComplete(session, pointer, frameCount, output, capacity)
                }
            }
            guard let value = result as? [String: Any], let complete = value["complete"] as? Bool else {
                throw BridgeError.invalidResponse
            }
            return complete
        }
    }

    func encodeMessage(messageType: UInt16, payload: Data) throws -> [Data] {
        try queue.sync {
            let requiredResult = payload.withUnsafeBytes { input in
                qcIosEncodeMessage(
                    messageType, input.bindMemory(to: UInt8.self).baseAddress,
                    payload.count, nil, 0
                )
            }
            guard requiredResult < -1 else { throw BridgeError.operationFailed }
            let requiredBytes = -requiredResult - 1
            guard requiredBytes > 0, requiredBytes <= 8 * 1024 * 1024,
                  requiredBytes % 129 == 0 else { throw BridgeError.invalidResponse }
            var output = [UInt8](repeating: 0, count: requiredBytes)
            let reportCount = output.withUnsafeMutableBufferPointer { target in
                payload.withUnsafeBytes { input in
                    qcIosEncodeMessage(
                        messageType, input.bindMemory(to: UInt8.self).baseAddress,
                        payload.count, target.baseAddress, target.count
                    )
                }
            }
            guard reportCount > 0, reportCount * 129 == requiredBytes else {
                throw BridgeError.operationFailed
            }
            return (0..<reportCount).map { index in
                Data(output[(index * 129)..<((index + 1) * 129)])
            }
        }
    }

    func currentPreset() throws -> [String: Any]? {
        try queue.sync {
            let result = try withJsonOutput { output, capacity in
                qcIosCurrentPreset(session, output, capacity)
            }
            if result is NSNull { return nil }
            guard let snapshot = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return snapshot
        }
    }

    func composeGlobalTempoSettings(global: [String: Any], preset: [String: Any]) throws -> [String: Any] {
        try queue.sync {
            let globalJson = try jsonString(global)
            let presetJson = try jsonString(preset)
            let result = try withJsonOutput { output, capacity in
                globalJson.withCString { globalPointer in
                    presetJson.withCString { presetPointer in
                        qcIosComposeGlobalTempoSettings(globalPointer, presetPointer, output, capacity)
                    }
                }
            }
            guard let composed = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return composed
        }
    }

    private func withDeliveryTicket(_ ticket: [String: Any], operation: (UnsafePointer<CChar>?, UnsafeMutablePointer<CChar>?, Int) -> Int) throws -> [String: Any] {
        try queue.sync {
            let ticketJson = try jsonString(ticket)
            let result = try withJsonOutput { output, capacity in
                ticketJson.withCString { pointer in operation(pointer, output, capacity) }
            }
            guard let record = result as? [String: Any] else { throw BridgeError.invalidResponse }
            return record
        }
    }

    private func jsonString(_ value: Any) throws -> String {
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value),
              let string = String(data: data, encoding: .utf8) else {
            throw BridgeError.invalidResponse
        }
        return string
    }

    private func withJsonOutput(_ operation: (UnsafeMutablePointer<CChar>?, Int) -> Int) throws -> Any {
        var output = [CChar](repeating: 0, count: initialJsonCapacity)
        var count = output.withUnsafeMutableBufferPointer { buffer in
            operation(buffer.baseAddress, buffer.count)
        }
        if count < -1 {
            let requiredCapacity = -count
            guard requiredCapacity <= 8 * 1024 * 1024 else { throw BridgeError.invalidResponse }
            output = [CChar](repeating: 0, count: requiredCapacity)
            count = output.withUnsafeMutableBufferPointer { buffer in
                operation(buffer.baseAddress, buffer.count)
            }
        }
        if count == 0 { return NSNull() }
        guard count > 0, count <= output.count else { throw BridgeError.operationFailed }
        let data = Data(output.prefix(count).map { UInt8(bitPattern: $0) })
        guard let value = try? JSONSerialization.jsonObject(with: data) else {
            throw BridgeError.invalidResponse
        }
        return value
    }
}
