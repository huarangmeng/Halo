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

@Test func protocolUUIDsRemainStable() {
    #expect(HaloBleUUID.service.uuidString == "B6882C7F-D426-4CB6-9012-D40BDE5E2000")
    #expect(HaloBleUUID.presence.uuidString == "8C2E5E61-4C6A-4C64-B804-1301A15251A0")
    #expect(HaloBleUUID.wakeLan.uuidString == "4FE6E851-DBC1-4A86-8E49-FCF1EABC1C82")
}
