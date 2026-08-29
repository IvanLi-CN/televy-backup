import Darwin
import Foundation
import os

struct ControlRequestFailure: Error {
    let code: String
    let message: String
    let retryable: Bool
}

func controlFailureMessage(_ failure: ControlRequestFailure) -> String {
    switch failure.code {
    case "control.method_not_found":
        return "This app is incompatible with the installed TelevyBackup service. Update the service and retry."
    case "control.unavailable":
        return "TelevyBackup service is unavailable."
    case "control.timeout":
        return "TelevyBackup service did not respond in time."
    case "secrets.vault_unavailable":
        return "The encrypted secrets vault is unavailable."
    case "settings.revision_conflict":
        return "Settings changed elsewhere. Reload before saving again."
    default:
        return failure.message
    }
}

private struct ControlIPCError: Decodable {
    let code: String
    let message: String
    let retryable: Bool
}

private struct ControlIPCResponse<Response: Decodable>: Decodable {
    let type: String
    let id: String
    let ok: Bool
    let result: Response?
    let error: ControlIPCError?
}

private struct ControlOperationAccepted: Decodable {
    let operationId: String
}

private enum ControlJSONValue: Codable {
    case object([String: ControlJSONValue])
    case array([ControlJSONValue])
    case string(String)
    case number(Double)
    case bool(Bool)
    case null

    init(from decoder: Decoder) throws {
        if let container = try? decoder.container(keyedBy: DynamicCodingKey.self) {
            var object: [String: ControlJSONValue] = [:]
            for key in container.allKeys {
                object[key.stringValue] = try container.decode(ControlJSONValue.self, forKey: key)
            }
            self = .object(object)
            return
        }
        if var container = try? decoder.unkeyedContainer() {
            var array: [ControlJSONValue] = []
            while !container.isAtEnd {
                array.append(try container.decode(ControlJSONValue.self))
            }
            self = .array(array)
            return
        }
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .object(value):
            var container = encoder.container(keyedBy: DynamicCodingKey.self)
            for (key, item) in value {
                try container.encode(item, forKey: DynamicCodingKey(stringValue: key)!)
            }
        case let .array(value):
            var container = encoder.unkeyedContainer()
            for item in value { try container.encode(item) }
        case let .string(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .number(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .bool(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case .null:
            var container = encoder.singleValueContainer()
            try container.encodeNil()
        }
    }
}

private struct DynamicCodingKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init?(stringValue: String) {
        self.stringValue = stringValue
        intValue = nil
    }

    init?(intValue: Int) {
        stringValue = String(intValue)
        self.intValue = intValue
    }
}

private struct ControlOperationStatus: Decodable {
    let operationId: String
    let state: String
    let progress: ControlJSONValue?
    let result: ControlJSONValue?
    let error: ControlIPCError?
}

enum ControlIPCClient {
    private static let logger = Logger(subsystem: "com.ivan.televybackup", category: "control-ipc")
    private static let maxResponseBytes = 8 * 1024 * 1024
    private static let connectTimeoutSeconds: Double = 1
    private static let requestTimeoutSeconds: Double = 10

    static func request<Response: Decodable>(
        socketPath: String,
        method: String,
        params: [String: Any] = [:],
        timeoutSeconds: Double = requestTimeoutSeconds
    ) -> Result<Response, ControlRequestFailure> {
        let requestId = UUID().uuidString
        logger.debug("control request started id=\(requestId, privacy: .public) method=\(method, privacy: .public)")
        let envelope: [String: Any] = [
            "type": "control.request",
            "id": requestId,
            "method": method,
            "params": params,
        ]
        guard JSONSerialization.isValidJSONObject(envelope),
              var request = try? JSONSerialization.data(withJSONObject: envelope)
        else {
            return .failure(.init(code: "control.invalid_request", message: "Request could not be encoded.", retryable: false))
        }
        request.append(0x0A)

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            return .failure(.init(code: "control.unavailable", message: "TelevyBackup service is unavailable.", retryable: true))
        }
        defer { close(fd) }

        guard setNonblocking(fd) else {
            return .failure(.init(code: "control.unavailable", message: "TelevyBackup service is unavailable.", retryable: true))
        }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let maxPathLength = MemoryLayout.size(ofValue: address.sun_path) - 1
        guard socketPath.utf8.count <= maxPathLength else {
            return .failure(.init(code: "control.invalid_request", message: "TelevyBackup service path is invalid.", retryable: false))
        }
        _ = socketPath.withCString { path in
            strncpy(&address.sun_path.0, path, maxPathLength)
        }

        let connectResult: Int32 = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        if connectResult != 0 {
            guard errno == EINPROGRESS, waitFor(fd: fd, events: Int16(POLLOUT), timeout: connectTimeoutSeconds), socketError(fd) == 0 else {
                return .failure(.init(code: "control.unavailable", message: "TelevyBackup service is unavailable.", retryable: true))
            }
        }

        guard writeAll(fd: fd, data: request, timeout: timeoutSeconds) else {
            return .failure(.init(code: "control.timeout", message: "TelevyBackup service did not accept the request.", retryable: true))
        }

