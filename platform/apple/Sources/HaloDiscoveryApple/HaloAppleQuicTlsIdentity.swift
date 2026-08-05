import Foundation
import Security

public enum HaloAppleQuicTlsIdentityError: Error, Equatable {
    case invalidCertificate
    case invalidPrivateKey
    case identityCreationFailed
}

/// An in-memory, short-lived TLS identity for the Apple QUIC bearer.
///
/// The certificate is deliberately not a Halo peer identity. Certificate
/// verification accepts the ephemeral transport certificate, then the shared
/// Rust pairing protocol authenticates both peers and binds its transcript to
/// the QUIC TLS exporter before any peer is trusted.
public final class HaloAppleQuicTlsIdentity: @unchecked Sendable {
    public static let maximumCertificateLength = 16_384
    public static let privateKeyLength = 97

    private let identity: sec_identity_t
    private let verifyQueue = DispatchQueue(label: "org.halo.transport.apple-p2p.verify")

    public init(certificateDER: Data, privateKeyX963: Data) throws {
        guard privateKeyX963.count == Self.privateKeyLength,
              privateKeyX963.first == 0x04
        else {
            throw HaloAppleQuicTlsIdentityError.invalidPrivateKey
        }
        guard !certificateDER.isEmpty,
              certificateDER.count <= Self.maximumCertificateLength,
              let certificate = SecCertificateCreateWithData(nil, certificateDER as CFData)
        else {
            throw HaloAppleQuicTlsIdentityError.invalidCertificate
        }
        let attributes: [CFString: Any] = [
            kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeyClass: kSecAttrKeyClassPrivate,
            kSecAttrKeySizeInBits: 256,
        ]
        var keyError: Unmanaged<CFError>?
        guard let privateKey = SecKeyCreateWithData(
            privateKeyX963 as CFData,
            attributes as CFDictionary,
            &keyError
        ) else {
            throw HaloAppleQuicTlsIdentityError.invalidPrivateKey
        }
        guard let securityIdentity = SecIdentityCreate(nil, certificate, privateKey),
              let protocolIdentity = sec_identity_create(securityIdentity)
        else {
            throw HaloAppleQuicTlsIdentityError.identityCreationFailed
        }
        identity = protocolIdentity
    }

    public func configure(_ options: sec_protocol_options_t) {
        sec_protocol_options_set_local_identity(options, identity)
        sec_protocol_options_set_verify_block(
            options,
            { _, _, complete in complete(true) },
            verifyQueue
        )
    }
}
