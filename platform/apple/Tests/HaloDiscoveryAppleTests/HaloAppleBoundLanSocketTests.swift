import Darwin
import Testing
@testable import HaloDiscoveryApple

@Test
func dualStackSocketIsPinnedToTheRequestedInterface() throws {
    let loopbackIndex = Darwin.if_nametoindex("lo0")
    #expect(loopbackIndex != 0)
    let descriptor = try HaloAppleBoundLanSocket.makeDualStackSocket(
        interfaceIndex: loopbackIndex
    )
    defer { Darwin.close(descriptor) }

    var actualIndex: UInt32 = 0
    var actualIndexLength = socklen_t(MemoryLayout<UInt32>.size)
    let optionResult = withUnsafeMutablePointer(to: &actualIndex) { pointer in
        Darwin.getsockopt(
            descriptor,
            IPPROTO_IPV6,
            IPV6_BOUND_IF,
            pointer,
            &actualIndexLength
        )
    }
    #expect(optionResult == 0)
    #expect(actualIndex == loopbackIndex)

    var ipv6Only: Int32 = -1
    var ipv6OnlyLength = socklen_t(MemoryLayout<Int32>.size)
    let dualStackResult = withUnsafeMutablePointer(to: &ipv6Only) { pointer in
        Darwin.getsockopt(
            descriptor,
            IPPROTO_IPV6,
            IPV6_V6ONLY,
            pointer,
            &ipv6OnlyLength
        )
    }
    #expect(dualStackResult == 0)
    #expect(ipv6Only == 0)

    var address = sockaddr_in6()
    var addressLength = socklen_t(MemoryLayout<sockaddr_in6>.size)
    let addressResult = withUnsafeMutablePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
            Darwin.getsockname(descriptor, socketAddress, &addressLength)
        }
    }
    #expect(addressResult == 0)
    #expect(address.sin6_family == sa_family_t(AF_INET6))
    #expect(address.sin6_port != 0)
}

@Test
func zeroInterfaceIndexIsRejectedWithoutCreatingASocket() {
    #expect(throws: HaloAppleBoundLanSocketError.invalidInterface) {
        try HaloAppleBoundLanSocket.makeDualStackSocket(interfaceIndex: 0)
    }
}

@Test
func sharedLanRejectsExpensiveOrConstrainedPaths() {
    #expect(HaloAppleBoundLanSocket.allows(
        isExpensive: false,
        isConstrained: false,
        scope: .shared
    ))
    #expect(!HaloAppleBoundLanSocket.allows(
        isExpensive: true,
        isConstrained: false,
        scope: .shared
    ))
    #expect(!HaloAppleBoundLanSocket.allows(
        isExpensive: false,
        isConstrained: true,
        scope: .shared
    ))
}

@Test
func explicitHotspotApprovalAllowsAppleExpensiveClassification() {
    #expect(HaloAppleBoundLanSocket.allows(
        isExpensive: true,
        isConstrained: true,
        scope: .userApprovedHotspot
    ))
}
