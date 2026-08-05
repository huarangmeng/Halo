import Foundation
import Network
import Testing

@testable import HaloDiscoveryApple

@Test func bonjourResolverRequiresAServiceEndpoint() {
    #expect(throws: HaloBonjourServiceResolverError.invalidServiceEndpoint) {
        _ = try HaloBonjourServiceResolver(
            endpoint: .hostPort(host: "127.0.0.1", port: 4433),
            queue: .main,
            completion: { _ in }
        )
    }
}

@Test func bonjourResolverConvertsNetworkByteOrderPort() {
    let resolved = "halo.local.".withCString {
        haloBonjourHostPort(host: $0, networkOrderPort: UInt16(4433).bigEndian)
    }
    #expect(resolved == HaloResolvedBonjourEndpoint(host: "halo.local.", port: 4433))
    #expect(resolved?.networkEndpoint != nil)
}
