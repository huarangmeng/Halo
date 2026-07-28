import Foundation
import Testing

@testable import HaloDiscoveryApple

@Test func configurationAcceptsExactPresencePacket() throws {
    let configuration = try HaloBleConfiguration(presence: Data(repeating: 0x11, count: 58))
    #expect(configuration.presence.count == 58)
    #expect(configuration.maximumConcurrentGattConnections == 2)
}

@Test func configurationRejectsMalformedPresencePacket() {
    #expect(throws: HaloBleConfigurationError.invalidPresenceLength(57)) {
        try HaloBleConfiguration(presence: Data(repeating: 0x11, count: 57))
    }
}

@Test func connectionLimitIsBounded() {
    let data = Data(repeating: 0x11, count: 58)
    #expect(throws: HaloBleConfigurationError.invalidConnectionLimit(0)) {
        try HaloBleConfiguration(presence: data, maximumConcurrentGattConnections: 0)
    }
    #expect(throws: HaloBleConfigurationError.invalidConnectionLimit(9)) {
        try HaloBleConfiguration(presence: data, maximumConcurrentGattConnections: 9)
    }
}

@Test func retryIntervalsAreValidated() {
    let data = Data(repeating: 0x11, count: 58)
    #expect(throws: HaloBleConfigurationError.invalidInterval("connectionTimeout")) {
        try HaloBleConfiguration(presence: data, connectionTimeout: 0)
    }
    #expect(throws: HaloBleConfigurationError.invalidInterval("peerRetry")) {
        try HaloBleConfiguration(presence: data, peerRetryBase: 10, peerRetryMaximum: 5)
    }
}

@Test func retryBackoffIsBounded() {
    #expect(haloBleBoundedBackoff(base: 2, maximum: 30, attempt: 1) == 2)
    #expect(haloBleBoundedBackoff(base: 2, maximum: 30, attempt: 4) == 16)
    #expect(haloBleBoundedBackoff(base: 2, maximum: 30, attempt: 8) == 30)
}

@Test func protocolUUIDsRemainStable() {
    #expect(HaloBleUUID.publicLocalName == "Halo")
    #expect(HaloBleUUID.service.uuidString == "B6882C7F-D426-4CB6-9012-D40BDE5E2000")
    #expect(HaloBleUUID.presence.uuidString == "8C2E5E61-4C6A-4C64-B804-1301A15251A0")
    #expect(HaloBleUUID.wakeLan.uuidString == "4FE6E851-DBC1-4A86-8E49-FCF1EABC1C82")
}

@Test func presenceReadSupportsAttBlobOffsets() {
    let presence = Data(0 ..< 58)

    #expect(haloBleReadSlice(presence, offset: 0) == presence)
    #expect(haloBleReadSlice(presence, offset: 22) == Data(22 ..< 58))
    #expect(haloBleReadSlice(presence, offset: 58) == Data())
    #expect(haloBleReadSlice(presence, offset: 59) == nil)
    #expect(haloBleReadSlice(presence, offset: -1) == nil)
}
