import CryptoKit
import Foundation

enum SigningError: Error, CustomStringConvertible {
    case usage
    case invalidPem
    case invalidDer
    case unsupportedKey
    case invalidArguments

    var description: String {
        switch self {
        case .usage:
            return "usage: sign-ed25519-receipt.swift <private-key.pem> <input> <signature>"
        case .invalidPem:
            return "private key is not a valid PKCS#8 PEM document"
        case .invalidDer:
            return "private key contains invalid DER"
        case .unsupportedKey:
            return "private key is not an Ed25519 PKCS#8 key"
        case .invalidArguments:
            return "invalid signing arguments"
        }
    }
}

func readLength(_ data: Data, offset: inout Int) throws -> Int {
    guard offset < data.count else { throw SigningError.invalidDer }
    let first = data[offset]
    offset += 1
    if first & 0x80 == 0 {
        return Int(first)
    }
    let byteCount = Int(first & 0x7f)
    guard byteCount > 0, byteCount <= 4, offset + byteCount <= data.count else {
        throw SigningError.invalidDer
    }
    var length = 0
    for _ in 0..<byteCount {
        length = (length << 8) | Int(data[offset])
        offset += 1
    }
    return length
}

func readElement(_ data: Data, offset: inout Int) throws -> (tag: UInt8, value: Data) {
    guard offset < data.count else { throw SigningError.invalidDer }
    let tag = data[offset]
    offset += 1
    let length = try readLength(data, offset: &offset)
    guard length >= 0, offset + length <= data.count else { throw SigningError.invalidDer }
    let value = data.subdata(in: offset..<(offset + length))
    offset += length
    return (tag, value)
}

func privateSeed(from pem: String) throws -> Data {
    let lines = pem.split(whereSeparator: \.isNewline).map(String.init)
    guard lines.first == "-----BEGIN PRIVATE KEY-----",
          lines.last == "-----END PRIVATE KEY-----" else {
        throw SigningError.invalidPem
    }
    let base64 = lines.dropFirst().dropLast().joined()
    guard let der = Data(base64Encoded: base64) else { throw SigningError.invalidPem }

    var outerOffset = 0
    let outer = try readElement(der, offset: &outerOffset)
    guard outer.tag == 0x30, outerOffset == der.count else { throw SigningError.invalidDer }
    var bodyOffset = 0
    let version = try readElement(outer.value, offset: &bodyOffset)
    let algorithm = try readElement(outer.value, offset: &bodyOffset)
    let privateKey = try readElement(outer.value, offset: &bodyOffset)
    guard version.tag == 0x02, algorithm.tag == 0x30, privateKey.tag == 0x04 else {
        throw SigningError.invalidDer
    }

    var algorithmOffset = 0
    let oid = try readElement(algorithm.value, offset: &algorithmOffset)
    guard oid.tag == 0x06, oid.value == Data([0x2b, 0x65, 0x70]) else {
        throw SigningError.unsupportedKey
    }

    if privateKey.value.count == 32 {
        return privateKey.value
    }
    var nestedOffset = 0
    let seed = try readElement(privateKey.value, offset: &nestedOffset)
    guard seed.tag == 0x04, seed.value.count == 32 else { throw SigningError.unsupportedKey }
    return seed.value
}

do {
    guard CommandLine.arguments.count == 4 else { throw SigningError.usage }
    let keyPath = CommandLine.arguments[1]
    let inputPath = CommandLine.arguments[2]
    let signaturePath = CommandLine.arguments[3]
    let pem = try String(contentsOfFile: keyPath, encoding: .utf8)
    let seed = try privateSeed(from: pem)
    let privateKey = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
    let message = try Data(contentsOf: URL(fileURLWithPath: inputPath), options: [.mappedIfSafe])
    let signature = try privateKey.signature(for: message)
    guard signature.count == 64 else { throw SigningError.invalidArguments }
    try signature.write(to: URL(fileURLWithPath: signaturePath), options: [.atomic])
} catch let error as SigningError {
    fputs("\(error.description)\n", stderr)
    exit(2)
} catch {
    fputs("Ed25519 signing failed: \(error)\n", stderr)
    exit(1)
}
