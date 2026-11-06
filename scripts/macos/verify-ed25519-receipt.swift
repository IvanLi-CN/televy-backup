import CryptoKit
import Foundation

enum VerificationError: Error, CustomStringConvertible, Equatable {
    case usage
    case invalidPem
    case invalidDer
    case unsupportedKey
    case invalidSignature

    var description: String {
        switch self {
        case .usage:
            return "usage: verify-ed25519-receipt.swift <public-key.pem> <input> <signature>"
        case .invalidPem:
            return "public key is not a valid SubjectPublicKeyInfo PEM document"
        case .invalidDer:
            return "public key contains invalid DER"
        case .unsupportedKey:
            return "public key is not an Ed25519 SubjectPublicKeyInfo key"
        case .invalidSignature:
            return "Ed25519 signature is invalid"
        }
    }
}

func readLength(_ data: Data, offset: inout Int) throws -> Int {
    guard offset < data.count else { throw VerificationError.invalidDer }
    let first = data[offset]
    offset += 1
    if first & 0x80 == 0 {
        return Int(first)
    }
    let byteCount = Int(first & 0x7f)
    guard byteCount > 0, byteCount <= 4, offset + byteCount <= data.count else {
        throw VerificationError.invalidDer
    }
    var length = 0
    for _ in 0..<byteCount {
        length = (length << 8) | Int(data[offset])
        offset += 1
    }
    return length
}

func readElement(_ data: Data, offset: inout Int) throws -> (tag: UInt8, value: Data) {
    guard offset < data.count else { throw VerificationError.invalidDer }
    let tag = data[offset]
    offset += 1
    let length = try readLength(data, offset: &offset)
    guard length >= 0, offset + length <= data.count else { throw VerificationError.invalidDer }
    let value = data.subdata(in: offset..<(offset + length))
    offset += length
    return (tag, value)
}

func publicKeyBytes(from pem: String) throws -> Data {
    let lines = pem.split(whereSeparator: \.isNewline).map(String.init)
    guard lines.first == "-----BEGIN PUBLIC KEY-----",
          lines.last == "-----END PUBLIC KEY-----" else {
        throw VerificationError.invalidPem
    }
    let base64 = lines.dropFirst().dropLast().joined()
    guard let der = Data(base64Encoded: base64) else { throw VerificationError.invalidPem }

    var outerOffset = 0
    let outer = try readElement(der, offset: &outerOffset)
    guard outer.tag == 0x30, outerOffset == der.count else { throw VerificationError.invalidDer }

    var bodyOffset = 0
    let algorithm = try readElement(outer.value, offset: &bodyOffset)
    let subject = try readElement(outer.value, offset: &bodyOffset)
    guard algorithm.tag == 0x30, subject.tag == 0x03, bodyOffset == outer.value.count else {
        throw VerificationError.invalidDer
    }

    var algorithmOffset = 0
    let oid = try readElement(algorithm.value, offset: &algorithmOffset)
    guard oid.tag == 0x06, oid.value == Data([0x2b, 0x65, 0x70]) else {
        throw VerificationError.unsupportedKey
    }
    guard subject.value.first == 0, subject.value.count == 33 else {
        throw VerificationError.unsupportedKey
    }
    return Data(subject.value.dropFirst())
}

do {
    guard CommandLine.arguments.count == 4 else { throw VerificationError.usage }
    let publicKeyPath = CommandLine.arguments[1]
    let inputPath = CommandLine.arguments[2]
    let signaturePath = CommandLine.arguments[3]
    let pem = try String(contentsOfFile: publicKeyPath, encoding: .utf8)
    let rawPublicKey = try publicKeyBytes(from: pem)
    let publicKey = try Curve25519.Signing.PublicKey(rawRepresentation: rawPublicKey)
    let message = try Data(contentsOf: URL(fileURLWithPath: inputPath), options: [.mappedIfSafe])
    let signature = try Data(contentsOf: URL(fileURLWithPath: signaturePath), options: [.mappedIfSafe])
    guard signature.count == 64, publicKey.isValidSignature(signature, for: message) else {
        throw VerificationError.invalidSignature
    }
} catch let error as VerificationError {
    fputs("\(error.description)\n", stderr)
    exit(error == .invalidSignature ? 1 : 2)
} catch {
    fputs("Ed25519 signature verification failed: \(error)\n", stderr)
    exit(2)
}
