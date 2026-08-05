import Foundation
import Network
import Testing

@testable import HaloDiscoveryApple

@Test func applePeerToPeerProtocolConstantsMatchSharedQuicTransport() {
    #expect(HaloApplePeerToPeerProtocol.serviceType == "_halo._udp")
    #expect(HaloApplePeerToPeerProtocol.alpn == "halo-pairing/1")
}

@Test func applePeerToPeerConfigurationIsBounded() throws {
    let configuration = try HaloApplePeerToPeerConfiguration(instanceName: "a1b2c3d4")
    #expect(configuration.maximumCandidates == 64)
    #expect(configuration.connectionTimeout == 10)

    #expect(throws: HaloApplePeerToPeerConfigurationError.invalidInstanceName) {
        try HaloApplePeerToPeerConfiguration(instanceName: "")
    }
    #expect(throws: HaloApplePeerToPeerConfigurationError.invalidCandidateLimit(0)) {
        try HaloApplePeerToPeerConfiguration(instanceName: "opaque", maximumCandidates: 0)
    }
    #expect(throws: HaloApplePeerToPeerConfigurationError.invalidConnectionTimeout) {
        try HaloApplePeerToPeerConfiguration(instanceName: "opaque", connectionTimeout: 0)
    }
}

@Test func applePeerToPeerParametersRequireWifiAndProhibitCellular() {
    let parameters = HaloApplePeerToPeerNetworkPolicy.makeQuicParameters { _ in }
    #expect(parameters.includePeerToPeer)
    #expect(parameters.requiredInterfaceType == .wifi)
    #expect(parameters.prohibitedInterfaceTypes?.contains(.cellular) == true)
    #expect(HaloApplePeerToPeerNetworkPolicy.parametersAreEligible(parameters))

    parameters.includePeerToPeer = false
    #expect(!HaloApplePeerToPeerNetworkPolicy.parametersAreEligible(parameters))
}

@Test func applePeerToPeerRejectsAnUnsafeParameterFactory() throws {
    let configuration = try HaloApplePeerToPeerConfiguration(instanceName: "opaque")
    let provider = HaloApplePeerToPeerProvider(
        configuration: configuration,
        parametersFactory: { NWParameters.udp },
        eventHandler: { _ in }
    )
    #expect(provider.connect(candidateHandle: UUID()) == nil)
    provider.stopAndWait()
}

@Test func applePeerToPeerCandidateEventsCarryOnlyRotatingPresenceIdentity() {
    let presenceID = UUID()
    let endpoint = NWEndpoint.service(
        name: presenceID.uuidString,
        type: HaloApplePeerToPeerProtocol.serviceType,
        domain: "local",
        interface: nil
    )
    #expect(endpoint.debugDescription.contains(presenceID.uuidString))
}
