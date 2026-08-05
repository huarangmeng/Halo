import Foundation
import Testing

@testable import HaloDiscoveryApple

@Test func appleQuicTlsIdentityRejectsMalformedMaterial() {
    #expect(throws: HaloAppleQuicTlsIdentityError.invalidCertificate) {
        var key = Data(repeating: 0, count: 97)
        key[0] = 0x04
        _ = try HaloAppleQuicTlsIdentity(
            certificateDER: Data(),
            privateKeyX963: key
        )
    }

    #expect(throws: HaloAppleQuicTlsIdentityError.invalidPrivateKey) {
        _ = try HaloAppleQuicTlsIdentity(
            certificateDER: Data([0x30]),
            privateKeyX963: Data(repeating: 0, count: 96)
        )
    }
}