        var response = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while response.count < maxResponseBytes {
            let remaining = deadline.timeIntervalSinceNow
            guard remaining > 0, waitFor(fd: fd, events: Int16(POLLIN), timeout: remaining) else {
                return .failure(.init(code: "control.timeout", message: "TelevyBackup service did not respond in time.", retryable: true))
            }
            let count = buffer.withUnsafeMutableBytes { rawBuffer in
                Darwin.read(fd, rawBuffer.baseAddress, rawBuffer.count)
            }
            if count > 0 {
                response.append(buffer, count: count)
                if response.contains(0x0A) { break }
            } else if count == 0 {
                break
            } else if errno != EINTR {
                return .failure(.init(code: "control.unavailable", message: "TelevyBackup service did not respond.", retryable: true))
            }
        }

        guard let newline = response.firstIndex(of: 0x0A) else {
            return .failure(.init(code: "control.invalid_response", message: "TelevyBackup service returned an invalid response.", retryable: true))
        }
        let line = response.prefix(upTo: newline)
        guard let decoded = try? JSONDecoder().decode(ControlIPCResponse<Response>.self, from: line),
              decoded.type == "control.response", decoded.id == requestId
        else {
            return .failure(.init(code: "control.invalid_response", message: "TelevyBackup service returned an invalid response.", retryable: true))
        }
        if decoded.ok, let result = decoded.result {
            logger.debug("control request completed id=\(requestId, privacy: .public) method=\(method, privacy: .public)")
            return .success(result)
        }
        let error = decoded.error ?? ControlIPCError(code: "control.failed", message: "TelevyBackup request failed.", retryable: false)
        logger.error("control request failed id=\(requestId, privacy: .public) method=\(method, privacy: .public) code=\(error.code, privacy: .public)")
        return .failure(.init(code: error.code, message: error.message, retryable: error.retryable))
    }

    static func requestOperation<Response: Decodable>(
        socketPath: String,
        method: String,
        params: [String: Any] = [:],
        timeoutSeconds: Double = requestTimeoutSeconds,
        operationTimeoutSeconds: Double
    ) -> Result<Response, ControlRequestFailure> {
        let startResult: Result<ControlOperationAccepted, ControlRequestFailure> = request(
            socketPath: socketPath,
            method: method,
            params: params,
            timeoutSeconds: timeoutSeconds
        )
        guard case let .success(accepted) = startResult else {
            if case let .failure(error) = startResult { return .failure(error) }
            return .failure(.init(code: "control.failed", message: "Operation could not start.", retryable: false))
        }

        let deadline = Date().addingTimeInterval(operationTimeoutSeconds)
        while deadline.timeIntervalSinceNow > 0 {
            let remaining = max(0.25, deadline.timeIntervalSinceNow)
            let status: Result<ControlOperationStatus, ControlRequestFailure> = request(
                socketPath: socketPath,
                method: "operation.get",
                params: ["operationId": accepted.operationId],
                timeoutSeconds: min(requestTimeoutSeconds, remaining)
            )
            switch status {
            case let .failure(error):
                return .failure(error)
            case let .success(status):
                switch status.state {
                case "pending", "running":
                    Thread.sleep(forTimeInterval: min(0.25, max(0, deadline.timeIntervalSinceNow)))
                case "succeeded":
                    guard let value = status.result,
                          let data = try? JSONEncoder().encode(value),
                          let decoded = try? JSONDecoder().decode(Response.self, from: data)
                    else {
                        return .failure(.init(code: "control.invalid_response", message: "Operation returned an invalid result.", retryable: false))
                    }
                    return .success(decoded)
                case "failed":
                    let error = status.error ?? ControlIPCError(code: "operation.failed", message: "Operation failed.", retryable: false)
                    return .failure(.init(code: error.code, message: error.message, retryable: error.retryable))
                default:
                    return .failure(.init(code: "operation.invalid_state", message: "Operation returned an invalid state.", retryable: false))
                }
            }
        }
        return .failure(.init(code: "control.timeout", message: "Operation did not finish in time.", retryable: true))
    }

    private static func setNonblocking(_ fd: Int32) -> Bool {
        let flags = fcntl(fd, F_GETFL, 0)
        return flags >= 0 && fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0
    }

    private static func socketError(_ fd: Int32) -> Int32 {
        var value: Int32 = 0
        var length = socklen_t(MemoryLayout<Int32>.size)
        getsockopt(fd, SOL_SOCKET, SO_ERROR, &value, &length)
        return value
    }

    private static func waitFor(fd: Int32, events: Int16, timeout: Double) -> Bool {
        var descriptor = pollfd(fd: fd, events: events, revents: 0)
        let milliseconds = Int32(max(1, min(Double(Int32.max), timeout * 1000).rounded()))
        repeat {
            let result = Darwin.poll(&descriptor, 1, milliseconds)
            if result > 0 { return descriptor.revents & (events | Int16(POLLERR) | Int16(POLLHUP)) != 0 }
            if result == 0 { return false }
        } while errno == EINTR
        return false
    }

    private static func writeAll(fd: Int32, data: Data, timeout: Double) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        return data.withUnsafeBytes { rawBuffer in
            guard let baseAddress = rawBuffer.baseAddress else { return true }
            var offset = 0
            while offset < data.count {
                let remaining = deadline.timeIntervalSinceNow
                guard remaining > 0, waitFor(fd: fd, events: Int16(POLLOUT), timeout: remaining) else { return false }
                let written = Darwin.write(fd, baseAddress.advanced(by: offset), data.count - offset)
                if written > 0 {
                    offset += written
                } else if written < 0, errno == EINTR {
                    continue
                } else {
                    return false
                }
            }
            return true
        }
    }
}
