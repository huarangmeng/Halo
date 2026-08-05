import Foundation
import Testing

@testable import HaloDiscoveryApple

@Test func appleControlProtocolMatchesRustPairingConstants() {
    #expect(HaloAppleQuicControlProtocol.frameHeaderLength == 12)
    #expect(HaloAppleQuicControlProtocol.maximumFrameLength == 4_096)
    #expect(HaloAppleQuicControlProtocol.exporterLabel == "EXPORTER-Halo-Pairing-v1")
    #expect(HaloAppleQuicControlProtocol.exporterLength == 32)
}

@Test func appleControlFrameLengthReadsOnlyBoundedLengthField() throws {
    var header = Data(repeating: 0, count: 12)
    header.replaceSubrange(8 ..< 12, with: [0, 0, 0, 20])
    #expect(try haloAppleControlFrameLength(header: header) == 32)
}

@Test func appleControlFrameLengthRejectsTruncationAndOversize() {
    #expect(throws: HaloAppleQuicControlError.truncated) {
        try haloAppleControlFrameLength(header: Data(repeating: 0, count: 11))
    }

    var header = Data(repeating: 0, count: 12)
    header.replaceSubrange(8 ..< 12, with: [0, 0, 0x10, 0])
    #expect(throws: HaloAppleQuicControlError.invalidFrameLength(4_108)) {
        try haloAppleControlFrameLength(header: header)
    }
}
